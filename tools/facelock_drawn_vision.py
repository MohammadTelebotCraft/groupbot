#!/usr/bin/env python3
"""The general model's eye for a drawing of a built-in person: «قفل چهره» on Lottie stickers.

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/facelock_drawn_vision.py embed   # renders and region faces (GPU)
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/facelock_drawn_vision.py fit     # assets/face_drawn_vision.bin

A Lottie sticker is a vector drawing, and the face model's prints of a traced drawing tell Khamenei from the other
clerics badly: its drawn model (`facelock_people.py`, `facelock::drawn_likeness`) took 15 and 6 of 40 drawings of
him in the first two styles of `lottie_trace.py`, and one drawing of another cleric. The general model has seen him
drawn — posters, murals, caricatures — and a logistic direction on its embedding of the bot's own renders (raw, as
`imgfilter` reads every frame) beside it, with its veto on a drawn face it clearly disagrees with
(`facelock::DRAWN_VETO`), takes 22, 17, 20 and 2 of the same 40 in the four styles through the bot's decoders, and
none of 52 drawings of the other built-in people or 29 of nobody in particular in any of them. Over 7 361 held-out
everyday pictures, 31 413 pool, 46 275 bank-source and the 1 024 real reservoir pictures none clears its cut.

The direction is fitted on the renders of three styles of every photograph `facelock_people.py embed` read: the
person's own (their face the largest) against everyone else's — the other built-in people and the Commons faces
of clerics and officials, minus any whose largest face is within 0.40 of the person's centre — and 20 000 ordinary
pictures from `assets/imgfilter_pool.bin`. Its cut is cross-fitted: five folds by photograph, each scored by the
direction fitted on the other four, the cut at the highest of every out-of-fold drawing of somebody else (some
11 000), the region pictures that do not show the person and the bank's 46 275 source pictures. The shipped
direction is fitted on all five. The highest drawings of others it is measured against are clerics with a black
turban, a white beard and glasses: the cut sits on one of them.

lines5 (`lottie_trace.py`'s stroked style, never trained on) stays weak, at 2 of 40: a style nobody fitted to.

`embed` runs SigLIP 2 through transformers on the GPU (the ONNX tower would take hours on this CPU for 70 000
frames) and checks it against `vision.onnx` on a sample; `FDV1` is u32 count, u32 width, then per person a u16
name length, the table's English name, f32 bias, f32 cut and `width` f32 weights.
"""

from __future__ import annotations

import hashlib
import pathlib
import pickle
import struct
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent.parent
FACES = pathlib.Path.home() / ".cache/faces"
RAW = [FACES / f"raw_train_{style}" for style in ("flat10", "cartoon6", "poster8")]
TESTS = {style: FACES / f"raw_test_{style}" for style in ("flat10", "cartoon6", "poster8", "lines5")}
EMBEDDED = FACES / "raw_vision.npz"
REGION_FACES = FACES / "region_khamenei.pkl"
OUT = HERE / "assets/face_drawn_vision.bin"
PEOPLE = ["ali khamenei"]
# Negatives whose largest face is this near the person's centre may be the person, unlabelled: a young Khamenei
# was filed under nobody in particular. Left out of the negatives, not counted as the person either.
SCREEN = 0.40
C = 10.0
POOL = 20000

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))


def _region_worker(path):
    import os

    import facelock_people as F

    if "pipe" not in F._WORKER:
        os.environ.setdefault("OMP_NUM_THREADS", "2")
        F._WORKER["pipe"] = F._pipeline()
    from PIL import Image

    try:
        im = Image.open(pathlib.Path.home() / ".cache" / path).convert("RGB")
    except Exception:
        return path, None
    if max(im.size) > 1280:
        s = 1280 / max(im.size)
        im = im.resize((round(im.width * s), round(im.height * s)))
    return path, np.array([p for _, p in F._prints(np.asarray(im))], np.float32).reshape(-1, 128)


def embed() -> None:
    import glob
    from concurrent.futures import ProcessPoolExecutor

    import concept_data
    import facelock_people as F
    import torch
    from PIL import Image
    from transformers import AutoModel

    if not REGION_FACES.exists():
        paths = list(np.load(concept_data.ROOT / "region_base384.npz")["path"].astype(str))
        with ProcessPoolExecutor(8) as pool:
            out = dict(pool.map(_region_worker, paths, chunksize=8))
        found, _ = F.dataset()
        centre = found["ali khamenei"][0]
        near = {p: (float((v @ centre).max()) if v is not None and len(v) else -1.0) for p, v in out.items()}
        pickle.dump(near, open(REGION_FACES, "wb"))
        print(f"{len(near)} region pictures, {sum(v >= SCREEN for v in near.values())} with a face near Khamenei's")
    files = sorted(sum((glob.glob(str(d / "*.png")) for d in RAW + list(TESTS.values())), []))
    model = AutoModel.from_pretrained("google/siglip2-base-patch16-384", dtype=torch.float32).cuda().eval()

    def load(p):
        im = Image.open(p).convert("RGB").resize((384, 384), Image.Resampling.BILINEAR)
        return np.asarray(im, np.float32).transpose(2, 0, 1) / 127.5 - 1.0

    out = np.zeros((len(files), 768), np.float32)
    with torch.no_grad():
        for at in range(0, len(files), 128):
            x = torch.from_numpy(np.stack([load(p) for p in files[at:at + 128]])).cuda()
            f = model.get_image_features(pixel_values=x)
            f = f.pooler_output if hasattr(f, "pooler_output") else f
            out[at:at + 128] = torch.nn.functional.normalize(f.float(), dim=-1).cpu().numpy()
    import onnxruntime as ort
    from evaluate_imgfilter import image_vector

    session = ort.InferenceSession(str(HERE / "target/release/vision.onnx"), providers=["CPUExecutionProvider"])
    sample = np.random.default_rng(0).choice(len(files), 16, replace=False)
    agree = min(float(image_vector(session, pathlib.Path(files[i])) @ out[i]) for i in sample)
    np.savez(EMBEDDED, files=np.array(files), emb=out)
    print(f"{len(files)} frames into {EMBEDDED}; against vision.onnx on 16 of them, cosine {agree:.4f} at the least")


def _data():
    import concept_data
    import facelock_people as F
    from imgfilter_concepts import load_bin

    z = np.load(EMBEDDED)
    files, emb = z["files"].astype(str), z["emb"]
    by = {}
    for i, f in enumerate(files):
        folder, name = f.split("/")[-2], f.split("/")[-1][:-4]
        by.setdefault((folder, name.rsplit("_", 1)[0]), []).append(i)
    data = pickle.load(open(F.EMBEDDED, "rb"))
    found, everyday = F.dataset()
    unit = lambda x: x / np.linalg.norm(x, axis=-1, keepdims=True)  # noqa: E731
    pool = unit(load_bin("imgfilter_pool.bin", b"IFP1"))
    pool = pool[np.random.default_rng(0).choice(len(pool), min(POOL, len(pool)), replace=False)]
    near = pickle.load(open(REGION_FACES, "rb"))
    rz = np.load(concept_data.ROOT / "region_base384.npz")
    region = unit(rz["embeddings"].astype(np.float32))[np.array([near.get(p, -1.0) < SCREEN for p in rz["path"].astype(str)])]
    bank_source = unit(np.load(concept_data.ROOT / "emb_base384.npz")["embeddings"].astype(np.float32))
    return files, emb, by, data, found, everyday, pool, region, bank_source


def fit() -> None:
    import facelock_people as F
    from sklearn.linear_model import LogisticRegression

    files, emb, by, data, found, everyday, pool, region, bank_source = _data()
    sha = lambda owner, path: hashlib.sha1(f"{owner}|{path}".encode()).hexdigest()[:16]  # noqa: E731
    blob = bytearray(b"FDV1" + struct.pack("<II", len(PEOPLE), emb.shape[1]))
    for person in PEOPLE:
        centre = found[person][0]
        stickers = []    # (frame indices, is the person, fold, who)
        for name, (_, rows) in found.items():
            for path, views, _ in rows:
                faces = data[(name, path)]["photo"]
                if not np.allclose(faces[0], views["photo"]):
                    continue    # the person is not the largest face, and the sticker is cut around the largest
                if name != person and float((faces @ centre).max()) >= SCREEN:
                    continue
                for raw in RAW:
                    idx = by.get((raw.name, sha(name, path)))
                    if idx:
                        stickers.append((np.array(idx), name == person, F._split(path, 5), name))
        for path, o in everyday:
            if len(o["boxes"]) and float((o["photo"] @ centre).max()) < SCREEN:
                for raw in RAW:
                    idx = by.get((raw.name, sha("", path)))
                    if idx:
                        stickers.append((np.array(idx), False, F._split(path, 5), "nobody"))

        def direction(folds):
            chosen = [s for s in stickers if s[2] in folds]
            x = np.concatenate([emb[s[0]] for s in chosen] + [pool])
            y = np.concatenate([np.full(len(s[0]), s[1]) for s in chosen] + [np.zeros(len(pool), bool)])
            clf = LogisticRegression(C=C, class_weight="balanced", max_iter=5000).fit(x, y)
            return clf.coef_[0].astype(np.float32), float(clf.intercept_[0])

        oof = np.full(len(files), np.nan, np.float32)
        pictures = []
        for fold in range(5):
            w, b = direction({f for f in range(5) if f != fold})
            for s in stickers:
                if s[2] == fold:
                    oof[s[0]] = emb[s[0]] @ w + b
            pictures.append(max(float((region @ w + b).max()), float((bank_source @ w + b).max())))
        best = lambda s: float(np.nanmax(oof[s[0]]))  # noqa: E731
        negatives = sorted(((best(s), s[3]) for s in stickers if not s[1]), reverse=True)
        mine = np.array([best(s) for s in stickers if s[1]])
        cut = max(negatives[0][0], max(pictures))
        print(f"{person}: cut {cut:.2f} over {len(negatives)} out-of-fold drawings of others (highest: "
              + ", ".join(f"{who} {v:.2f}" for v, who in negatives[:4]) + f") and pictures ({max(pictures):.2f}); "
              f"out-of-fold drawings of {person} over it {np.mean(mine >= cut):.2f} of {len(mine)}")
        if person == "ali khamenei" and all(d.exists() for d in TESTS.values()):
            w, b = direction({0, 1, 2})    # the test sets are cut from photographs of folds 3 and 4
            line = []
            for style, d in TESTS.items():
                counts = {}
                for (folder, stem), idx in by.items():
                    if folder == d.name:
                        label = stem.split("__")[0]
                        hit = float((emb[idx] @ w + b).max()) >= cut
                        taken, seen = counts.get(label, (0, 0))
                        counts[label] = (taken + hit, seen + 1)
                line.append(f"{style} " + " ".join(f"{k} {v[0]}/{v[1]}" for k, v in sorted(counts.items())))
            print("  the test sets, by the direction alone (fitted on folds 0-2): " + "; ".join(line))
        w, b = direction(set(range(5)))
        name = person.encode()
        blob += struct.pack("<H", len(name)) + name + struct.pack("<ff", b, cut) + w.astype("<f4").tobytes()
    OUT.write_bytes(bytes(blob))
    print(f"wrote {OUT}: {len(PEOPLE)} people, {len(blob)} bytes")


if __name__ == "__main__":
    {"embed": embed, "fit": fit}[sys.argv[1]]()
