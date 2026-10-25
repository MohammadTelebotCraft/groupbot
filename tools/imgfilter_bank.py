#!/usr/bin/env python3
"""Writes `assets/imgfilter_bank.bin`: the everyday pictures every «فیلتر تصویری» is calibrated on.

    ../bin/python tools/imgfilter_bank.py

A group's own filter — a phrase or a few example photos — used to be compared with one fixed
number: 0.038 for a phrase, 0.62 for examples. Measured on Persian phrases against
human-verified Open Images pictures (`tools/imgfilter_phrases.py`), one number is wrong in both
directions at once: it caught 98% of «شطرنج» and 0% of «گچ», and «آتش», «مه», «تفنگ» and «شمشیر»
each deleted 20-29 of 6 539 everyday pictures. For examples it was worse and got worse with use —
an average of several photos is a more generic direction, so at 0.62 one example deleted 8
everyday pictures in 6 539, three deleted 42 and eight deleted 105, along with 27% of unrelated
photographs.

So each filter's cut is now measured on the filter itself: its margin over a fixed bank of
everyday pictures — stickers, memes, screenshots, people, food, anime, emoji, the NSFW corpus's
ordinary groups — and the cut is where one picture in a thousand of them would be deleted. The
bank is this file: unit embeddings from the general model, int8 with one scale per row.

Format: `IFB1`, u32 count, u32 width, then per row an f32 scale and `width` int8 values, little
endian. Ordinary Danbooru drawings are capped so they do not outvote the other groups.

**Since 2026-10-15 it also holds the region's own pictures** (`imgfilter_region.py`): Iranian
people, streets, food and religious life, the officials and clerics who fill Iranian news, and
chat screenshots and captioned photographs in Persian script. Without them a phrase whose
direction leaned towards "a Middle Eastern man" was calibrated against nobody who looks like one,
which is how the live filters came to flag 42 of 630 held-out pictures of that kind; with them in
the bank, 1. Run `imgfilter_region.py` first — this writes the bank without them if they are
missing, and says so.
"""

from __future__ import annotations

import collections
import hashlib
import pathlib
import struct
import sys

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import concept_data  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent
OUT = HERE / "assets/imgfilter_bank.bin"
CAP = {"drawn_safe": 1500}


def main() -> int:
    emb = np.load(concept_data.ROOT / "emb_base384.npz", allow_pickle=False)
    rows = []
    for path, vector in zip(emb["path"].astype(str), emb["embeddings"]):
        parts = path.split("/")
        if len(parts) < 4 or parts[0] != "nsfw-train" or parts[2] not in concept_data.CORPUS_NEGATIVE:
            continue
        rows.append((parts[2], hashlib.sha1(path.encode()).hexdigest(), vector))
    region = concept_data.ROOT / "region_base384.npz"
    if region.exists():
        extra = np.load(region, allow_pickle=False)
        for path, vector in zip(extra["path"].astype(str), extra["embeddings"]):
            parts = path.split("/")
            rows.append((f"region:{parts[-3]}/{parts[-2]}" if len(parts) >= 3 else "region",
                         hashlib.sha1(path.encode()).hexdigest(), vector))
    else:
        print(f"no {region}: the bank is written without the region's pictures")
    rows.sort(key=lambda r: (r[0], r[1]))
    kept, count = [], {}
    for group, _, vector in rows:
        if count.get(group, 0) >= CAP.get(group, 1 << 30):
            continue
        count[group] = count.get(group, 0) + 1
        kept.append(vector / np.linalg.norm(vector))
    width = len(kept[0])
    out = bytearray(b"IFB1" + struct.pack("<II", len(kept), width))
    worst = 0.0
    for vector in kept:
        scale = float(np.abs(vector).max()) / 127.0
        q = np.clip(np.round(vector / scale), -127, 127).astype(np.int8)
        worst = max(worst, float(np.abs(q.astype(np.float32) * scale - vector).max()))
        out += struct.pack("<f", scale) + q.tobytes()
    OUT.write_bytes(bytes(out))
    print(f"wrote {OUT}: {len(kept)} pictures, {len(out) / 1e6:.1f} MB, worst component error {worst:.5f}")
    groups = collections.Counter()
    for g, n in count.items():
        groups[g.split("/")[0] if g.startswith("region:") else g] += n
    print("  " + ", ".join(f"{g} {n}" for g, n in sorted(groups.items())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
