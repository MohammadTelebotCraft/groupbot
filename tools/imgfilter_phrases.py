#!/usr/bin/env python3
"""«فیلتر تصویری» phrase filters, measured on Persian phrases against human-verified pictures.

    ../bin/python tools/imgfilter_phrases.py --vision-files target/release

`evaluate_imgfilter.py` measures the phrase path on ImageNette's ten English class names. A group
types Persian, and what it names is rarely a dog breed: the live filters on 2026-10-14 were a
public figure, two video games and two vulgar words. This takes the concept corpus
(`concept_data.py`) — Open Images pictures with human-verified labels for some hundred everyday
classes, beside the NSFW corpus's ordinary groups — and asks, for each Persian phrase below, the
two questions that decide whether a filter is usable:

    recall      of the pictures verified to show the thing, how many clear the cut
    everyday    of what a group ordinarily sends (people, food, memes, stickers, screenshots,
                anime, emoji), how many clear it — each one a deleted ordinary picture

The direction is built exactly the way `imgtext::ensemble` builds it — every template run through
the text tower, the raw outputs summed, then normalised — and scored the way `imgfilter::margin`
scores it, against `BACKGROUND`.
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import sys

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import concept_data  # noqa: E402
from evaluate_imgfilter import TEMPLATES, Bpe, background_vector  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent

# Persian phrase -> the Open Images classes a picture must be verified for to count as a match.
PHRASES = {
    "سیگار": ["Cigarette"], "سیگار برگ": ["Cigar"], "آبجو": ["Beer"], "شراب": ["Wine"],
    "ویسکی": ["Whisky"], "کوکتل": ["Cocktail"], "شامپاین": ["Champagne"], "تفنگ": ["Rifle"],
    "تپانچه": ["Handgun"], "چاقو": ["Knife"], "کازینو": ["Casino"], "پوکر": ["Poker"],
    "تاس": ["Dice"], "شطرنج": ["Chess"], "شمع": ["Candle"], "سس گوجه": ["Ketchup"],
    "توت فرنگی": ["Strawberry"], "گوجه فرنگی": ["Tomato"], "رژ لب": ["Lipstick"],
    "قیچی": ["Scissors"], "اره برقی": ["Chainsaw"], "شمشیر": ["Sword"], "چنگال": ["Fork"],
    "قاشق": ["Spoon"], "چای": ["Tea"], "آب میوه": ["Juice"], "نوشابه": ["Soft drink", "Cola"],
    "اسفناج": ["Spinach"], "گچ": ["Chalk"], "مداد": ["Pencil"], "خودکار": ["Pen"],
    "قرص": ["Pill"], "سرنگ": ["Syringe"], "خون": ["Blood"], "ماری جوانا": ["Cannabis"],
    "آتش": ["Fire"], "مه": ["Fog"], "اسباب بازی": ["Toy"], "پیچ گوشتی": ["Screwdriver"],
    "گلوله": ["Bullet", "Ammunition"], "اسب دوانی": ["Horse racing"], "ورق بازی": ["Playing card"],
    "قلیان": [], "بطری": ["Bottle"],
}
EVERYDAY = set(concept_data.CORPUS_NEGATIVE)


def text_direction(session, bpe: Bpe, phrase: str) -> np.ndarray:
    name = session.get_inputs()[0].name
    total = np.zeros(768, dtype=np.float64)
    for template in TEMPLATES:
        out = session.run(None, {name: bpe.encode(template.format(phrase))})[0]
        total += np.asarray(out, dtype=np.float64).reshape(-1)[:768]
    return (total / np.linalg.norm(total)).astype(np.float32)


def load():
    emb = np.load(concept_data.ROOT / "emb_base384.npz", allow_pickle=False)
    paths = emb["path"].astype(str)
    x = emb["embeddings"].astype(np.float32)
    _, verified = concept_data.read_labels()
    classes = []
    everyday = np.zeros(len(paths), dtype=bool)
    for i, p in enumerate(paths):
        if p.startswith("concepts/img/") and p.split("/")[2] in verified:
            table = verified[p.split("/")[2]]
            classes.append({n for n, v in table.get(pathlib.Path(p).stem, {}).items() if v == 1.0})
        else:
            classes.append(set())
            group = p.split("/")[2] if p.startswith("nsfw-train/") else ""
            everyday[i] = group in EVERYDAY
    return paths, x, classes, everyday


def main() -> int:
    import onnxruntime as ort

    parser = argparse.ArgumentParser()
    parser.add_argument("--vision-files", type=pathlib.Path, default=HERE / "target/release")
    parser.add_argument("--cut", type=float, default=0.038)
    args = parser.parse_args()
    session = ort.InferenceSession(str(args.vision_files / "vision_text.onnx"), providers=["CPUExecutionProvider"])
    bpe = Bpe(args.vision_files / "vision_text_vocab.txt", args.vision_files / "vision_text_merges.txt")
    background = background_vector()
    paths, x, classes, everyday = load()
    bank = everyday & np.array([hashlib.sha1(p.encode()).hexdigest()[0] in "01234567" for p in paths])
    held_everyday = everyday & ~bank
    base = x @ background
    print(f"{len(paths)} pictures; everyday bank {int(bank.sum())}, everyday held {int(held_everyday.sum())}")
    rows = []
    for phrase, wanted in PHRASES.items():
        v = text_direction(session, bpe, phrase)
        margin = x @ v - base
        pos = np.array([bool(c & set(wanted)) for c in classes]) if wanted else np.zeros(len(x), bool)
        others = np.array([bool(c) and not (c & set(wanted)) for c in classes])
        fixed = margin >= args.cut
        rec = fixed[pos].mean() if pos.any() else float("nan")
        ev_fp = int(fixed[held_everyday].sum())
        oth_fp = fixed[others].mean()
        q = np.quantile(margin[bank], [0.99, 0.999])
        rows.append((phrase, wanted, int(pos.sum()), rec, ev_fp, oth_fp, q, margin, pos, others))
        print(f"  {phrase:12} {','.join(wanted)[:22]:22} n={int(pos.sum()):4d} recall@{args.cut} {rec:.3f}  "
              f"everyday FP {ev_fp:3d}/{int(held_everyday.sum())}  other-class FP {oth_fp:.3f}  "
              f"bank q99 {q[0]:+.4f} q99.9 {q[1]:+.4f}")
    np.savez(concept_data.ROOT / "phrase_margins.npz",
             phrases=np.array([r[0] for r in rows]),
             margins=np.stack([r[7] for r in rows]), pos=np.stack([r[8] for r in rows]),
             others=np.stack([r[9] for r in rows]), bank=bank, held=held_everyday)
    return 0


if __name__ == "__main__":
    sys.exit(main())
