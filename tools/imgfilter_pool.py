#!/usr/bin/env python3
"""The general picture pool a «فیلتر تصویری» phrase learns from, and the bank it learns against.

    ../bin/python tools/imgfilter_pool.py fetch      # random hosted Open Images train pictures
    ../bin/python tools/imgfilter_pool.py manifest   # embed_manifest for vision_embed.py
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/vision_embed.py --manifest ~/.cache/concepts/pool_manifest.tsv \
        --root ~/.cache --hf google/siglip2-base-patch16-384 --out ~/.cache/concepts/pool_base384.npz --resume
    ../bin/python tools/imgfilter_pool.py write      # assets/imgfilter_pool.bin

A phrase filter used to be the phrase's text direction and nothing else. A text direction is a
poor picture classifier — measured on Persian phrases with verified Open Images pictures it caught
0.58 of them — and the fix (`imgfilter::learn`) is to fit a small logistic head when the filter is
made: the pictures in this pool the phrase scores highest are its positives, the everyday bank
(`imgfilter_bank.py`) its negatives. That needs a pool that has *something* near whatever an admin
types, so it is a uniform sample of Open Images — twenty thousand everyday classes — not the
concept corpus, which covers a hundred.

Pictures whose id hashes into the concept corpus's evaluation quarter are never sampled, so the
pool cannot leak into `imgfilter_phrases.py`'s numbers.
"""

from __future__ import annotations

import hashlib
import pathlib
import random
import struct
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import concept_data  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent
POOL_DIR = concept_data.IMG / "pool"
WANT = 30_000
OUT = HERE / "assets/imgfilter_pool.bin"


def fetch(threads: int = 48) -> None:
    hosted = (concept_data.OI / "train-hosted.txt").read_text().split()
    rng = random.Random(20261014)
    rng.shuffle(hosted)
    chosen = [i for i in hosted if concept_data.split_of(i, "train") == "train"][: int(WANT * 1.05)]
    POOL_DIR.mkdir(parents=True, exist_ok=True)

    def one(image: str) -> bool:
        out = POOL_DIR / f"{image}.jpg"
        if out.exists():
            return True
        try:
            with urllib.request.urlopen(f"{concept_data.PIXELS}/train/{image}.jpg", timeout=30) as r:
                jpeg = concept_data.as_jpeg(r.read())
        except Exception:  # noqa: BLE001
            return False
        if jpeg is None:
            return False
        out.write_bytes(jpeg)
        return True

    with ThreadPoolExecutor(threads) as pool:
        ok = sum(pool.map(one, chosen))
    print(f"pool: {ok} of {len(chosen)} pictures")


def manifest() -> None:
    rows = ["path\tgroup\tsource\tsplit"]
    for path in sorted(POOL_DIR.glob("*.jpg")):
        rows.append(f"concepts/img/pool/{path.name}\tpool\toi\ttrain")
    (concept_data.ROOT / "pool_manifest.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    print(f"{len(rows) - 1} rows")


def write() -> None:
    emb = np.load(concept_data.ROOT / "pool_base384.npz", allow_pickle=False)
    vectors = emb["embeddings"].astype(np.float32)
    vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
    out = bytearray(b"IFP1" + struct.pack("<II", len(vectors), vectors.shape[1]))
    for vector in vectors:
        scale = float(np.abs(vector).max()) / 127.0
        out += struct.pack("<f", scale) + np.clip(np.round(vector / scale), -127, 127).astype(np.int8).tobytes()
    OUT.write_bytes(bytes(out))
    print(f"wrote {OUT}: {len(vectors)} pictures, {len(out) / 1e6:.1f} MB")


if __name__ == "__main__":
    {"fetch": fetch, "manifest": manifest, "write": write}[sys.argv[1]]()
