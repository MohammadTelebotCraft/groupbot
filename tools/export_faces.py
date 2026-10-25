#!/usr/bin/env python3
"""Writes the face lock's two models into `assets/`, and checks the Rust pipeline's arithmetic.

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/export_faces.py            # fetch, pin, relax, write
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/export_faces.py --check DIR [DIR ...]

- `face_detect.onnx` — YuNet 2023mar (OpenCV Zoo, MIT). The published graph declares a fixed
  640x640 input, but every reshape in it flattens with -1 and both upsamples are by scale, so the
  only fixed thing is the declaration. It is relaxed here to `height`x`width`. That matters more
  than it looks: detected on a 640 letterbox, the landmarks of a face in an 800px photograph are
  coarse enough that SFace embeddings of the *same* face agree with OpenCV's at a median cosine
  of 0.91; detected at the picture's own size (padded to a multiple of 32, as OpenCV does), 78 of
  80 pictures give the same faces and the embeddings agree at 0.9999.
- `face_embed.onnx` — SFace 2021dec (OpenCV Zoo, Apache-2.0), fp32. The int8 build is a quarter
  of the size and moves an embedding to a median cosine of 0.976 from the fp32 one, which is a
  different face at the cut the lock uses; it is not taken.

`--check` re-runs the pipeline exactly as `facelock.rs` does it — YuNet decode, NMS, the
least-squares similarity onto the ArcFace five-point template, bilinear warp, SFace — and
compares every face against `cv2.FaceDetectorYN` + `cv2.FaceRecognizerSF` on the same files.
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import sys
import urllib.request

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent.parent
ASSETS = HERE / "assets"
SOURCES = {
    "face_detect.onnx": (
        "https://huggingface.co/opencv/face_detection_yunet/resolve/main/face_detection_yunet_2023mar.onnx",
        "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    ),
    "face_embed.onnx": (
        "https://huggingface.co/opencv/face_recognition_sface/resolve/main/face_recognition_sface_2021dec.onnx",
        "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    ),
}
TEMPLATE = np.array([[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366],
                     [41.5493, 92.3655], [70.7299, 92.2041]], np.float64)
CAP = 1280
SCORE = 0.7
NMS = 0.3


def fetch(url: str, sha: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as r:
        data = r.read()
    got = hashlib.sha256(data).hexdigest()
    if got != sha:
        sys.exit(f"{url}: SHA-256 {got}, expected {sha}")
    return data


def write() -> None:
    import onnx

    detect = onnx.load_from_string(fetch(*SOURCES["face_detect.onnx"]))
    dims = detect.graph.input[0].type.tensor_type.shape.dim
    for at, name in ((2, "height"), (3, "width")):
        dims[at].ClearField("dim_value")
        dims[at].dim_param = name
    for output in detect.graph.output:
        output.type.tensor_type.shape.dim[1].ClearField("dim_value")
        output.type.tensor_type.shape.dim[1].dim_param = "anchors"
    onnx.checker.check_model(detect)
    (ASSETS / "face_detect.onnx").write_bytes(detect.SerializeToString())
    (ASSETS / "face_embed.onnx").write_bytes(fetch(*SOURCES["face_embed.onnx"]))
    for name in ("face_detect.onnx", "face_embed.onnx"):
        data = (ASSETS / name).read_bytes()
        print(f"{name}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")


class Pipeline:
    """`facelock.rs`, line for line."""

    def __init__(self):
        import onnxruntime as ort

        self.detect = ort.InferenceSession(str(ASSETS / "face_detect.onnx"), providers=["CPUExecutionProvider"])
        self.embed_session = ort.InferenceSession(str(ASSETS / "face_embed.onnx"), providers=["CPUExecutionProvider"])
        self.names = [o.name for o in self.detect.get_outputs()]

    def faces(self, rgb: np.ndarray) -> list[tuple[float, np.ndarray, np.ndarray]]:
        from PIL import Image

        h, w = rgb.shape[:2]
        s = min(1.0, CAP / max(h, w))
        nw, nh = max(1, round(w * s)), max(1, round(h * s))
        pixels = (np.asarray(Image.fromarray(rgb).resize((nw, nh), Image.Resampling.BILINEAR), dtype=np.float32)
                  if s < 1.0 else rgb.astype(np.float32))
        ph, pw = (nh + 31) // 32 * 32, (nw + 31) // 32 * 32
        canvas = np.zeros((ph, pw, 3), np.float32)
        canvas[:nh, :nw] = pixels[:, :, ::-1]
        out = dict(zip(self.names, self.detect.run(None, {"input": canvas.transpose(2, 0, 1)[None]})))
        found = []
        for stride in (8, 16, 32):
            cols = pw // stride
            cls, obj = out[f"cls_{stride}"][0, :, 0], out[f"obj_{stride}"][0, :, 0]
            box, kps = out[f"bbox_{stride}"][0], out[f"kps_{stride}"][0]
            score = np.sqrt(np.clip(cls, 0, 1) * np.clip(obj, 0, 1))
            for idx in np.where(score >= SCORE)[0]:
                row, col = divmod(int(idx), cols)
                cx, cy = (col + box[idx, 0]) * stride, (row + box[idx, 1]) * stride
                bw, bh = np.exp(box[idx, 2]) * stride, np.exp(box[idx, 3]) * stride
                marks = np.array([[(kps[idx, 2 * n] + col) * stride, (kps[idx, 2 * n + 1] + row) * stride] for n in range(5)])
                found.append((float(score[idx]), np.array([cx - bw / 2, cy - bh / 2, bw, bh]) / s, marks / s))
        found.sort(key=lambda f: -f[0])
        kept = []
        for face in found:
            x, y, fw, fh = face[1]
            if all(iou((x, y, fw, fh), other[1]) <= NMS for other in kept):
                kept.append(face)
        return kept

    def embed(self, rgb: np.ndarray, marks: np.ndarray) -> np.ndarray:
        crop = warp(rgb, similarity(marks.astype(np.float64), TEMPLATE))
        e = self.embed_session.run(None, {"data": crop.transpose(2, 0, 1)[None].astype(np.float32)})[0][0]
        return e / np.linalg.norm(e)


def iou(a, b) -> float:
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    ix = max(0.0, min(ax + aw, bx + bw) - max(ax, bx))
    iy = max(0.0, min(ay + ah, by + bh) - max(ay, by))
    inter = ix * iy
    union = aw * ah + bw * bh - inter
    return inter / union if union > 0 else 0.0


def similarity(src: np.ndarray, dst: np.ndarray) -> np.ndarray:
    """Least-squares rotation + uniform scale + translation taking `src` onto `dst`."""
    ms, md = src.mean(0), dst.mean(0)
    a, b = src - ms, dst - md
    n = (a ** 2).sum()
    p = (a[:, 0] * b[:, 0] + a[:, 1] * b[:, 1]).sum() / n
    q = (a[:, 0] * b[:, 1] - a[:, 1] * b[:, 0]).sum() / n
    m = np.array([[p, -q, 0.0], [q, p, 0.0]])
    m[:, 2] = md - m[:, :2] @ ms
    return m


def warp(rgb: np.ndarray, m: np.ndarray, size: int = 112) -> np.ndarray:
    """Inverse-mapped bilinear sampling, zero outside the picture (`cv2.warpAffine` defaults)."""
    inv = np.linalg.inv(np.vstack([m, [0, 0, 1]]))
    ys, xs = np.mgrid[0:size, 0:size]
    pts = inv @ np.stack([xs.ravel(), ys.ravel(), np.ones(size * size)])
    sx, sy = pts[0], pts[1]
    x0, y0 = np.floor(sx).astype(int), np.floor(sy).astype(int)
    fx, fy = sx - x0, sy - y0
    h, w = rgb.shape[:2]
    image = rgb.astype(np.float32)

    def at(x, y):
        ok = (x >= 0) & (x < w) & (y >= 0) & (y < h)
        value = np.zeros((len(x), 3), np.float32)
        value[ok] = image[y[ok], x[ok]]
        return value

    out = (at(x0, y0) * ((1 - fx) * (1 - fy))[:, None] + at(x0 + 1, y0) * (fx * (1 - fy))[:, None]
           + at(x0, y0 + 1) * ((1 - fx) * fy)[:, None] + at(x0 + 1, y0 + 1) * (fx * fy)[:, None])
    return out.reshape(size, size, 3)


def check(dirs: list[str]) -> None:
    import cv2
    from PIL import Image

    pipeline = Pipeline()
    upstream = pathlib.Path.home() / ".cache/concepts/faces"
    upstream.mkdir(parents=True, exist_ok=True)
    det_path = upstream / "yunet_2023mar.onnx"
    rec_path = upstream / "sface_2021dec.onnx"
    if not det_path.exists():
        det_path.write_bytes(fetch(*SOURCES["face_detect.onnx"]))
    if not rec_path.exists():
        rec_path.write_bytes((ASSETS / "face_embed.onnx").read_bytes())
    det = cv2.FaceDetectorYN.create(str(det_path), "", (320, 320), SCORE, NMS, 5000)
    rec = cv2.FaceRecognizerSF.create(str(rec_path), "")
    same, total, cosines = 0, 0, []
    for directory in dirs:
        for path in sorted(pathlib.Path(directory).glob("*.jpg"))[:80]:
            rgb = np.asarray(Image.open(path).convert("RGB"))
            bgr = rgb[:, :, ::-1].copy()
            h, w = bgr.shape[:2]
            if max(h, w) > CAP:
                continue
            det.setInputSize((w, h))
            _, ref = det.detect(bgr)
            ref = [] if ref is None else ref
            mine = pipeline.faces(rgb)
            total += 1
            same += len(mine) == len(ref)
            for face in mine:
                if not len(ref):
                    break
                cx, cy = face[1][0] + face[1][2] / 2, face[1][1] + face[1][3] / 2
                j = int(np.argmin([(r[0] + r[2] / 2 - cx) ** 2 + (r[1] + r[3] / 2 - cy) ** 2 for r in ref]))
                theirs = rec.feature(rec.alignCrop(bgr, ref[j])).reshape(-1)
                cosines.append(float(pipeline.embed(rgb, face[2]) @ (theirs / np.linalg.norm(theirs))))
    cosines = np.array(cosines)
    print(f"faces agree on {same}/{total} pictures; embeddings against OpenCV: "
          f"median cosine {np.median(cosines):.4f}, {np.mean(cosines > 0.99) * 100:.1f}% above 0.99")


def reference() -> None:
    """The numbers `facelock::tests::the_embedder_agrees_with_the_reference_pipeline` pins: a
    synthetic picture and five fixed landmarks through the warp and SFace."""
    ys, xs = np.mgrid[0:220, 0:200]
    rgb = np.stack([(xs * 7 + ys * 3) % 256, (xs * 2 + ys * 5) % 256, (xs ^ ys) % 256], axis=-1).astype(np.uint8)
    marks = np.array([[70.0, 90.0], [130.0, 88.0], [100.0, 120.0], [76.0, 150.0], [126.0, 148.0]])
    print_ = Pipeline().embed(rgb, marks)
    print("    const REFERENCE_PRINT: [(usize, f32); 4] = [" + ", ".join(f"({at}, {print_[at]:.6f})" for at in (0, 31, 64, 127)) + "];")


def prints_json(directory: str, out: str) -> None:
    """Per picture: how many faces `facelock::prints` should keep and the largest one's
    embedding. `FACE_BENCH_IMAGES=DIR FACE_BENCH_JSON=OUT cargo test facelock -- --ignored`
    compares the Rust pipeline against it on real photographs."""
    import json

    from PIL import Image

    pipeline = Pipeline()
    rows = {}
    for path in sorted(pathlib.Path(directory).glob("*.jpg"))[:60]:
        rgb = np.asarray(Image.open(path).convert("RGB"))
        faces = [f for f in pipeline.faces(rgb) if min(f[1][2], f[1][3]) >= 24.0]
        faces.sort(key=lambda f: -(f[1][2] * f[1][3]))
        faces = faces[:6]
        rows[path.name] = {
            "faces": len(faces),
            "largest": pipeline.embed(rgb, faces[0][2]).tolist() if faces else None,
            "all": [pipeline.embed(rgb, face[2]).tolist() for face in faces],
            "areas": [float(face[1][2] * face[1][3]) for face in faces],
        }
    pathlib.Path(out).write_text(json.dumps(rows))
    print(f"{len(rows)} pictures -> {out}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", nargs="+")
    parser.add_argument("--reference", action="store_true")
    parser.add_argument("--prints-json", nargs=2, metavar=("DIR", "OUT"))
    args = parser.parse_args()
    if args.check:
        check(args.check)
    elif args.reference:
        reference()
    elif args.prints_json:
        prints_json(*args.prints_json)
    else:
        write()
    return 0


if __name__ == "__main__":
    sys.exit(main())
