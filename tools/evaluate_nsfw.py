#!/usr/bin/env python3
"""Evaluate the NSFW cascade on public stress sets, with and without the general model.

**The negative set is the whole experiment, and the first version of this file got it wrong.**
It used `ethz/food101` as the safe side, reported a 0.0% false-positive rate, and was believed
— while the complaint from real groups was false positives. Photographs of food are not what a
group sends and not what the classifier trips on. Measured against 550 photographs *of people*,
the same cascade deletes two: a studio headshot the first model scores 0.934, and a woman in a
strapless top at a bar it scores 0.746. Both are exactly the complaint, and neither could ever
have appeared in a set of dinners.

So the default negative side is people — Flickr30k for everyday photographs and CelebA for
portraits, which is where the first model is weakest. Food-101 is kept as `--negatives food`,
because a control that the cascade finds easy is worth having when a change looks catastrophic.

    pip install numpy pillow onnxruntime requests
    python3 tools/evaluate_nsfw.py --count 128
    python3 tools/evaluate_nsfw.py --count 128 --vision target/release/vision.onnx

With `--vision` it also evaluates the arbiter, which is the reason the cascade has a third
model at all. Nothing here retypes a threshold: every constant is read out of the Rust source,
so this tool cannot drift from what the bot actually does.

Images are downloaded into memory unless `--cache` is given, and `--cache` is worth giving —
the sets take minutes to fetch and seconds to score, so every rerun after the first is free.
"""

from __future__ import annotations

import argparse
import io
import pathlib
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass

import numpy as np
import onnxruntime as ort
import requests
from PIL import Image

ROWS = "https://datasets-server.huggingface.co/rows"
HERE = pathlib.Path(__file__).resolve().parent.parent

# (dataset, config, split, row filter). The filter is a `violation_type` for PAD3, whose rows
# are labelled, and `None` for the sets that are one thing all the way through.
SETS = {
    "people": ("nlphuji/flickr30k", "TEST", "test", None),
    "faces": ("nielsr/CelebA-faces", "default", "train", None),
    "food": ("ethz/food101", "default", "train", None),
    "nsfw": ("x1101/nsfw-full", "default", "train", None),
    "porn": ("arkananta27/pad3-image", "default", "train", ("violation_type", "pornography")),
    "softporn": ("arkananta27/pad3-image", "default", "train", ("violation_type", "soft_porn")),
}

NEGATIVES = {
    "people": ["people", "faces"],
    "food": ["food"],
    "all": ["people", "faces", "food"],
}

MAIN_SIDE = MAIN_RESIZE = 384
GRADE_SIDE, GRADE_RESIZE = 224, 256


def rust_const(module: str, name: str) -> float:
    """One constant, read out of the Rust source that owns it.

    The version of this file that retyped them had already drifted from the bot in two places
    by the time anyone checked, which is the same argument `setting::SETTINGS` makes for taking
    a range from the module that declares it.
    """
    source = (HERE / "src" / "handlers" / f"{module}.rs").read_text(encoding="utf-8")
    found = re.search(rf"const {name}: f32 = ([\d.]+);", source)
    if not found:
        sys.exit(f"{name} is not a f32 constant in {module}.rs any more")
    return float(found.group(1))


def rust_vector(name: str) -> np.ndarray:
    source = (HERE / "src" / "handlers" / "concept_vectors.rs").read_text(encoding="utf-8")
    found = re.search(rf"pub const {name}: \[f32; \d+\] = \[(.*?)\];", source, re.S)
    if not found:
        sys.exit(f"{name} is not in concept_vectors.rs any more")
    return np.asarray([float(x) for x in re.findall(r"-?\d+\.\d+", found.group(1))], dtype=np.float32)


CORROBORATE = rust_const("nsfw", "CORROBORATE")
WIDE = rust_const("nsfw", "WIDE")
SHARPNESS_FLOOR = rust_const("nsfw", "SHARPNESS_FLOOR")
GRADED_CONFIDENT = rust_const("nsfw", "GRADED_CONFIDENT")
CONFIDENT = rust_const("nsfw", "CONFIDENT")
BRIDGE_SCORE = rust_const("nsfw", "BRIDGE_SCORE")
BRIDGE_FLOOR = rust_const("nsfw", "BRIDGE_FLOOR")
STRONG_BRIDGE_SCORE = rust_const("nsfw", "STRONG_BRIDGE_SCORE")
STRONG_BRIDGE_FLOOR = rust_const("nsfw", "STRONG_BRIDGE_FLOOR")
NEUTRAL_FLOOR = rust_const("nsfw", "NEUTRAL_FLOOR")
EXPLICIT_FLOOR = rust_const("nsfw", "EXPLICIT_FLOOR")
AGREE = rust_const("nsfw", "AGREE")
RECOVER = rust_const("nsfw", "RECOVER")
DETAIL_FLOOR = MAIN_RESIZE


@dataclass
class Score:
    group: str
    label: int
    nsfw: float
    hard: float
    neutral: float
    frail: bool
    arbiter: float | None


# ---------------------------------------------------------------------------------------
# the same transforms the bot uses
# ---------------------------------------------------------------------------------------


def softmax(values: np.ndarray) -> np.ndarray:
    values = values.astype(np.float32, copy=False)
    values = values - values.max()
    values = np.exp(values)
    return values / values.sum()


def fit(image: Image.Image, resize: int, side: int) -> Image.Image:
    width, height = image.size
    scale = resize / max(1, min(width, height))
    resized = image.resize(
        (max(side, round(width * scale)), max(side, round(height * scale))),
        Image.Resampling.BICUBIC,
    )
    left, top = (resized.width - side) // 2, (resized.height - side) // 2
    return resized.crop((left, top, left + side, top + side))


def tiles(image: Image.Image) -> list[Image.Image]:
    width, height = image.size
    long_side, short_side = max(width, height), max(1, min(width, height))
    if long_side / short_side < WIDE:
        return [image]
    if width >= height:
        return [image, image.crop((0, 0, short_side, short_side)),
                image.crop((width - short_side, 0, width, short_side))]
    return [image, image.crop((0, 0, short_side, short_side)),
            image.crop((0, height - short_side, short_side, height))]


def planar(view: Image.Image, low: float, high: float) -> np.ndarray:
    array = np.asarray(view, dtype=np.float32) / 255.0
    return np.transpose(array * (high - low) + low, (2, 0, 1))


def sharpness(image: Image.Image) -> float:
    array = np.asarray(image, dtype=np.float32)
    if min(array.shape[:2]) < 3:
        return 0.0
    luma = 0.299 * array[..., 0] + 0.587 * array[..., 1] + 0.114 * array[..., 2]
    laplacian = (
        luma[:-2, 1:-1] + luma[2:, 1:-1] + luma[1:-1, :-2] + luma[1:-1, 2:]
        - 4.0 * luma[1:-1, 1:-1]
    )
    return float(laplacian.var())


# ---------------------------------------------------------------------------------------
# fetching
# ---------------------------------------------------------------------------------------


def get(url: str, **kwargs) -> requests.Response | None:
    for attempt in range(5):
        try:
            response = requests.get(url, timeout=90, **kwargs)
            if response.status_code == 200:
                return response
        except requests.RequestException:
            pass
        time.sleep(1.5 ** attempt)
    return None


def page(dataset: str, config: str, split: str, offset: int, length: int) -> list[dict]:
    response = get(ROWS, params={"dataset": dataset, "config": config, "split": split,
                                 "offset": offset, "length": length})
    return response.json().get("rows", []) if response else []


def fetch(group: str, count: int, cache: pathlib.Path | None) -> list[Image.Image]:
    """`count` pictures from one set, from the cache when there is one.

    PAD3 is one big mixed split, so its rows are filtered by label rather than sliced — which
    means walking it a hundred rows at a time until enough of the wanted kind turn up.
    """
    folder = cache / group if cache else None
    if folder and folder.is_dir():
        have = sorted(folder.glob("*.jpg"))[:count]
        if len(have) >= count:
            return [Image.open(p).convert("RGB") for p in have]

    dataset, config, split, keep = SETS[group]
    images: list[Image.Image] = []
    offset, block = 0, 100

    def take(source: str) -> Image.Image | None:
        response = get(source)
        if not response:
            return None
        try:
            return Image.open(io.BytesIO(response.content)).convert("RGB")
        except (OSError, ValueError):
            return None

    with ThreadPoolExecutor(12) as pool:
        pending = []
        while len(pending) < count and offset < 60_000:
            rows = page(dataset, config, split, offset, block)
            if not rows:
                break
            for row in rows:
                fields = row.get("row", {})
                if keep and fields.get(keep[0]) != keep[1]:
                    continue
                source = (fields.get("image") or {}).get("src")
                if source:
                    pending.append(pool.submit(take, source))
                if len(pending) >= count:
                    break
            offset += block
        images = [image for image in (job.result() for job in pending) if image is not None]

    if folder:
        folder.mkdir(parents=True, exist_ok=True)
        for at, image in enumerate(images):
            image.save(folder / f"{at:05d}.jpg", "JPEG", quality=92)
    return images


# ---------------------------------------------------------------------------------------
# scoring
# ---------------------------------------------------------------------------------------


def view_score(view: Image.Image, session: ort.InferenceSession, name: str, low: float) -> float:
    logits = session.run(None, {name: planar(view, -1.0, 1.0)[None, ...]})[0][0]
    first = float(softmax(logits)[0])
    if not (low < first < CONFIDENT):
        return first
    mirrored = view.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
    logits = session.run(None, {name: planar(mirrored, -1.0, 1.0)[None, ...]})[0][0]
    return (first + float(softmax(logits)[0])) / 2.0


def score(images: dict[str, list[Image.Image]], main, grader, vision) -> list[Score]:
    main_in, grade_in = main.get_inputs()[0].name, grader.get_inputs()[0].name
    if vision:
        vision_in = vision.get_inputs()[0].name
        side = vision.get_inputs()[0].shape[2]
        explicit_at, safe_at = rust_vector("EXPLICIT"), rust_vector("SAFE")
    grade_above = 0.10
    out: list[Score] = []
    for group, pictures in images.items():
        label = 1 if group in ("nsfw", "porn", "softporn") else 0
        for image in pictures:
            views = [fit(tile, MAIN_RESIZE, MAIN_SIDE) for tile in tiles(image)]
            scores = [view_score(view, main, main_in, grade_above) for view in views]
            frail = min(image.size) < DETAIL_FLOOR or sharpness(image) < SHARPNESS_FLOOR
            whole, ends = scores[0], scores[1:]
            nsfw = whole if (whole < CORROBORATE or frail) else max([whole, *ends])

            grades = []
            for tile in tiles(image):
                logits = grader.run(
                    None, {grade_in: planar(fit(tile, GRADE_RESIZE, GRADE_SIDE), 0.0, 1.0)}
                )[0]
                grades.append(softmax(logits[0] if logits.ndim == 2 else logits))
                if grades[-1][1] + grades[-1][3] >= EXPLICIT_FLOOR:
                    break
            hard = max(float(p[1] + p[3]) for p in grades)
            neutral = min(float(p[2]) for p in grades)

            arbiter = None
            if vision:
                square = image.resize((side, side), Image.Resampling.BILINEAR)
                embedding = vision.run(None, {vision_in: planar(square, -1.0, 1.0)[None, ...]})[0][0]
                embedding = embedding / (np.linalg.norm(embedding) + 1e-12)
                arbiter = float(embedding @ explicit_at) - float(embedding @ safe_at)

            out.append(Score(group, label, nsfw, hard, neutral, frail, arbiter))
    return out


def cascade(s: Score, limit: float) -> bool:
    """The two-model policy — what a deploy without the general model still does."""
    if s.nsfw * 100 < limit:
        return False
    confirmed = (
        s.nsfw >= GRADED_CONFIDENT
        or (s.nsfw >= BRIDGE_SCORE and s.hard >= BRIDGE_FLOOR)
        or (s.nsfw >= STRONG_BRIDGE_SCORE and s.hard >= STRONG_BRIDGE_FLOOR)
    )
    innocent = s.neutral >= NEUTRAL_FLOOR and s.nsfw < CONFIDENT
    weak = s.frail and s.hard < EXPLICIT_FLOOR
    return confirmed and not innocent and not weak


def deployed(s: Score, limit: float) -> bool:
    """`nsfw::allow_delete`, arbiter and all."""
    if s.arbiter is None:
        return cascade(s, limit)
    if s.nsfw * 100 < limit or s.arbiter < AGREE:
        return False
    return cascade(s, limit) or s.arbiter >= RECOVER


def rates(scores: list[Score], predicate) -> tuple[float, float, int, int]:
    positive = [s for s in scores if s.label == 1]
    negative = [s for s in scores if s.label == 0]
    caught = sum(predicate(s) for s in positive)
    wrong = sum(predicate(s) for s in negative)
    return (
        caught / max(1, len(positive)),
        wrong / max(1, len(negative)),
        wrong,
        len(negative),
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=96, help="pictures from each set")
    parser.add_argument("--negatives", choices=tuple(NEGATIVES), default="people")
    parser.add_argument("--vision", help="path to vision.onnx, to evaluate the arbiter too")
    parser.add_argument("--cache", help="keep the pictures here so a rerun costs nothing")
    args = parser.parse_args()

    cache = pathlib.Path(args.cache) if args.cache else None
    wanted = NEGATIVES[args.negatives] + ["nsfw", "porn"]
    images = {}
    for group in wanted:
        images[group] = fetch(group, args.count, cache)
        print(f"{group}: {len(images[group])} pictures")
    if not any(images[g] for g in ("nsfw", "porn")):
        sys.exit("no offending pictures were fetched; nothing to measure")

    main_session = ort.InferenceSession(str(HERE / "assets/nsfw.onnx"), providers=["CPUExecutionProvider"])
    grade_session = ort.InferenceSession(str(HERE / "assets/nsfw_grade.onnx"), providers=["CPUExecutionProvider"])
    vision_session = (
        ort.InferenceSession(args.vision, providers=["CPUExecutionProvider"]) if args.vision else None
    )
    if not vision_session:
        print("\nno --vision: measuring the two-model cascade only, which is what a deploy")
        print("without vision.onnx beside the binary actually runs.\n")

    scored = score(images, main_session, grade_session, vision_session)
    positives = sum(s.label == 1 for s in scored)
    negatives = sum(s.label == 0 for s in scored)
    print(f"\n{positives} offending, {negatives} ordinary ({args.negatives})")

    for limit in (30, 50, 70):
        recall, fpr, wrong, total = rates(scored, lambda s, l=limit: cascade(s, l))
        print(f"limit {limit:>3}  cascade        recall={recall:.3f} fpr={fpr:.4f} ({wrong}/{total})")
        if vision_session:
            recall, fpr, wrong, total = rates(scored, lambda s, l=limit: deployed(s, l))
            print(f"           with arbiter   recall={recall:.3f} fpr={fpr:.4f} ({wrong}/{total})")

    worst = sorted((s for s in scored if s.label == 0), key=lambda s: -s.nsfw)[:8]
    print("\nthe ordinary pictures the first model is most wrong about:")
    for s in worst:
        arbiter = f" arbiter {s.arbiter:+.4f}" if s.arbiter is not None else ""
        print(f"  {s.group:7} score {s.nsfw:.3f} hard {s.hard:.3f} neutral {s.neutral:.3f}{arbiter}")


if __name__ == "__main__":
    main()
