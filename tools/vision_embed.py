#!/usr/bin/env python3
"""Embeds a manifest of pictures through the general model, the way `vision::embed_of` does.

    ../bin/python tools/vision_embed.py --manifest ~/.cache/nsfw-train/corpus/manifest.tsv \
        --onnx target/release/vision.onnx --out ~/.cache/nsfw-train/emb/corpus_base224.npz
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/vision_embed.py --manifest ... \
        --hf google/siglip2-base-patch16-384 --out ~/.cache/nsfw-train/emb/corpus_base384.npz

Two backends, one transform. `--onnx` runs the exported file on the CPU and is the number the
bot will actually compute; `--hf` runs the checkpoint through `transformers` (on the GPU when
there is one) and exists because five towers over fifteen thousand pictures is an afternoon on
a CPU and minutes on a card. Both go through `pixels_of` below, which is `vision::view` —
**the whole frame squashed to the model's square, not centre-cropped, scaled `/127.5 - 1`** —
and neither uses the checkpoint's own image processor, so the two backends cannot drift from
each other or from the Rust.

The output is one `.npz`: `embeddings` (unit length, float32), `path`, `group`, `split`,
`index` (row number in the manifest), `side`, `tower`. `--resume` keeps the rows an existing
output already has and embeds only the new ones, matched by path — a corpus grows, its
embeddings do not have to be recomputed. `anime_nsfw/` paths in an old file are read as
`anime_scenes/`, the rename `nsfw_data.py` made.
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import time

import numpy as np
from PIL import Image

SCALE = 127.5


def read_manifest(path: pathlib.Path) -> list[tuple[str, str, str]]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 4:
            continue
        rows.append((cols[0], cols[1], cols[3]))
    return rows


def pixels_of(image: Image.Image, side: int) -> np.ndarray:
    """`vision::view` + `nsfw::pixels_of`: squash to `side`x`side`, planar RGB, `-1..=1`."""
    image = image.convert("RGB").resize((side, side), Image.Resampling.BILINEAR)
    arr = np.asarray(image, dtype=np.float32) / SCALE - 1.0
    return arr.transpose(2, 0, 1)


class Onnx:
    def __init__(self, path: str, threads: int):
        import onnxruntime as ort

        opts = ort.SessionOptions()
        opts.intra_op_num_threads = threads
        self.session = ort.InferenceSession(path, opts, providers=["CPUExecutionProvider"])
        inp = self.session.get_inputs()[0]
        self.input = inp.name
        self.side = int(inp.shape[2]) if isinstance(inp.shape[2], int) and inp.shape[2] > 0 else 224
        self.name = pathlib.Path(path).resolve().parent.name + "/" + pathlib.Path(path).name

    def __call__(self, batch: np.ndarray) -> np.ndarray:
        return self.session.run(None, {self.input: batch})[0]


class Hf:
    def __init__(self, model: str):
        import torch
        from transformers import AutoModel

        self.torch = torch
        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        self.model = AutoModel.from_pretrained(model).eval().to(self.device)
        self.side = int(self.model.config.vision_config.image_size)
        self.name = model

    def __call__(self, batch: np.ndarray) -> np.ndarray:
        torch = self.torch
        with torch.no_grad():
            x = torch.from_numpy(batch).to(self.device)
            out = self.model.get_image_features(pixel_values=x)
            out = out.pooler_output if hasattr(out, "pooler_output") else out
            return out.float().cpu().numpy()


def unit(x: np.ndarray) -> np.ndarray:
    n = np.linalg.norm(x, axis=-1, keepdims=True)
    n[n == 0] = 1
    return x / n


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--root", help="directory the manifest paths are relative to (default: beside it)")
    backend = parser.add_mutually_exclusive_group(required=True)
    backend.add_argument("--onnx")
    backend.add_argument("--hf")
    parser.add_argument("--out", required=True)
    parser.add_argument("--batch", type=int, default=16)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()

    manifest = pathlib.Path(args.manifest)
    root = pathlib.Path(args.root) if args.root else manifest.parent
    rows = read_manifest(manifest)
    if args.limit:
        rows = rows[: args.limit]
    model = Onnx(args.onnx, args.threads) if args.onnx else Hf(args.hf)
    side = model.side

    done: dict[str, np.ndarray] = {}
    out = pathlib.Path(args.out)
    if args.resume and out.exists():
        old = np.load(out, allow_pickle=False)
        if int(old["side"]) == side:
            for p, e in zip(old["path"], old["embeddings"]):
                done[str(p).replace("anime_nsfw/", "anime_scenes/")] = e
            print(f"resume: {len(done)} rows already embedded", flush=True)
        else:
            print(f"resume: {out} is {int(old['side'])}px, not {side}px; starting over", flush=True)

    todo = [(i, r) for i, r in enumerate(rows) if r[0] not in done]
    print(f"{len(rows)} rows, {len(todo)} to embed at {side}px through {model.name}", flush=True)
    embeddings: dict[str, np.ndarray] = dict(done)
    t = time.time()
    for at in range(0, len(todo), args.batch):
        chunk = todo[at : at + args.batch]
        pixels, keep = [], []
        for _, (p, _, _) in chunk:
            try:
                with Image.open(root / p) as im:
                    pixels.append(pixels_of(im, side))
                keep.append(p)
            except Exception as e:  # noqa: BLE001
                print(f"  unreadable {p}: {e}", flush=True)
        if not pixels:
            continue
        got = unit(model(np.stack(pixels).astype(np.float32)))
        for p, e in zip(keep, got):
            embeddings[p] = e.astype(np.float32)
        if (at // args.batch) % 20 == 0:
            rate = (at + len(chunk)) / max(time.time() - t, 1e-6)
            print(f"  {at + len(chunk)}/{len(todo)} {rate:.1f}/s", flush=True)

    order = [(i, r) for i, r in enumerate(rows) if r[0] in embeddings]
    np.savez(
        out,
        embeddings=np.stack([embeddings[r[0]] for _, r in order]),
        index=np.array([i for i, _ in order], dtype=np.int64),
        path=np.array([r[0] for _, r in order]),
        group=np.array([r[1] for _, r in order]),
        split=np.array([r[2] for _, r in order]),
        side=np.array(side),
        tower=np.array(model.name),
    )
    print(f"wrote {out}: {len(order)} rows in {time.time() - t:.0f}s", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
