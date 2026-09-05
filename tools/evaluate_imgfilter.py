#!/usr/bin/env python3
"""Evaluate the no-calibration generic image-filter path on labelled image sets.

The bot's decision is:

    dot(image_embedding, phrase_embedding) - dot(image_embedding, BACKGROUND) >= cut

This tool reproduces that path with the exported ONNX image/text towers, the Rust BPE files,
and the generated background vector.  It reports the deployed fixed cut, a threshold sweep,
class-balanced metrics, and the largest false-positive scores.  A high raw accuracy on an
imbalanced one-vs-rest set is not enough, so the useful gates are balanced accuracy, recall,
and false-positive rate.

Examples:

    python3 tools/evaluate_imgfilter.py --vision-files target/release --dataset imagenette \
        --per-class 30 --cache
    python3 tools/evaluate_imgfilter.py --vision-files target/release \
        --dataset imagenette --dataset food101 --per-class 20 --cache

The datasets are fetched from the public Hugging Face dataset-server API and cached under
target/imgfilter-eval.  No labels are inferred from the model: ImageNette and Food-101 provide
the class labels used for the positive and negative sides of every phrase.
"""

from __future__ import annotations

import argparse
import io
import json
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

HERE = pathlib.Path(__file__).resolve().parent.parent
ROWS = "https://datasets-server.huggingface.co/rows"
SIDE = 224
TOKENS = 64
TEMPLATES = [
    "{}",
    "عکس {}",
    "تصویری از {}",
    "a photo of {}",
    "a photograph of {}",
    "a picture of {}",
    "an image of {}",
    "a close-up photo of {}",
]
MODEL_CUT = 0.038

DATASETS = {
    "imagenette": ("frgfm/imagenette", "320px", "validation", "label", "image"),
    "food101": ("ethz/food101", "default", "validation", "label", "image"),
}

PARQUET_DATASETS = {
    "imagenette": "https://huggingface.co/datasets/frgfm/imagenette/resolve/refs%2Fconvert%2Fparquet/320px/validation/0000.parquet",
}


@dataclass(frozen=True)
class Sample:
    path: pathlib.Path
    label: int
    name: str


def request_json(params: dict) -> dict:
    for attempt in range(6):
        try:
            response = requests.get(ROWS, params=params, timeout=90)
            if response.status_code == 200:
                return response.json()
        except requests.RequestException:
            pass
        time.sleep(min(2.0**attempt, 15.0))
    raise RuntimeError(f"could not fetch dataset rows: {params}")


def page(dataset: str, config: str, split: str, offset: int) -> dict:
    return request_json(
        {"dataset": dataset, "config": config, "split": split, "offset": offset, "length": 100}
    )


def safe_name(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", name).strip("_") or "class"


def download(url: str, path: pathlib.Path) -> bool:
    if path.exists() and path.stat().st_size > 100:
        return True
    for attempt in range(5):
        try:
            response = requests.get(url, timeout=90)
            if response.status_code == 200:
                image = Image.open(io.BytesIO(response.content)).convert("RGB")
                path.parent.mkdir(parents=True, exist_ok=True)
                image.save(path, "JPEG", quality=94)
                return True
        except (OSError, ValueError, requests.RequestException):
            pass
        time.sleep(min(1.5**attempt, 10.0))
    return False


def download_file(url: str, path: pathlib.Path) -> None:
    if path.exists() and path.stat().st_size > 100_000:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".part")
    with requests.get(url, stream=True, timeout=120) as response:
        response.raise_for_status()
        with temporary.open("wb") as handle:
            for chunk in response.iter_content(1024 * 1024):
                if chunk:
                    handle.write(chunk)
    temporary.replace(path)


def fetch_parquet_dataset(
    name: str, per_class: int, cache: pathlib.Path, max_classes: int | None
) -> list[Sample]:
    """Read embedded image bytes from a public parquet conversion without the rate-limited rows API."""
    try:
        import pyarrow.parquet as parquet
    except ImportError as error:
        raise RuntimeError("ImageNette evaluation needs pyarrow in the evaluation environment") from error
    parquet_path = cache / name / "validation.parquet"
    print(f"{name}: downloading parquet if needed", flush=True)
    download_file(PARQUET_DATASETS[name], parquet_path)
    file = parquet.ParquetFile(parquet_path)
    metadata = json.loads(file.schema_arrow.metadata[b"huggingface"])
    names = metadata["info"]["features"]["label"]["names"]
    root = cache / name
    classes = list(range(min(len(names), max_classes))) if max_classes else list(range(len(names)))
    counts = {label: 0 for label in classes}
    for batch in file.iter_batches(batch_size=64, columns=["image", "label"]):
        for row in batch.to_pylist():
            label = int(row["label"])
            if label not in counts or counts[label] >= per_class:
                continue
            folder = root / f"{label:03d}_{safe_name(names[label])}"
            path = folder / f"{counts[label]:05d}.jpg"
            counts[label] += 1
            if path.exists() and path.stat().st_size > 100:
                continue
            image_bytes = row["image"]["bytes"]
            folder.mkdir(parents=True, exist_ok=True)
            Image.open(io.BytesIO(image_bytes)).convert("RGB").save(path, "JPEG", quality=94)
        if all(count >= per_class for count in counts.values()):
            break
    out: list[Sample] = []
    for label in classes:
        label_name = names[label]
        folder = root / f"{label:03d}_{safe_name(label_name)}"
        out.extend(Sample(path, label, label_name) for path in sorted(folder.glob("*.jpg"))[:per_class])
    missing = [names[label] for label in counts if counts[label] < per_class]
    if missing:
        raise RuntimeError(f"{name}: missing classes: {missing[:8]}")
    print(f"{name}: loaded {len(out)} cached labelled images", flush=True)
    return out


def fetch_dataset(
    name: str, per_class: int, cache: pathlib.Path, max_classes: int | None
) -> list[Sample]:
    if name in PARQUET_DATASETS:
        return fetch_parquet_dataset(name, per_class, cache, max_classes)
    dataset, config, split, label_key, image_key = DATASETS[name]
    root = cache / name
    print(f"{name}: reading labels", flush=True)
    first = page(dataset, config, split, 0)
    feature = next(
        item for item in first["features"] if item["name"] == label_key
    )
    names = feature["type"]["names"]
    classes = list(range(min(len(names), max_classes))) if max_classes else list(range(len(names)))
    cached: list[Sample] = []
    for label in classes:
        folder = root / f"{label:03d}_{safe_name(names[label])}"
        paths = sorted(folder.glob("*.jpg"))
        cached.extend(Sample(path, label, names[label]) for path in paths[:per_class])
    counts = {label: 0 for label in classes}
    for sample in cached:
        counts[sample.label] += 1
    if all(count >= per_class for count in counts.values()):
        return cached

    jobs: list[tuple[str, pathlib.Path]] = []
    offset = 0
    total = first.get("num_rows_total", 100_000)
    while offset < total and not all(count >= per_class for count in counts.values()):
        print(f"{name}: scanning rows {offset}/{total}", flush=True)
        data = first if offset == 0 else page(dataset, config, split, offset)
        for entry in data.get("rows", []):
            row = entry["row"]
            label = int(row[label_key])
            if label not in counts:
                continue
            if counts[label] >= per_class:
                continue
            folder = root / f"{label:03d}_{safe_name(names[label])}"
            path = folder / f"{counts[label]:05d}.jpg"
            counts[label] += 1
            if not (path.exists() and path.stat().st_size > 100):
                source = row[image_key]["src"]
                jobs.append((source, path))
        offset += 100

    print(f"{name}: downloading {len(jobs)} images", flush=True)
    with ThreadPoolExecutor(max_workers=16) as pool:
        good = sum(pool.map(lambda job: download(*job), jobs))
    if jobs:
        print(f"{name}: downloaded {good}/{len(jobs)} images")

    out: list[Sample] = []
    for label in classes:
        folder = root / f"{label:03d}_{safe_name(names[label])}"
        paths = sorted(folder.glob("*.jpg"))[:per_class]
        out.extend(Sample(path, label, names[label]) for path in paths)
    missing = [names[label] for label in classes if counts[label] < per_class]
    if missing:
        raise RuntimeError(f"{name}: missing classes: {missing[:8]}")
    return out


def unescape(text: str) -> str:
    if "\\" not in text:
        return text
    out: list[str] = []
    chars = iter(text)
    for char in chars:
        if char != "\\":
            out.append(char)
            continue
        try:
            escaped = next(chars)
        except StopIteration:
            out.append("\\")
            break
        out.append({"n": "\n", "r": "\r", "s": " ", "\\": "\\"}.get(escaped, "\\" + escaped))
    return "".join(out)


class Bpe:
    def __init__(self, vocab: pathlib.Path, merges: pathlib.Path):
        self.ids = {unescape(line.rstrip("\r")): at for at, line in enumerate(vocab.read_text(encoding="utf-8").splitlines())}
        self.eos = self.ids["<eos>"]
        unk = self.ids["<unk>"]
        self.bytes = [self.ids.get(f"<0x{value:02X}>", unk) for value in range(256)]
        self.merges: dict[tuple[int, int], tuple[int, int]] = {}
        for rank, line in enumerate(merges.read_text(encoding="utf-8").splitlines()):
            pair = line.rstrip("\r").split(" ", 1)
            if len(pair) != 2:
                continue
            left, right = (unescape(piece) for piece in pair)
            joined = left + right
            if left in self.ids and right in self.ids and joined in self.ids:
                self.merges.setdefault((self.ids[left], self.ids[right]), (rank, self.ids[joined]))

    def encode(self, text: str) -> np.ndarray:
        symbols: list[int] = []
        for char in text:
            char = "▁" if char == " " else char
            encoded = char.encode("utf-8")
            if char in self.ids:
                symbols.append(self.ids[char])
            else:
                symbols.extend(self.bytes[value] for value in encoded)
        while len(symbols) > 1:
            best: tuple[int, int, int] | None = None
            for at in range(len(symbols) - 1):
                merged = self.merges.get((symbols[at], symbols[at + 1]))
                if merged and (best is None or merged[0] < best[0]):
                    best = (merged[0], at, merged[1])
            if best is None:
                break
            _, at, merged = best
            left, right = symbols[at], symbols[at + 1]
            next_symbols: list[int] = []
            index = 0
            while index < len(symbols):
                if index + 1 < len(symbols) and symbols[index] == left and symbols[index + 1] == right:
                    next_symbols.append(merged)
                    index += 2
                else:
                    next_symbols.append(symbols[index])
                    index += 1
            symbols = next_symbols
        symbols = symbols[: TOKENS - 1] + [self.eos]
        return np.asarray(symbols + [0] * (TOKENS - len(symbols)), dtype=np.int64)[None, :]


def unit(values: np.ndarray) -> np.ndarray:
    norm = np.linalg.norm(values)
    return values if norm <= 0 else values / norm


def text_vector(session: ort.InferenceSession, bpe: Bpe, phrase: str) -> np.ndarray:
    name = session.get_inputs()[0].name
    vectors = []
    for template in TEMPLATES:
        output = session.run(None, {name: bpe.encode(template.format(phrase))})[0][0]
        vectors.append(unit(output.astype(np.float32)))
    return unit(np.sum(vectors, axis=0))


def background_vector() -> np.ndarray:
    source = (HERE / "src" / "handlers" / "concept_vectors.rs").read_text(encoding="utf-8")
    body = re.search(r"pub const BACKGROUND: \[f32; 768\] = \[(.*?)\];", source, re.S)
    if not body:
        raise RuntimeError("BACKGROUND is missing from concept_vectors.rs")
    return np.asarray([float(value) for value in re.findall(r"-?\d+\.\d+", body.group(1))], dtype=np.float32)


def image_vector(session: ort.InferenceSession, path: pathlib.Path) -> np.ndarray:
    shape = session.get_inputs()[0].shape
    side = next((int(value) for value in shape[-2:] if isinstance(value, int) and value > 0), SIDE)
    with Image.open(path) as image:
        image = image.convert("RGB").resize((side, side), Image.Resampling.BILINEAR)
        pixels = np.asarray(image, dtype=np.float32).transpose(2, 0, 1) / 127.5 - 1.0
    outputs = session.run(None, {session.get_inputs()[0].name: pixels[None, ...]})
    for output in outputs:
        values = np.asarray(output, dtype=np.float32)
        if values.size == 768:
            return unit(values.reshape(-1))
    raise RuntimeError("vision graph did not emit a 768-wide embedding")


def metrics(scores: np.ndarray, labels: np.ndarray, cut: float) -> tuple[float, float, float, float, int, int, int, int]:
    predicted = scores >= cut
    positive = labels == 1
    tp = int(np.count_nonzero(predicted & positive))
    fn = int(np.count_nonzero(~predicted & positive))
    fp = int(np.count_nonzero(predicted & ~positive))
    tn = int(np.count_nonzero(~predicted & ~positive))
    recall = tp / max(1, tp + fn)
    fpr = fp / max(1, fp + tn)
    specificity = tn / max(1, fp + tn)
    balanced = (recall + specificity) / 2.0
    accuracy = (tp + tn) / max(1, tp + tn + fp + fn)
    return accuracy, balanced, recall, fpr, tp, fn, fp, tn


def evaluate(samples: list[Sample], image_session: ort.InferenceSession, text_session: ort.InferenceSession, bpe: Bpe) -> None:
    names = sorted({sample.name for sample in samples}, key=lambda value: value.lower())
    labels = np.asarray([sample.name for sample in samples])
    images = []
    for at, sample in enumerate(samples, 1):
        images.append(image_vector(image_session, sample.path))
        if at % 100 == 0 or at == len(samples):
            print(f"embedded {at}/{len(samples)} images")
    image_matrix = np.asarray(images, dtype=np.float32)
    background = background_vector()
    phrase_matrix = np.asarray([text_vector(text_session, bpe, name) for name in names])
    scores = image_matrix @ phrase_matrix.T - (image_matrix @ background)[:, None]

    print("\ncut       accuracy  balanced  recall    fpr       tp/fn     fp/tn")
    for cut in [0.05 + at * 0.01 for at in range(21)] + [MODEL_CUT]:
        flat_scores = []
        flat_labels = []
        for index, name in enumerate(names):
            flat_scores.extend(scores[:, index])
            flat_labels.extend(labels == name)
        result = metrics(np.asarray(flat_scores), np.asarray(flat_labels), cut)
        print(f"{cut:0.3f}     {result[0]:0.4f}    {result[1]:0.4f}    {result[2]:0.4f}  {result[3]:0.4f}  {result[4]}/{result[5]}   {result[6]}/{result[7]}")

    best = None
    for cut in np.arange(-0.05, 0.501, 0.001):
        flat_scores = []
        flat_labels = []
        for index, name in enumerate(names):
            flat_scores.extend(scores[:, index])
            flat_labels.extend(labels == name)
        result = metrics(np.asarray(flat_scores), np.asarray(flat_labels), float(cut))
        if result[3] <= 0.01 and (best is None or (result[2], result[1]) > (best[1][2], best[1][1])):
            best = (float(cut), result)
    print("\nBest global cut with FPR <= 1%:")
    if best:
        cut, result = best
        print(f"cut={cut:.3f} accuracy={result[0]:.4f} balanced={result[1]:.4f} recall={result[2]:.4f} fpr={result[3]:.4f}")
    else:
        print("none")

    print("\nPer-class deployed-cut summary (largest false positives first):")
    for index, name in enumerate(names):
        result = metrics(scores[:, index], labels == name, MODEL_CUT)
        false_positive = sorted(
            ((float(score), sample.name, sample.path.name) for score, sample in zip(scores[:, index], samples) if sample.name != name),
            reverse=True,
        )[:3]
        print(f"{name}: recall={result[2]:.3f} fpr={result[3]:.3f} balanced={result[1]:.3f} top_fp={false_positive}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vision-files", type=pathlib.Path, default=HERE / "target" / "release")
    parser.add_argument("--dataset", action="append", choices=sorted(DATASETS))
    parser.add_argument("--per-class", type=int, default=30)
    parser.add_argument("--max-classes", type=int, help="only the first N labelled classes")
    parser.add_argument("--cache", action="store_true")
    args = parser.parse_args()
    if args.per_class < 2:
        raise SystemExit("--per-class must be at least 2")
    cache = HERE / "target" / "imgfilter-eval"
    all_samples: list[Sample] = []
    for name in dict.fromkeys(args.dataset or ["imagenette"]):
        all_samples.extend(fetch_dataset(name, args.per_class, cache, args.max_classes))
    print(f"samples={len(all_samples)} classes={len(set(sample.name for sample in all_samples))}")
    image_session = ort.InferenceSession(str(args.vision_files / "vision.onnx"), providers=["CPUExecutionProvider"])
    text_session = ort.InferenceSession(str(args.vision_files / "vision_text.onnx"), providers=["CPUExecutionProvider"])
    bpe = Bpe(args.vision_files / "vision_text_vocab.txt", args.vision_files / "vision_text_merges.txt")
    evaluate(all_samples, image_session, text_session, bpe)


if __name__ == "__main__":
    main()
