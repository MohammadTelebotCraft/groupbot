#!/usr/bin/env python3
"""Writes `assets/imgfilter_concepts.bin`: a classifier fitted on human-verified pictures for every phrase a table
names that Open Images has enough verified pictures of, stored as the filter it makes — a vector and its cut.

    ../bin/python tools/imgfilter_concepts.py fetch            # Open Images val, test and chosen train pictures
    for d in oivali oitest oitrain; do                         # embed each with the production tower
        ../bin/python tools/imgfilter_concepts.py manifest $d
        ~/.cache/nsfw-train/venv-gpu/bin/python tools/vision_embed.py --manifest ~/.cache/concepts/${d}_manifest.tsv \
            --root ~/.cache --hf google/siglip2-base-patch16-384 --out ~/.cache/concepts/${d/oivali/oival}_base384.npz \
            --batch 64 --threads 12 --resume
    done
    ../bin/python tools/imgfilter_concepts.py labels           # one pass over the train labels for our pictures
    ../bin/python tools/imgfilter_concepts.py eval [--limit N] # held-out numbers (against the shipped bank)
    ../bin/python tools/imgfilter_concepts.py write            # the asset, fitted on everything

Needs `oidv7-{val,test}-annotations-human-imagelabels.csv`, `oidv7-train-annotations-human-imagelabels.csv`
(2.7 GB, storage.googleapis.com/openimages/v7/) and `train-hosted.txt` in `~/.cache/concepts/oi`.

Pictures: Open Images validation and test (every picture), the hosted train pictures chosen so each class
with twenty verified positives has up to 250, and the phrase pool — embedded by `vision_embed.py` with the
production tower as `oival_base384.npz`, `oitest_base384.npz`, `oitrain_base384.npz`, `pool_base384.npz`.
About 34 GB of pictures and two hours of fetching and embedding on a laptop GPU.

A phrase filter is otherwise `imgfilter::learn`: the phrase's text direction, blended with a head fitted on
the thirty pool pictures that direction ranks highest. Those thirty are the direction's own guess, so the
head learns the direction's mistakes with it. Here the positives are pictures people verified, the
negatives are pictures people verified *not* to show it, ordinary pictures and the everyday bank, and the
text direction stays in the blend for what the verified pictures do not cover. The cut is measured exactly
as `learn` measures it — the bank and the pool past the direction's own top, at `LEARNED_PHRASE_RATE` —
less the pool pictures verified to show the thing, which are not mistakes.

Which English phrase a class answers to: a lexicon row's English *is* the class's display name, lowercased;
a hand-written row's English matches when it is the display name with an article (`a dog`), or through
`ALIAS` below. The text direction is of the row's own English, the one the bot would embed.
"""

from __future__ import annotations

import argparse
import collections
import csv
import hashlib
import os
import pathlib
import pickle
import struct
import sys
import time

# `write` fits in WORKERS processes; each fit is matrix-vector work that a second thread barely helps.
WORKERS = 6
for variable in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(variable, "2")

import numpy as np  # noqa: E402

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from evaluate_imgfilter import Bpe, background_vector  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent
ROOT = pathlib.Path.home() / ".cache/concepts"
OI = ROOT / "oi"
OUT = HERE / "assets/imgfilter_concepts.bin"
VISION = HERE / "target/release"
EN_T = ["{}", "a photo of {}", "a photograph of {}", "a picture of {}", "an image of {}", "a close-up photo of {}"]
CANONICAL = {"english springer": "english springer spaniel"}  # imgtext::canonical_phrase
RATE = 5e-5  # imgfilter::LEARNED_PHRASE_RATE
EXCLUDE = 1000  # imgfilter::LEARN_EXCLUDE
MIN_POSITIVES = 40
SHARE = 0.25  # the text direction's share of the blend, standardised over the calibration set
# The most of the pictures people verified NOT to show a thing that may clear its cut. Measured over 150
# phrases, held out: at 20% recall 0.603 -> 0.482, verified look-alikes 35.7% -> 18.7%, held-out ordinary
# pictures 19.7 -> 9.7 per 100 000 (two thirds of those really show the thing), and hits in the real
# production reservoir 15 -> 2 over 150 x 1 024.
LOOKALIKE_SHARE = 0.20
# And then this many standard deviations of the filter's scores over the bank above all of that. Every one of the
# 105 held-out ordinary pictures the 150 filters still took was looked at: 81 showed the thing, 24 did not (a steak
# for "onion", a girl for "student", bubble gum for "foam", a web page for "push-button"), and 22 of the 24 sat
# within 0.4 deviations of the cut. At +0.5: recall 0.482 -> 0.364, 23 hits with 2 wrong; at +0.25, 51 with 9
# wrong. The bot also asks each picture what it is (`imgfilter::Gate`, `tools/imgfilter_vocab.py`): at +0.25 with
# the filter's subject among the picture's 25 best of 20 931 names, 25 hits and none wrong, recall 0.326, and
# production reservoir hits 2 -> 1 — better than either a stricter cut or the gate alone.
MARGIN_SD = 0.25
# The negatives' relative weights. Without the region's pictures the fitted filters flagged more held-out
# Iranian photographs than `learn` did (30 against 21 over 200 phrases); weighted in, 12. Weighting the bank
# up from 0.15 to 0.5 then bought recall and fewer everyday deletions for a few more look-alikes.
SHARES = {"verified negatives": 0.15, "ordinary": 0.1, "bank": 0.5, "pool": 0.1, "region": 0.25}
EMBEDDINGS = ["oival_base384.npz", "oitest_base384.npz", "oitrain_base384.npz", "pool_base384.npz"]
# Hand-written English that names an Open Images class by other words.
ALIAS = {
    "a mouse or a rat": "Mouse", "money and banknotes": "Banknote", "a luxury sports car": "Sports car",
    "tomatoes": "Tomato", "strawberries": "Strawberry", "flowers": "Flower", "the sea": "Sea",
    "the sky": "Sky", "clouds": "Cloud", "the moon": "Moon", "the sun": "Sun", "stars at night": "Star",
    "bullets": "Bullet", "coins": "Coin", "eggs": "Egg", "vegetables": "Vegetable", "glasses": "Glasses",
    "sunglasses": "Sunglasses", "shoes": "Footwear", "clothes": "Clothing", "balloons": "Balloon",
    "headphones": "Headphones", "french fries": "French fries", "pills and medicine": "Pill",
    "football (soccer)": "Football", "a gym and bodybuilding": "Bodybuilding", "people dancing": "Dance",
    "a crowd of people": "Crowd", "soldiers": "Soldier", "police officers": "Police officer",
    "musical instruments": "Musical instrument", "playing cards": "Playing card", "tools": "Tool",
    "makeup and lipstick": "Lipstick", "feet": "Foot", "hands": "Hand", "mushrooms": "Mushroom",
    "potato chips": "Potato chip", "sausages": "Sausage", "almonds": "Almond", "walnuts": "Walnut",
    "pistachios": "Pistachio", "raisins": "Raisin", "dates (fruit)": "Date (fruit)", "teeth": "Tooth",
    "fingernails": "Nail (body part)", "muscles": "Muscle", "trousers": "Trousers", "socks": "Sock",
    "slippers": "Slipper", "pajamas": "Pajamas", "leaves": "Leaf", "stairs": "Stairs",
    "an orange (fruit)": "Orange", "tangerines": "Tangerine", "sour cherries": "Cherry",
    "a turkey (bird)": "Turkey", "seashells": "Seashell", "coral reef": "Coral reef",
}

B = background_vector().astype(np.float32)


def load_bin(name: str, magic: bytes) -> np.ndarray:
    raw = (HERE / "assets" / name).read_bytes()
    assert raw[:4] == magic, name
    count, width = struct.unpack("<II", raw[4:12])
    rows = np.frombuffer(raw[12:], dtype=np.uint8).reshape(count, 4 + width)
    scales = rows[:, :4].copy().view("<f4").reshape(-1)
    return (rows[:, 4:].view(np.int8).astype(np.float32) * scales[:, None]).astype(np.float32)


def quantize(v: np.ndarray) -> tuple[np.ndarray, np.float32]:
    """`imgfilter::quantize`, in f32 as Rust computes it."""
    v = v.astype(np.float32)
    scale = np.float32(max(float(np.abs(v).max()), 1e-6)) / np.float32(127.0)
    return np.clip(np.round(v / scale), -127, 127).astype(np.int8), np.float32(scale)


class Text:
    def __init__(self) -> None:
        import onnxruntime as ort

        self.session = ort.InferenceSession(str(VISION / "vision_text.onnx"), providers=["CPUExecutionProvider"])
        self.bpe = Bpe(VISION / "vision_text_vocab.txt", VISION / "vision_text_merges.txt")
        self.name = self.session.get_inputs()[0].name

    def direction(self, phrase: str) -> np.ndarray:
        """`imgtext::try_embed_framed(phrase, Frames::English)`: lowercase, canonical, the six frames summed."""
        phrase = phrase.strip().lower()
        phrase = CANONICAL.get(phrase, phrase)
        total = np.zeros(768, np.float64)
        for template in EN_T:
            out = self.session.run(None, {self.name: self.bpe.encode(template.replace("{}", phrase))})[0]
            total += out.reshape(-1)[:768]
        return (total / np.linalg.norm(total)).astype(np.float32)


def table_rows() -> list[tuple[str, str]]:
    """(english, source) for every visual row of both tables, hand-written first."""
    out = []
    for name in ("imgfilter_subjects.tsv", "imgfilter_lexicon.tsv"):
        for line in (HERE / "assets" / name).read_text(encoding="utf-8").splitlines():
            if line.strip() and not line.startswith("#"):
                kind, english = line.split("\t")[:2]
                if kind == "visual":
                    out.append((english, name))
    return out


def class_of(english: str, by_name: dict[str, str]) -> str | None:
    if english in ALIAS:
        return by_name.get(ALIAS[english].lower())
    if english in by_name:
        return by_name[english]
    for article in ("a ", "an ", "the "):
        if english.startswith(article) and english[len(article):] in by_name:
            return by_name[english[len(article):]]
    return None


PIXELS = "https://open-images-dataset.s3.amazonaws.com"
TRAIN_PER_CLASS = 250


def fetch() -> None:
    """Every validation and test picture with human-verified labels, then the hosted train pictures that bring
    each class with twenty verified positives up to `TRAIN_PER_CLASS` (counting val, test and the pool)."""
    import random
    import urllib.request
    from concurrent.futures import ThreadPoolExecutor

    import concept_data

    def grab(split: str, ids: list[str], directory: pathlib.Path) -> None:
        directory.mkdir(parents=True, exist_ok=True)

        def one(image: str) -> bool:
            out = directory / f"{image}.jpg"
            if out.exists():
                return True
            for attempt in range(3):
                try:
                    with urllib.request.urlopen(f"{PIXELS}/{split}/{image}.jpg", timeout=30) as response:
                        jpeg = concept_data.as_jpeg(response.read())
                    if jpeg is None:
                        return False
                    out.write_bytes(jpeg)
                    return True
                except Exception:  # noqa: BLE001
                    time.sleep(1 + attempt)
            return False

        with ThreadPoolExecutor(64) as pool:
            done = sum(pool.map(one, ids))
        print(f"{split}: {done} of {len(ids)} pictures", flush=True)

    counted: collections.Counter = collections.Counter()
    for split, name, directory in (("validation", "oidv7-val-annotations-human-imagelabels.csv", "oivali"),
                                   ("test", "oidv7-test-annotations-human-imagelabels.csv", "oitest")):
        ids = set()
        with open(OI / name, encoding="utf-8") as handle:
            reader = csv.reader(handle)
            next(reader)
            for image, _, label, confidence in reader:
                ids.add(image)
                if confidence == "1":
                    counted[label] += 1
        grab(split, sorted(ids), ROOT / "img" / directory)
    hosted = set((OI / "train-hosted.txt").read_text().split())
    held = {p.stem for p in (ROOT / "img/pool").glob("*.jpg")}
    positives: dict[str, list[str]] = collections.defaultdict(list)
    with open(OI / "oidv7-train-annotations-human-imagelabels.csv", encoding="utf-8") as handle:
        next(handle)
        for line in handle:
            image, _, label, confidence = line.rstrip("\n").split(",")
            if confidence == "1" and image in hosted:
                positives[label].append(image)
    rng, chosen = random.Random(5), set()
    for label, images in sorted(positives.items(), key=lambda item: len(item[1])):
        if len(images) + counted[label] < 20:
            continue
        need = max(0, TRAIN_PER_CLASS - counted[label])
        have = [i for i in images if i in chosen or i in held]
        rest = [i for i in images if i not in chosen and i not in held]
        rng.shuffle(rest)
        chosen.update(rest[:max(0, need - len(have))])
    (OI / "train_chosen.txt").write_text("\n".join(sorted(chosen)) + "\n")
    grab("train", sorted(chosen), ROOT / "img/oitrain")


def manifest(directory: str) -> None:
    """`vision_embed.py`'s manifest for one fetched directory."""
    names = sorted(p.name for p in (ROOT / "img" / directory).glob("*.jpg"))
    rows = ["path\tgroup\tsource\tsplit"] + [f"concepts/img/{directory}/{n}\t{directory}\toi\teval" for n in names]
    (ROOT / f"{directory}_manifest.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    print(f"{len(names)} rows")


def labels() -> None:
    """Every train label of the train and pool pictures we hold, in one pass over the 58M-row file."""
    wanted = set((OI / "train_chosen.txt").read_text().split())
    wanted.update(p.rsplit("/", 1)[1][:-4] for p in np.load(ROOT / "pool_base384.npz")["path"].astype(str))
    found = collections.defaultdict(list)
    with open(OI / "oidv7-train-annotations-human-imagelabels.csv", encoding="utf-8") as handle:
        next(handle)
        for line in handle:
            image, _, label, confidence = line.rstrip("\n").split(",")
            if image in wanted:
                found[image].append((label, confidence == "1"))
    pickle.dump(dict(found), open(OI / "train_labels_held.pkl", "wb"))
    print(f"labels for {len(found)} of {len(wanted)} train pictures")


class Data:
    def __init__(self) -> None:
        xs, ids = [], []
        for name in EMBEDDINGS:
            z = np.load(ROOT / name)
            xs.append(z["embeddings"].astype(np.float32))
            ids += [p.rsplit("/", 1)[1][:-4] for p in z["path"].astype(str)]
        self.x = np.concatenate(xs)
        self.x /= np.linalg.norm(self.x, axis=1, keepdims=True)
        self.ids = ids
        row = {image: at for at, image in enumerate(ids)}
        self.pos: dict[str, set[int]] = collections.defaultdict(set)
        self.neg: dict[str, set[int]] = collections.defaultdict(set)
        for name in ("oidv7-val-annotations-human-imagelabels.csv", "oidv7-test-annotations-human-imagelabels.csv"):
            with open(OI / name, encoding="utf-8") as handle:
                reader = csv.reader(handle)
                next(reader)
                for image, _, label, confidence in reader:
                    at = row.get(image)
                    if at is not None:
                        (self.pos if confidence == "1" else self.neg)[label].add(at)
        for image, found in pickle.load(open(OI / "train_labels_held.pkl", "rb")).items():
            at = row.get(image)
            if at is not None:
                for label, positive in found:
                    (self.pos if positive else self.neg)[label].add(at)
        for label in self.pos:
            self.neg[label] -= self.pos[label]
        self.held = np.array([int(hashlib.sha1(i.encode()).hexdigest()[0], 16) >= 8 for i in ids])
        self.bank = load_bin("imgfilter_bank.bin", b"IFB1")
        self.pool = load_bin("imgfilter_pool.bin", b"IFP1")
        # The region's own pictures (`imgfilter_region.py`): Iranian life and news, Persian script.
        region = np.load(ROOT / "region_base384.npz")["embeddings"].astype(np.float32)
        self.region = region / np.linalg.norm(region, axis=1, keepdims=True)
        pool_ids = [p.rsplit("/", 1)[1][:-4] for p in np.load(ROOT / "pool_base384.npz")["path"].astype(str)]
        self.pool_row = np.array([row[i] for i in pool_ids])  # pool index -> row in x
        self.bank_bg, self.pool_bg = self.bank @ B, self.pool @ B


def fit(data: Data, start: np.ndarray, positives: np.ndarray, hard: np.ndarray, other: np.ndarray,
        pool_rest: np.ndarray, l2: float = 1e-3) -> np.ndarray:
    """Weighted logistic regression from the text direction: verified positives half the weight, and the other
    half verified negatives, ordinary pictures, the bank, pool photographs and the region's own pictures in
    the proportions of `SHARES`."""
    from scipy.optimize import minimize

    if len(pool_rest) > 8000:  # imgfilter::LEARN_POOL_NEGATIVES, evenly spaced as `blend` takes them
        pool_rest = pool_rest[[at * (len(pool_rest) - 1) // 7999 for at in range(8000)]]
    if len(hard) > 6000:
        hard = hard[np.linspace(0, len(hard) - 1, 6000).astype(int)]
    parts = [data.x[positives], data.x[hard], data.x[other], data.bank, data.pool[pool_rest], data.region]
    share = [0.5, SHARES["verified negatives"] if len(hard) else 0.0, SHARES["ordinary"], SHARES["bank"],
             SHARES["pool"], SHARES["region"]]
    rest = sum(share[1:])
    share = [share[0]] + [s * 0.5 / rest for s in share[1:]]
    X = np.concatenate(parts).astype(np.float32)
    y = np.concatenate([np.ones(len(parts[0])), np.zeros(len(X) - len(parts[0]))]).astype(np.float32)
    w = np.concatenate([np.full(len(p), s / max(len(p), 1)) for p, s in zip(parts, share)]).astype(np.float32)

    def loss(theta):
        weights = theta[:-1].astype(np.float32)
        z = X @ weights + np.float32(theta[-1])
        value = float((w * (np.logaddexp(0, z) - y * z)).sum()) + 0.5 * l2 * float(theta[:-1] @ theta[:-1])
        g = w * (1 / (1 + np.exp(-z)) - y)
        return value, np.concatenate([(X.T @ g).astype(np.float64) + l2 * theta[:-1], [float(g.sum())]])

    theta = minimize(loss, np.concatenate([start.astype(np.float64) * 10, [0.0]]), jac=True, method="L-BFGS-B",
                     options={"maxiter": 400}).x
    head = theta[:-1]
    return (head / np.linalg.norm(head)).astype(np.float32)


def build(data: Data, text: np.ndarray, label: str, train: np.ndarray, rng) -> tuple[np.ndarray, float]:
    """The stored vector and its cut, from the pictures `train` allows."""
    positives = np.array(sorted(a for a in data.pos[label] if train[a]), dtype=int)
    hard = np.array(sorted(a for a in data.neg[label] if train[a]), dtype=int)
    candidates = np.where(train)[0]
    other = rng.choice(candidates, min(6000, len(candidates)), replace=False)
    other = other[~np.isin(other, list(data.pos[label]))][:4000]
    pool_margin = data.pool @ text - data.pool_bg
    ranked = np.lexsort((np.arange(len(pool_margin)), -pool_margin))
    head = fit(data, text, positives, hard, other, ranked[EXCLUDE:])
    keep = np.ones(len(data.pool), bool)
    keep[ranked[:EXCLUDE]] = False
    keep[np.argsort(-(data.pool @ head))[:EXCLUDE]] = False
    keep[np.isin(data.pool_row, list(data.pos[label]))] = False
    rest = np.where(keep)[0]
    over_t = np.concatenate([data.bank @ text - data.bank_bg, data.pool[rest] @ text - data.pool_bg[rest]])
    over_h = np.concatenate([data.bank @ head, data.pool[rest] @ head])
    a, b = SHARE / over_t.std(), (1 - SHARE) / over_h.std()
    # margin = a(e.t - e.B) + b(e.h) = e.(a t + b h + (1 - a) B) - e.B
    q, scale = quantize(a * text + b * head + (1 - a) * B)
    vector = q.astype(np.float32) * scale
    over = np.concatenate([data.bank @ vector - data.bank_bg, data.pool[rest] @ vector - data.pool_bg[rest]])
    over = np.sort(over)[::-1]
    cut = float(over[min(int(len(over) * RATE), len(over) - 1)])
    # Near-misses: the cut also clears all but LOOKALIKE_SHARE of the pictures people verified do not
    # show the thing. The bank holds ordinary pictures, and what a filter got wrong after it was its
    # neighbours — a cello for "oboe", a café table for "wallet", a helicopter for "mechanical fan".
    if len(hard) >= 20:
        near = np.sort(data.x[hard] @ vector - data.x[hard] @ B)[::-1]
        cut = max(cut, float(near[min(int(len(near) * LOOKALIKE_SHARE), len(near) - 1)]))
    cut += MARGIN_SD * float((data.bank @ vector - data.bank_bg).std())
    return vector, cut


def candidates(data: Data) -> list[tuple[str, str]]:
    by_name = {}
    for mid, name in list(csv.reader(open(OI / "oidv7-class-descriptions.csv", encoding="utf-8")))[1:]:
        by_name.setdefault(" ".join(name.lower().split()), mid)
    out, seen = [], set()
    for english, _ in table_rows():
        mid = class_of(english, by_name)
        if mid and english not in seen and len(data.pos[mid]) >= MIN_POSITIVES:
            out.append((english, mid))
            seen.add(english)
    return out


def evaluate(limit: int) -> None:
    data, text = Data(), Text()
    rng = np.random.default_rng(7)
    rows = candidates(data)[:limit]
    print(f"{len(rows)} phrases with >= {MIN_POSITIVES} verified pictures", flush=True)

    everyday = np.load(ROOT / "phrase_eval_dump.npz")
    held_everyday = everyday["x"][everyday["held"]]
    results = collections.defaultdict(list)
    for at, (english, label) in enumerate(rows):
        t = text.direction(english)
        test_pos = np.array(sorted(a for a in data.pos[label] if data.held[a]), dtype=int)
        test_neg = np.array(sorted(a for a in data.neg[label] if data.held[a]), dtype=int)
        if len(test_pos) < 10:
            continue
        vector, cut = build(data, t, label, ~data.held, rng)
        score = lambda e: e @ vector - e @ B  # noqa: E731
        results["recall"].append(float((score(data.x[test_pos]) >= cut).mean()))
        if len(test_neg):
            results["hard"].append(float((score(data.x[test_neg]) >= cut).mean()))
        results["everyday"].append(int((score(held_everyday) >= cut).sum()))
        if at % 50 == 0:
            print(f"[{at}/{len(rows)}] {english}: recall {results['recall'][-1]:.2f}", flush=True)
    print(f"recall {np.mean(results['recall']):.3f}  verified-negative FP {np.mean(results['hard'])*100:.3f}%  "
          f"everyday {sum(results['everyday'])} over {len(results['recall'])} phrases")


_SHARED: dict = {}


def _fit_one(job: tuple[int, str, str]) -> tuple[int, bytes]:
    at, english, label = job
    data, directions = _SHARED["data"], _SHARED["directions"]
    vector, cut = build(data, directions[english], label, np.ones(len(data.x), bool), np.random.default_rng(at))
    q, scale = quantize(vector)
    key = english.encode("utf-8")
    return at, struct.pack("<H", len(key)) + key + struct.pack("<ff", cut, scale) + q.tobytes()


def write() -> None:
    import multiprocessing

    data, text = Data(), Text()
    rows = candidates(data)
    print(f"{len(rows)} phrases with >= {MIN_POSITIVES} verified pictures", flush=True)
    # The text tower runs here, before the fork; the workers only do arithmetic on what they inherit.
    _SHARED["data"] = data
    _SHARED["directions"] = {english: text.direction(english) for english, _ in rows}
    started, pieces = time.time(), {}
    with multiprocessing.get_context("fork").Pool(WORKERS) as pool:
        jobs = [(at, english, label) for at, (english, label) in enumerate(rows)]
        for done, (at, piece) in enumerate(pool.imap_unordered(_fit_one, jobs, chunksize=4)):
            pieces[at] = piece
            if done % 100 == 0:
                print(f"[{done}/{len(rows)} {time.time() - started:.0f}s]", flush=True)
    out = bytearray(b"IFC1" + struct.pack("<II", len(rows), 768))
    for at in range(len(rows)):
        out += pieces[at]
    OUT.write_bytes(bytes(out))
    print(f"wrote {OUT}: {len(rows)} filters, {len(out)} bytes")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["fetch", "manifest", "labels", "eval", "write"])
    parser.add_argument("directory", nargs="?")
    parser.add_argument("--limit", type=int, default=100000)
    args = parser.parse_args()
    {"fetch": fetch, "manifest": lambda: manifest(args.directory), "labels": labels,
     "eval": lambda: evaluate(args.limit), "write": write}[args.command]()
