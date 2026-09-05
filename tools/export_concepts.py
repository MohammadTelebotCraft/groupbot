#!/usr/bin/env python3
"""Regenerates `src/handlers/concept_vectors.rs` — every fixed direction the bot ships.

Three kinds of vector come out of one text tower, and they are here together because they all
have to be produced by *the same* tower as the pictures they will be scored against. A vector
from anywhere else is scored by a dot product that means nothing, and nothing about that
failure is visible: the filter still runs, still produces numbers, and deletes at random.

* **`BACKGROUND`** — what an ordinary photograph looks like. Every concept score in the bot is
  a *margin* over this, because a contrastive model puts almost every picture at roughly the
  same absolute cosine against almost every phrase; only the difference carries signal.
* **The six shipped concepts** — «سیگار» and its siblings, the ones `concepts::CONCEPTS` names.
* **The NSFW arbiter's three groups** — explicit, suggestive, and ordinary. These are the
  second opinion that stops the small NSFW classifier deleting a portrait, and unlike the two
  above they are *not* used as margins: SigLIP ships a calibrated scale and bias, so a raw
  cosine against these becomes a probability. See `nsfw::arbiter`.

    python3 tools/export_concepts.py --write

Prompts are English and Persian together on purpose. The tower is multilingual and the
picture space is one space, so both pull in the same direction and their average is a steadier
estimate of it than either alone — the same argument `imgtext::TEMPLATES` makes for framing a
single phrase several ways.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

import torch

DIM = 768
MAX_TOKENS = 64

# Ordinary photographs, in the sense of "the sort of thing a group sends all day". The point
# of this set is to be *dull* — it is subtracted from everything, so anything specific in here
# would quietly blind the bot to that one thing.
BACKGROUND = [
    "a photo", "a photograph", "a picture", "a snapshot", "an image",
    "a photo of a person", "a photo of people", "a photo of a place",
    "a photo of an object", "a photo of an animal", "a screenshot",
    "a selfie", "a photo of food", "a photo of a landscape", "a photo of a building",
    "عکس", "یک عکس", "تصویر", "عکس یک نفر", "عکس آدم ها", "عکسی از یک جا",
    "عکس یک شیء", "عکس حیوان", "اسکرین شات", "سلفی", "عکس غذا", "عکس منظره",
]

CONCEPTS: dict[str, list[str]] = {
    "CIGARETTE": [
        "a photo of a cigarette", "a person smoking a cigarette", "a lit cigarette",
        "a pack of cigarettes", "a person smoking a hookah", "a shisha water pipe",
        "cigarette smoke", "a vape pen", "a person vaping", "an ashtray with cigarettes",
        "عکس سیگار", "کسی که سیگار می کشد", "قلیان", "پاکت سیگار", "دود سیگار", "ویپ",
    ],
    "ALCOHOL": [
        "a photo of alcohol", "a bottle of whiskey", "a glass of beer", "a glass of wine",
        "a bottle of vodka", "people drinking alcohol", "a bar with bottles of liquor",
        "a cocktail glass", "a champagne bottle",
        "عکس مشروب الکلی", "بطری ویسکی", "لیوان آبجو", "شراب", "ودکا", "نوشیدنی الکلی",
    ],
    "WEAPON": [
        "a photo of a gun", "a handgun", "a pistol", "an assault rifle", "a shotgun",
        "a person holding a gun", "a knife held as a weapon", "ammunition and bullets",
        "a photo of firearms",
        "عکس اسلحه", "کلت کمری", "تفنگ", "کسی که اسلحه دستش است", "فشنگ و مهمات", "چاقو",
    ],
    "GAMBLING": [
        "a photo of gambling", "a casino", "a roulette table", "poker chips and cards",
        "a slot machine", "an online betting website", "a sports betting slip",
        "a hand of playing cards for gambling", "dice being thrown for money",
        "عکس قمار", "کازینو", "میز رولت", "ژتون پوکر", "سایت شرط بندی", "دستگاه اسلات",
    ],
    "DRUGS": [
        "a photo of illegal drugs", "cocaine powder and lines", "cannabis buds and marijuana",
        "a syringe and heroin", "crystal methamphetamine", "pills of ecstasy",
        "a person rolling a joint", "drug paraphernalia",
        "عکس مواد مخدر", "کوکائین", "ماری جوانا", "سرنگ و هروئین", "شیشه", "قرص اکستازی",
    ],
    "BLOOD": [
        "a photo of blood", "a bleeding wound", "a bloody injury", "blood splatter",
        "a gory scene with blood", "a person covered in blood", "a severe open wound",
        "عکس خون", "زخم خونی", "خونریزی", "صحنه خونین", "جراحت باز",
    ],
}

# The arbiter's groups. Read as calibrated probabilities rather than margins, so unlike the
# concepts above these have to describe the *whole picture*, not a thing that appears in it.
ARBITER: dict[str, list[str]] = {
    "EXPLICIT": [
        "explicit pornography", "a pornographic photo", "explicit sexual activity",
        "a photo of sexual intercourse", "a naked person exposing their genitals",
        "full frontal nudity", "a nude person with visible genitals",
        "hardcore pornography", "an explicit hentai drawing", "explicit sexual content",
        "پورن", "عکس مستهجن", "رابطه جنسی", "برهنگی کامل",
    ],
    "SUGGESTIVE": [
        "a person in lingerie", "a person in a bikini", "a shirtless man",
        "a woman in a swimsuit", "a suggestive photo of a person",
        "a person posing seductively in underwear", "a revealing outfit",
        "a photo of a person at the beach in swimwear",
        "عکس تحریک آمیز", "لباس زیر", "مایو", "لباس باز",
    ],
    "SAFE": [
        "an ordinary photograph", "a portrait of a fully clothed person",
        "a photo of a person's face", "a family photo", "a selfie of a clothed person",
        "a photo of people talking", "a photo of a landscape", "a photo of food",
        "a screenshot of text", "a photo of an animal", "a photo of a car",
        "a group of friends posing for a photo", "a professional headshot",
        "عکس معمولی", "عکس یک نفر با لباس", "عکس خانوادگی", "عکس چهره", "سلفی",
    ],
}


def pooled(out):
    """The pooled embedding, whichever shape the library hands back — transformers 4 returned a
    tensor from `get_text_features`, transformers 5 returns the whole output object."""
    return out.pooler_output if hasattr(out, "pooler_output") else out


def encode(model, tokenizer, prompts: list[str]) -> torch.Tensor:
    """The unit-length mean of a set of prompts, in the picture's own space.

    Each prompt is normalised *before* it is added, so a long one cannot outvote a short one by
    having a larger norm — the same rule `imgtext::embed` follows for its templates.
    """
    ids = [tokenizer(p)["input_ids"][:MAX_TOKENS] for p in prompts]
    padded = torch.tensor([row + [0] * (MAX_TOKENS - len(row)) for row in ids])
    with torch.no_grad():
        out = pooled(model.text_model(input_ids=padded))
    out = torch.nn.functional.normalize(out, dim=-1)
    return torch.nn.functional.normalize(out.mean(0), dim=-1)


def rust(name: str, vector: torch.Tensor) -> str:
    values = [f"{v:.6f}" for v in vector.tolist()]
    rows = [
        "    " + ", ".join(values[at : at + 8]) + ","
        for at in range(0, len(values), 8)
    ]
    return f"pub const {name}: [f32; {DIM}] = [\n" + "\n".join(rows) + "\n];\n"


HEADER = '''//! The fixed directions, generated by `tools/export_concepts.py` and never computed here.
//!
//! Each is the unit-length mean of a handful of English and Persian prompts through the
//! general model's text tower. Averaging a handful rather than trusting one is what makes the
//! margins usable: a single phrase moves the answer around far more than the picture does.
//!
//! `BACKGROUND` is the same thing for a set of deliberately dull prompts, and a concept's
//! score is its similarity *minus* that baseline. A contrastive model's absolute similarities
//! sit in a narrow band for everything, so only the difference carries information.
//!
//! `EXPLICIT`, `SUGGESTIVE` and `SAFE` are the NSFW arbiter's, and they are the exception:
//! they are read as calibrated probabilities through `vision::probability`, not as margins.
//! See `nsfw::arbiter` for why that difference exists.
//!
//! Regenerate all of them together — they are only comparable because one tower produced them:
//!
//! ```sh
//! python3 tools/export_concepts.py --write
//! ```

'''


def main() -> None:
    parser = argparse.ArgumentParser()
    # Must match export_vision.py's default: the vectors only mean anything against the image
    # tower of the SAME checkpoint. Regenerate both together when either changes.
    parser.add_argument("--model", default="google/siglip2-base-patch16-384")
    parser.add_argument("--out", default="src/handlers/concept_vectors.rs")
    parser.add_argument("--write", action="store_true", help="write the file rather than print a summary")
    parser.add_argument("--npz", help="write every vector to this .npz instead — for tools/nsfw_bench.py on a tower that is not the shipped one")
    args = parser.parse_args()

    from transformers import AutoModel, AutoTokenizer

    model = AutoModel.from_pretrained(args.model).eval()
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    if model.config.vision_config.hidden_size != DIM and not args.npz:
        sys.exit(f"{args.model} embeds in {model.config.vision_config.hidden_size}, not {DIM}")
    if args.npz:
        import numpy as np

        vectors = {name: encode(model, tokenizer, prompts).numpy() for name, prompts in {"BACKGROUND": BACKGROUND, **CONCEPTS, **ARBITER}.items()}
        np.savez(args.npz, model=np.array(args.model), **vectors)
        print(f"wrote {args.npz}: {', '.join(vectors)}")
        return

    body = [HEADER, rust("BACKGROUND", encode(model, tokenizer, BACKGROUND))]
    background = encode(model, tokenizer, BACKGROUND)
    for name, prompts in {**CONCEPTS, **ARBITER}.items():
        vector = encode(model, tokenizer, prompts)
        body.append("\n" + rust(name, vector))
        # Against the background *direction*, not against a picture. Two prompt centroids in a
        # contrastive space are always close to each other; what this catches is a set that
        # collapsed onto the baseline, which would make its margin identically zero.
        print(f"{name:12} cosine with the baseline direction {float(vector @ background):+.4f}")

    print(f"\nlogit_scale {float(model.logit_scale.exp()):.6f}  logit_bias {float(model.logit_bias):.6f}")
    text = "".join(body)
    if not args.write:
        print(f"\n{len(text)} bytes; pass --write to replace {args.out}")
        return
    pathlib.Path(args.out).write_text(text, encoding="utf-8")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
