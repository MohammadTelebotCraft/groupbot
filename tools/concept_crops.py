#!/usr/bin/env python3
"""Does a crop rescue still earn its passes once the concept locks are fitted heads?

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/concept_crops.py --heads /tmp/heads.npz

The prompt-margin locks had three: two halves for a gun beside a bigger allowed object, three
squares for a small gambling subject, three strips to confirm blood against red wine. Each one
costs two or three more passes of the general model on the pictures that reach it. This takes
the evaluation pictures whose whole-frame logit sits in a band under the head's cut, embeds the
same crops, and prints what raising the score to the best crop would catch and what it would
wrongly delete — the measurement that decides whether `concepts.rs` keeps a rescue at all.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import concept_data  # noqa: E402
import train_concept_heads as tch  # noqa: E402
from vision_embed import Hf, pixels_of, unit  # noqa: E402

CACHE = pathlib.Path.home() / ".cache"


def halves(im: Image.Image):
    w, h = im.size
    if w >= h:
        return [im.crop((0, 0, w // 2, h)), im.crop((w // 2, 0, w, h))]
    return [im.crop((0, 0, w, h // 2)), im.crop((0, h // 2, w, h))]


def squares(im: Image.Image):
    w, h = im.size
    side = round(0.7 * min(w, h))
    long = max(w, h) - side
    short = (min(w, h) - side) // 2
    out = []
    for at in (0, long // 2, long):
        box = (at, short, at + side, short + side) if w >= h else (short, at, short + side, at + side)
        out.append(im.crop(box))
    return out


def strips(im: Image.Image):
    w, h = im.size
    if w >= h:
        cuts = [0, w // 3, w * 2 // 3, w]
        return [im.crop((cuts[i], 0, cuts[i + 1], h)) for i in range(3)]
    cuts = [0, h // 3, h * 2 // 3, h]
    return [im.crop((0, cuts[i], w, cuts[i + 1])) for i in range(3)]


RESCUES = {"c_gun": halves, "c_bet": squares, "c_bld": strips, "c_cig": halves, "c_alc": halves, "c_drg": halves}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--heads", required=True)
    parser.add_argument("--band", type=float, default=2.0, help="logits under the cut that earn crops")
    args = parser.parse_args()
    heads = np.load(args.heads)
    paths, x, split, source, y, kind, dev, tower = tch.load()
    model = Hf("google/siglip2-base-patch16-384")
    ev = split == "eval"
    for s, slot in enumerate(concept_data.SLOTS):
        w, b, cut = heads[f"{slot}_w"], float(heads[f"{slot}_b"]), float(heads[f"{slot}_cut"])
        ys = y[:, s]
        rows = np.where(ev & (ys >= 0))[0]
        z = x[rows] @ w + b
        band = (z < cut) & (z >= cut - args.band)
        todo = rows[band]
        best = {}
        for at in range(0, len(todo), 16):
            chunk = todo[at:at + 16]
            pixels, owner = [], []
            for i in chunk:
                with Image.open(CACHE / paths[i]) as im:
                    for crop in RESCUES[slot](im.convert("RGB")):
                        pixels.append(pixels_of(crop, model.side))
                        owner.append(i)
            got = unit(model(np.stack(pixels).astype(np.float32)))
            for i, e in zip(owner, got):
                best[i] = max(best.get(i, -1e9), float(e @ w + b))
        rescued = np.array([best.get(i, -1e9) >= cut for i in rows])
        full = z >= cut
        after = full | (band & rescued)
        pos, neg = ys[rows] == 1, ys[rows] == 0
        everyday = tch.everyday_of(kind[rows, s])
        print(f"{slot}: band {int(band.sum())} pictures ({int((band & pos).sum())} positive), "
              f"crops {len(todo) * len(RESCUES[slot](Image.new('RGB', (8, 8))))}")
        print(f"   recall {full[pos].mean():.3f} -> {after[pos].mean():.3f}   "
              f"FP everyday {int(full[neg & everyday].sum())} -> {int(after[neg & everyday].sum())} of {int((neg & everyday).sum())}   "
              f"FP hard {int(full[neg & ~everyday].sum())} -> {int(after[neg & ~everyday].sum())} of {int((neg & ~everyday).sum())}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
