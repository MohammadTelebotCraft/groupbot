#!/usr/bin/env python3
"""The object vocabulary a picture is asked "what is this?" against before a phrase filter may delete it
(`imgfilter::Gate`): Open Images' 20 931 class names, lowercased, through the general model's text tower exactly as
`imgtext` frames an English phrase (the six templates summed, unit length).

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/imgfilter_vocab.py embed   # the GPU text tower, ~3 minutes
    ../bin/python tools/imgfilter_vocab.py write                             # assets/imgfilter_vocab.{bin,tsv}

`embed` runs the same SigLIP 2 weights through transformers on the GPU: the ONNX tower takes some three hours
for twenty thousand names on this CPU. Checked against `vision_text.onnx` on a sample of names: cosine 0.9986
at the least, 0.9995 on average.

`IFV1`, u32 count, u32 width, then per name an f32 scale and `width` int8 values (`imgfilter_bank.bin`'s layout);
the `.tsv` maps every table English with an Open Images class to its row.

Why it exists, measured on 150 phrases held out, every ordinary picture the filters took looked at by eye: the
trained filters' cut (`imgfilter_concepts.py`, LOOKALIKE_SHARE and MARGIN_SD 0.5) still took 23 pictures, 2 of
them wrong; asking that the picture rank the filter's own subject among its 25 best names — names within 0.8 of
the subject's, with the vocabulary's mean taken out of both, are the subject and skipped — at MARGIN_SD 0.25 took
25, none wrong, for recall 0.364 -> 0.326 and production-reservoir hits 2 -> 1. Learned phrase filters
(`LEARNED_MARGIN` 1.25): 15 hits, 1 wrong -> among the best hundred, 14 hits, none wrong, recall 0.319 -> 0.312,
reservoir hits 0. Without the mean taken out, every name leans the same way: "dog" and "cat" at 0.94, and 2 647
names within 0.85 of "dog".
"""

from __future__ import annotations

import csv
import pathlib
import struct
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent.parent
ROOT = pathlib.Path.home() / ".cache/concepts"
CACHE = ROOT / "vocab_direction.npz"
OUT = HERE / "assets/imgfilter_vocab.bin"
TABLE = HERE / "assets/imgfilter_vocab.tsv"
EN_T = ["{}", "a photo of {}", "a photograph of {}", "a picture of {}", "an image of {}", "a close-up photo of {}"]
CANONICAL = {"english springer": "english springer spaniel"}  # imgtext::canonical_phrase


def names() -> list[str]:
    rows = list(csv.reader(open(ROOT / "oi/oidv7-class-descriptions.csv", encoding="utf-8")))[1:]
    return sorted({" ".join(name.lower().split()) for _, name in rows})


def embed() -> None:
    import torch
    from transformers import AutoModel, AutoTokenizer

    vocabulary = names()
    tok = AutoTokenizer.from_pretrained("google/siglip2-base-patch16-384")
    model = AutoModel.from_pretrained("google/siglip2-base-patch16-384", dtype=torch.float32).cuda().eval()
    out = []
    for at in range(0, len(vocabulary), 2048):
        phrases = vocabulary[at:at + 2048]
        texts = [t.replace("{}", CANONICAL.get(p, p)) for p in phrases for t in EN_T]
        parts = []
        for i in range(0, len(texts), 768):
            enc = tok([s.lower() for s in texts[i:i + 768]], padding="max_length", max_length=64, truncation=True,
                      return_tensors="pt")
            with torch.no_grad():
                f = model.get_text_features(input_ids=enc["input_ids"].cuda())
            f = f.pooler_output if hasattr(f, "pooler_output") else f
            parts.append(f.float().cpu().numpy())
        e = np.concatenate(parts).reshape(len(phrases), len(EN_T), -1).sum(1)
        out.append(e / np.linalg.norm(e, axis=1, keepdims=True))
    np.savez(CACHE, names=np.array(vocabulary), vectors=np.concatenate(out).astype(np.float32))
    print(f"embedded {len(vocabulary)} names into {CACHE}")


def write() -> None:
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    import imgfilter_concepts as C

    z = np.load(CACHE)
    vocabulary = [str(n) for n in z["names"]]
    vectors = z["vectors"].astype(np.float32)
    blob = bytearray(b"IFV1" + struct.pack("<II", len(vocabulary), vectors.shape[1]))
    for v in vectors:
        q, scale = C.quantize(v)
        blob += struct.pack("<f", scale) + q.tobytes()
    OUT.write_bytes(bytes(blob))
    row = {n: at for at, n in enumerate(vocabulary)}
    by_name, name_of = {}, {}
    for mid, name in list(csv.reader(open(C.OI / "oidv7-class-descriptions.csv", encoding="utf-8")))[1:]:
        key = " ".join(name.lower().split())
        by_name.setdefault(key, mid)
        name_of[mid] = key
    lines, seen = [], set()
    for english, _ in C.table_rows():
        mid = C.class_of(english, by_name)
        if english not in seen and mid and name_of.get(mid) in row:
            lines.append(f"{english}\t{row[name_of[mid]]}")
            seen.add(english)
    TABLE.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {OUT} ({len(blob)} bytes) and {TABLE} ({len(lines)} Englishes)")


if __name__ == "__main__":
    {"embed": embed, "write": write}[sys.argv[1]]()
