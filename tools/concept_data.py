#!/usr/bin/env python3
"""The concept-lock corpus: what each of the six concepts is, what it must not be, and where the
pictures come from.

The six concept locks («قفل سیگار», «قفل مشروب», «قفل اسلحه», «قفل قمار», «قفل مواد», «قفل خون»)
used to be prompt margins: a caption's direction minus an ordinary-photograph baseline, then a
stack of hand-tuned contrasts on top — chainsaw, sword, scissors, tool, red wine, red drink,
cannabis bud, dried herb — each one added after a lookalike was deleted. That is the same shape
the NSFW arbiter had before its head, and it ends the same way: every new lookalike is a new
contrast. `train_concept_heads.py` fits one logistic direction per concept instead, on pictures,
and this file decides what those pictures are, once, so the trainer and the bench cannot
disagree about what counts.

**Open Images V7, human-verified image-level labels.** `1.0` is a person confirming the object is
there; `0.0` is a person confirming it is *not*, on an image a classifier had proposed for it —
so the verified negatives of a class are exactly the hard negatives a prompt margin gets wrong.
Three kinds of picture per concept:

    positive   verified 1.0 for any of the concept's classes
    hard       verified 0.0 for one of them, and 1.0 for none of them or of its ambiguous classes
    lookalike  verified 1.0 for a class the lock must leave alone (chainsaw, juice, chalk ...)

Classes in `AMBIGUOUS` are never trained on either way and are reported on their own row, the
way `nsfw_data.py` treats `softporn`: a kitchen knife, a toy gun, a pharmacy shelf are where a
group's policy decides, not the model.

The policy, as the user set it for the weapon lock on 2026-09-23: **guns and knives**; a chainsaw,
a sword and cutlery are allowed. Ammunition without a gun is outside the lock (that is what the
old `WEAPON_AMMO_ONLY` contrast encoded).

Splits: Open Images `train` trains; `validation` and `test` are the untouched evaluation set.
Groups reused from the NSFW corpus keep that corpus's own `train`/`heldout` split.

    ../bin/python tools/concept_data.py fetch      # labels -> ~/.cache/concepts/oi, pictures -> img/
    ../bin/python tools/concept_data.py manifest   # rewrites ~/.cache/concepts/manifest.tsv
    ../bin/python tools/concept_data.py groups

Pictures are stored like the NSFW corpus: RGB JPEG, longest side 800.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import pathlib
import random
import sys
import time
import urllib.error
import urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor

HOME = pathlib.Path.home()
ROOT = HOME / ".cache/concepts"
OI = ROOT / "oi"
IMG = ROOT / "img"
NSFW_CORPUS = HOME / ".cache/nsfw-train/corpus"
LABELS = "https://storage.googleapis.com/openimages/v7"
PIXELS = "https://open-images-dataset.s3.amazonaws.com"
# The image-level labels cover all nine million `train` pictures, but the public bucket holds only
# the 1.7 million «boxable» ones; the rest answer 404. Validation and test are hosted whole.
HOSTED = "https://storage.googleapis.com/openimages/2018_04/train/train-images-boxable.csv"
MAX_SIDE = 800

# Slot order is `concepts::CONCEPTS`. Every name is an Open Images display name.
CONCEPTS = {
    "c_cig": {
        "positive": ["Cigarette", "Cigar", "Smoking", "Tobacco", "Tobacco products", "Electronic cigarette"],
        "lookalike": ["Chalk", "Crayon", "Candle", "Incense", "Drinking straw", "Chopsticks", "Pen",
                      "Pencil", "Lipstick", "Fog", "Steam", "Fire"],
        "ambiguous": ["Pipe", "Ashtray", "Smoke", "Cannabis"],
        "corpus": [],
    },
    "c_alc": {
        "positive": ["Alcoholic beverage", "Alcohol", "Beer", "Beer bottle", "Wine", "Wine bottle",
                     "Red wine", "Whisky", "Vodka", "Liqueur", "Cocktail", "Champagne",
                     "Distilled beverage"],
        "lookalike": ["Juice", "Soft drink", "Cola", "Tea", "Water", "Ketchup"],
        "ambiguous": ["Drink", "Bottle"],
        "corpus": [],
    },
    "c_gun": {
        "positive": ["Gun", "Handgun", "Rifle", "Shotgun", "Firearm", "Assault rifle", "Machine gun",
                     "Sniper rifle", "Pistol", "Revolver", "Air gun", "Knife", "Dagger",
                     "Bowie knife", "Hunting knife"],
        "lookalike": ["Chainsaw", "Sword", "Scissors", "Fork", "Spoon", "Table knife", "Tool",
                      "Screwdriver", "Ammunition", "Bullet", "Chopsticks"],
        "ambiguous": ["Kitchen knife", "Utility knife", "Machete", "Scalpel", "Water gun", "Toy",
                      "Weapon", "Violence"],
        "corpus": ["weapon", "terrorist"],
    },
    "c_bet": {
        "positive": ["Casino", "Gambling", "Poker", "Poker table", "Slot machine"],
        "lookalike": ["Board game", "Chess"],
        "ambiguous": ["Card game", "Playing card", "Dice", "Horse racing"],
        "corpus": ["betting"],
    },
    "c_drg": {
        # Open Images has no usable drug class: «Joint» is verified on knees and elbows and
        # «Cannabis» on potted plants (looked at, 2026-10-14). Positives come from `COMMONS`.
        "positive": ["Bong", "Heroin", "Drug"],
        "lookalike": ["Herb", "Spinach", "Flour", "Sugar", "Tea", "Chalk"],
        "ambiguous": ["Syringe", "Pill", "Medicine", "Pharmaceutical drug", "Capsule", "Tablet",
                      "Smoking", "Cigarette", "Tobacco", "Tobacco products", "Pipe", "Cannabis"],
        "corpus": [],
    },
    "c_bld": {
        "positive": ["Blood"],
        "lookalike": ["Ketchup", "Red wine", "Juice", "Lipstick", "Strawberry", "Tomato", "Paint"],
        "ambiguous": ["Injury", "Bandage", "Violence", "Syringe", "Scalpel"],
        "corpus": [],
    },
}
SLOTS = list(CONCEPTS)

# What a group actually sends that no concept lock may delete. Reused from the NSFW corpus so the
# heads see the stickers, memes, screenshots and anime that live traffic is made of, not only
# photographs. `violence` is not here: fights carry blood and weapons often enough to be
# ambiguous for two of the six.
CORPUS_NEGATIVE = [
    "anime_faces", "anime_scenes", "cartoon_faces", "drawn_safe", "emoji", "faces", "food", "memes",
    "people", "safe", "screenshots", "stickers", "tg_stickers",
]

# Per concept: how many of each kind are fetched at most, across every Open Images split.
CAP = {"positive": 3200, "hard": 2400, "lookalike": 560}

# Labelled sets beside Open Images, for the concepts its hosted pictures cover thinly. Each folder
# is `(repo, folder prefix, our split, {slot: label})`; a slot not named is excluded (-1), because
# a set built for one question says nothing about the other five.
#
# `smoking_img_final` is people smoking against people *not* smoking — eating, on the phone, a
# hand at the mouth — which is exactly the gesture a cigarette lock must not fire on.
SUPPLEMENTS = {
    "smoking": ("ccclllwww/smoking_img_final", [
        ("images/train/smoking/", "train", {"c_cig": 1}),
        ("images/train/not_smoking/", "train", {"c_cig": 0}),
        ("images/test/smoking/", "eval", {"c_cig": 1}),
        ("images/test/not_smoking/", "eval", {"c_cig": 0}),
    ]),
}


# Wikimedia Commons categories, for a thing Open Images has no class for at all: «قلیان» is one
# of the cigarette lock's own names, and not one hookah is labelled there. `(categories, {slot:
# label})`; the split is the file name's hash, a quarter to evaluation, like Open Images' train.
COMMONS = {
    "hookah": (["Category:Hookahs", "Category:People smoking the hookah", "Category:Hookah lounges"],
               {"c_cig": 1}),
    # Open Images' «Joint» is a knee and its «Cannabis» is a potted plant; neither is used. These
    # are the drug pictures a group sees: buds, joints, hashish, bongs, powder and needles. A
    # chemical-structure PNG is dropped by the JPEG filter; the rest is checked by eye.
    "drugs": (["Category:Cannabis buds", "Category:Dried cannabis buds", "Category:Joints (cannabis)",
               "Category:Cannabis smoking", "Category:Blunt", "Category:Hashish", "Category:Bongs",
               "Category:Crack cocaine", "Category:People using crack cocaine", "Category:Heroin injection",
               "Category:Heroin paraphernalia", "Category:Cocaine hydrochloride"],
              {"c_drg": 1, "c_cig": -1}),
    "blood": (["Category:Blood", "Category:Bleeding", "Category:Nosebleed", "Category:Bloodstains",
               "Category:Lacerations", "Category:Gunshot wounds"],
              {"c_bld": 1}),
}
COMMONS_API = "https://commons.wikimedia.org/w/api.php"
COMMONS_AGENT = {"User-Agent": "groupbot-concept-corpus/1.0 (offline evaluation of a moderation model)"}


def fetch_commons() -> None:
    import json
    import urllib.parse

    def patient(url: str) -> bytes:
        # Commons answers 429 to anything brisk; back off and try again rather than skip.
        for attempt in range(6):
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers=COMMONS_AGENT), timeout=60) as r:
                    return r.read()
            except urllib.error.HTTPError as e:
                if e.code != 429:
                    raise
                time.sleep(10 * (attempt + 1))
        raise RuntimeError(f"rate limited: {url}")

    def query(params: dict) -> dict:
        url = COMMONS_API + "?" + urllib.parse.urlencode({**params, "format": "json"})
        return json.loads(patient(url))

    for name, (categories, _) in COMMONS.items():
        out_dir = IMG / "commons" / name
        out_dir.mkdir(parents=True, exist_ok=True)
        urls = {}
        for category in categories:
            params = {"action": "query", "generator": "categorymembers", "gcmtitle": category,
                      "gcmtype": "file", "gcmlimit": 500, "prop": "imageinfo", "iiprop": "url|mime",
                      "iiurlwidth": MAX_SIDE}
            pages = query(params).get("query", {}).get("pages", {})
            for page in pages.values():
                info = (page.get("imageinfo") or [{}])[0]
                if info.get("mime") in ("image/jpeg", "image/png") and info.get("thumburl"):
                    urls[page["title"]] = info["thumburl"]
        got = 0
        for title, url in sorted(urls.items()):
            out = out_dir / (hashlib.sha1(title.encode()).hexdigest()[:16] + ".jpg")
            if out.exists():
                got += 1
                continue
            try:
                jpeg = as_jpeg(patient(url))
            except Exception:  # noqa: BLE001
                continue
            if jpeg is not None:
                out.write_bytes(jpeg)
                got += 1
            time.sleep(1.0)
        print(f"commons/{name}: {got} of {len(urls)}")


def commons_rows() -> list[tuple[str, str, str, list[int], list[str]]]:
    rows = []
    for name, (_, labels) in COMMONS.items():
        for path in sorted((IMG / "commons" / name).glob("*.jpg")):
            ours = "eval" if path.stem[0] in "0123" else "train"
            y = [labels.get(s, -1) for s in SLOTS]
            rows.append((f"concepts/img/commons/{name}/{path.name}", ours, f"commons_{name}", y,
                         [f"supplement:commons/{name}" for _ in SLOTS]))
    return rows


def fetch_supplements() -> None:
    from huggingface_hub import snapshot_download

    for name, (repo, folders) in SUPPLEMENTS.items():
        try:
            local = pathlib.Path(snapshot_download(repo, repo_type="dataset",
                                                   allow_patterns=[f + "*" for f, _, _ in folders]))
        except Exception as error:  # noqa: BLE001
            # The Hub rate-limits unauthenticated snapshots; every file that did arrive is whole
            # in the cache, so use those rather than nothing.
            snapshots = sorted((HOME / ".cache/huggingface/hub" / f"datasets--{repo.replace('/', '--')}"
                                / "snapshots").glob("*"))
            if not snapshots:
                raise
            local = snapshots[-1]
            print(f"{name}: {error.__class__.__name__}; using the partial cache at {local}")
        for folder, _, _ in folders:
            out_dir = IMG / name / folder.strip("/").replace("/", "_")
            out_dir.mkdir(parents=True, exist_ok=True)
            if not (local / folder).exists():
                continue
            for src in sorted((local / folder).iterdir()):
                out = out_dir / (src.stem + ".jpg")
                if out.exists():
                    continue
                jpeg = as_jpeg(src.read_bytes())
                if jpeg is not None:
                    out.write_bytes(jpeg)
        print(f"{name}: " + ", ".join(
            f"{f} {len(list((IMG / name / f.strip('/').replace('/', '_')).glob('*.jpg')))}" for f, _, _ in folders))


def supplement_rows() -> list[tuple[str, str, str, list[int], list[str]]]:
    rows = []
    for name, (_, folders) in SUPPLEMENTS.items():
        for folder, ours, labels in folders:
            sub = folder.strip("/").replace("/", "_")
            for path in sorted((IMG / name / sub).glob("*.jpg")):
                y = [labels.get(s, -1) for s in SLOTS]
                kinds = [f"supplement:{name}/{sub.split('_', 2)[-1]}" for _ in SLOTS]
                rows.append((f"concepts/img/{name}/{sub}/{path.name}", ours, name, y, kinds))
    return rows


def split_of(image: str, oi_split: str) -> str:
    """Open Images `validation` and `test` are evaluation; so is a quarter of `train`, chosen by
    the image id's hash — cigarettes, drugs and blood have too few verified pictures outside
    `train` to measure anything on."""
    if oi_split != "train":
        return "eval"
    return "eval" if hashlib.sha1(image.encode()).hexdigest()[0] in "0123" else "train"


def label_files() -> dict[str, pathlib.Path]:
    return {
        "train": OI / "train-filtered.csv",
        "validation": OI / "oidv7-val-annotations-human-imagelabels-filtered.csv",
        "test": OI / "oidv7-test-annotations-human-imagelabels-filtered.csv",
    }


def all_names() -> set[str]:
    out = set()
    for c in CONCEPTS.values():
        for kind in ("positive", "lookalike", "ambiguous"):
            out.update(c[kind])
    return out


def fetch_labels() -> None:
    """The training label file is 2.7 GB; it is streamed and only rows for the classes above are
    kept. Validation and test are small and kept whole."""
    OI.mkdir(parents=True, exist_ok=True)
    desc = OI / "oidv7-class-descriptions.csv"
    if not desc.exists():
        urllib.request.urlretrieve(f"{LABELS}/oidv7-class-descriptions.csv", desc)
    ids = {r[1]: r[0] for r in csv.reader(desc.open(encoding="utf-8")) if len(r) >= 2}
    wanted = {ids[n] for n in all_names() if n in ids}
    missing = sorted(n for n in all_names() if n not in ids)
    if missing:
        print("not in Open Images:", ", ".join(missing))
    sources = {
        "train": "oidv7-train-annotations-human-imagelabels.csv",
        "validation": "oidv7-val-annotations-human-imagelabels.csv",
        "test": "oidv7-test-annotations-human-imagelabels.csv",
    }
    hosted = OI / "train-hosted.txt"
    if not hosted.exists():
        print("streaming the hosted train image ids", flush=True)
        tmp = hosted.with_suffix(".part")
        with urllib.request.urlopen(HOSTED) as r, tmp.open("w") as f:
            for raw in io.TextIOWrapper(r, encoding="utf-8"):
                image = raw.split(",", 1)[0]
                if image and image != "ImageID":
                    f.write(image + "\n")
        tmp.rename(hosted)
    for split, out in label_files().items():
        if out.exists():
            continue
        print(f"streaming {sources[split]}", flush=True)
        tmp = out.with_suffix(".part")
        with urllib.request.urlopen(f"{LABELS}/{sources[split]}") as r, tmp.open("w") as f:
            for raw in io.TextIOWrapper(r, encoding="utf-8"):
                cols = raw.split(",")
                if len(cols) >= 4 and cols[2] in wanted:
                    f.write(raw)
        tmp.rename(out)


def read_labels() -> tuple[dict[str, str], dict[str, dict[str, dict[str, float]]]]:
    """`name_of[label id]`, and `verified[split][image id][name] = 1.0 | 0.0`."""
    desc = OI / "oidv7-class-descriptions.csv"
    name_of = {r[0]: r[1] for r in csv.reader(desc.open(encoding="utf-8")) if len(r) >= 2}
    keep = all_names()
    verified: dict[str, dict[str, dict[str, float]]] = {}
    for split, path in label_files().items():
        table: dict[str, dict[str, float]] = defaultdict(dict)
        for cols in csv.reader(path.open(encoding="utf-8")):
            if len(cols) < 4 or cols[2] not in name_of:
                continue
            name = name_of[cols[2]]
            if name in keep:
                table[cols[0]][name] = float(cols[3])
        verified[split] = table
    return name_of, verified


def choose(verified, seed: int = 0) -> list[tuple[str, str, str, str]]:
    """`(image id, oi split, our split, reason)` for every picture to fetch. `reason` is
    `<slot>:<kind>:<class>` — a picture chosen for one concept is still labelled for all six by
    `labels_for`, so it is fetched once whichever concept asked first."""
    rng = random.Random(seed)
    hosted = set((OI / "train-hosted.txt").read_text().split())
    pool = sorted((image, oi_split) for oi_split, table in verified.items() for image in table
                  if oi_split != "train" or image in hosted)
    rng.shuffle(pool)
    chosen: dict[tuple[str, str], str] = {}
    for slot, c in CONCEPTS.items():
        pos, amb, look = set(c["positive"]), set(c["ambiguous"]), set(c["lookalike"])
        taken: dict[str, int] = defaultdict(int)
        for image, oi_split in pool:
            labels = verified[oi_split][image]
            yes = {n for n, v in labels.items() if v == 1.0}
            no = {n for n, v in labels.items() if v == 0.0}
            if yes & pos:
                kind, cls, key = "positive", sorted(yes & pos)[0], "positive"
            elif yes & amb:
                continue
            elif no & pos:
                kind, cls, key = "hard", sorted(no & pos)[0], "hard"
            elif yes & look:
                cls = sorted(yes & look)[0]
                kind, key = "lookalike", f"look:{cls}"
            else:
                continue
            if taken[key] >= CAP[kind]:
                continue
            taken[key] += 1
            chosen.setdefault((image, oi_split), f"{slot}:{kind}:{cls}")
    return [(image, oi_split, split_of(image, oi_split), why)
            for (image, oi_split), why in chosen.items()]


def as_jpeg(data: bytes) -> bytes | None:
    from PIL import Image

    try:
        im = Image.open(io.BytesIO(data))
        im.load()
        im = im.convert("RGB")
    except Exception:  # noqa: BLE001
        return None
    w, h = im.size
    if min(w, h) < 64:
        return None
    if max(w, h) > MAX_SIDE:
        s = MAX_SIDE / max(w, h)
        im = im.resize((max(1, round(w * s)), max(1, round(h * s))), Image.Resampling.BICUBIC)
    out = io.BytesIO()
    im.save(out, "JPEG", quality=90)
    return out.getvalue()


def fetch_pixels(threads: int) -> None:
    _, verified = read_labels()
    todo = choose(verified)
    IMG.mkdir(parents=True, exist_ok=True)
    folder = {"train": "train", "validation": "validation", "test": "test"}

    def one(row):
        image, oi_split, _, _ = row
        out = IMG / folder[oi_split] / f"{image}.jpg"
        if out.exists():
            return True
        out.parent.mkdir(parents=True, exist_ok=True)
        for attempt in range(3):
            try:
                with urllib.request.urlopen(f"{PIXELS}/{oi_split}/{image}.jpg", timeout=30) as r:
                    data = r.read()
                jpeg = as_jpeg(data)
                if jpeg is None:
                    return False
                out.write_bytes(jpeg)
                return True
            except urllib.error.HTTPError as e:
                if e.code in (403, 404):
                    return False
                time.sleep(1 + attempt)
            except Exception:  # noqa: BLE001
                time.sleep(1 + attempt)
        return False

    print(f"{len(todo)} pictures chosen", flush=True)
    t = time.time()
    ok = 0
    with ThreadPoolExecutor(threads) as pool:
        for i, got in enumerate(pool.map(one, todo)):
            ok += bool(got)
            if i % 1000 == 999:
                print(f"  {i + 1}/{len(todo)} ok {ok} {(i + 1) / (time.time() - t):.1f}/s", flush=True)
    print(f"fetched {ok}/{len(todo)} in {time.time() - t:.0f}s", flush=True)


def labels_for(names: dict[str, float]) -> list[int]:
    """Per slot: 1 positive, 0 negative, -1 excluded. A verified-negative or unlabelled class
    counts as negative; any verified ambiguous class excludes the picture from that concept."""
    yes = {n for n, v in names.items() if v == 1.0}
    out = []
    for c in CONCEPTS.values():
        if yes & set(c["positive"]):
            out.append(1)
        elif yes & set(c["ambiguous"]):
            out.append(-1)
        else:
            out.append(0)
    return out


def kind_for(names: dict[str, float], slot: str) -> str:
    c = CONCEPTS[slot]
    yes = {n for n, v in names.items() if v == 1.0}
    no = {n for n, v in names.items() if v == 0.0}
    if yes & set(c["positive"]):
        return "positive"
    if yes & set(c["ambiguous"]):
        return "ambiguous:" + sorted(yes & set(c["ambiguous"]))[0]
    if yes & set(c["lookalike"]):
        return "lookalike:" + sorted(yes & set(c["lookalike"]))[0]
    if no & set(c["positive"]):
        return "hard"
    return "other"


def manifest() -> None:
    """`path  split  source  <slot labels...>  <slot kinds...>` with paths relative to `~/.cache`,
    so one manifest spans this corpus and the NSFW one."""
    _, verified = read_labels()
    lines = ["path\tsplit\tsource\t" + "\t".join(SLOTS) + "\t" + "\t".join(f"{s}_kind" for s in SLOTS)]
    count = defaultdict(lambda: [0, 0, 0])
    for oi_split, table in verified.items():
        for path in sorted((IMG / oi_split).glob("*.jpg")):
            ours = split_of(path.stem, oi_split)
            names = table.get(path.stem, {})
            y = labels_for(names)
            kinds = [kind_for(names, s) for s in SLOTS]
            lines.append(f"concepts/img/{oi_split}/{path.name}\t{ours}\toi\t"
                         + "\t".join(map(str, y)) + "\t" + "\t".join(kinds))
            for s, v in zip(SLOTS, y):
                count[(s, ours)][v if v >= 0 else 2] += 1
    for rel, ours, name, y, kinds in supplement_rows() + commons_rows():
        lines.append(f"{rel}\t{ours}\t{name}\t" + "\t".join(map(str, y)) + "\t" + "\t".join(kinds))
        for s, v in zip(SLOTS, y):
            count[(s, ours)][v if v >= 0 else 2] += 1
    nsfw_manifest = NSFW_CORPUS / "manifest.tsv"
    for line in nsfw_manifest.read_text(encoding="utf-8").splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) < 4:
            continue
        rel, group, split = cols[0], cols[1], cols[3]
        ours = "train" if split == "train" else "eval"
        y, kinds = [], []
        for s in SLOTS:
            if group in CONCEPTS[s]["corpus"]:
                y.append(1)
                kinds.append(f"corpus:{group}")
            elif group in CORPUS_NEGATIVE:
                y.append(0)
                kinds.append(f"corpus:{group}")
            else:
                y.append(-1)
                kinds.append(f"corpus:{group}")
        if all(v == -1 for v in y):
            continue
        lines.append(f"nsfw-train/corpus/{rel}\t{ours}\t{group}\t" + "\t".join(map(str, y)) + "\t" + "\t".join(kinds))
        for s, v in zip(SLOTS, y):
            count[(s, ours)][v if v >= 0 else 2] += 1
    out = ROOT / "manifest.tsv"
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {out}: {len(lines) - 1} rows")
    # The same rows in `vision_embed.py`'s layout (`path group source split`, relative to
    # `~/.cache`), so the general model embeds this corpus exactly the way it embeds the NSFW one.
    embed = ["path\tgroup\tsource\tsplit"]
    for line in lines[1:]:
        cols = line.split("\t")
        embed.append(f"{cols[0]}\t{cols[2]}\t{cols[2]}\t{cols[1]}")
    (ROOT / "embed_manifest.tsv").write_text("\n".join(embed) + "\n", encoding="utf-8")
    for s in SLOTS:
        tr, ev = count[(s, "train")], count[(s, "eval")]
        print(f"  {s}: train {tr[1]} pos / {tr[0]} neg / {tr[2]} excluded; eval {ev[1]} / {ev[0]} / {ev[2]}")


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    f = sub.add_parser("fetch")
    f.add_argument("--threads", type=int, default=48)
    sub.add_parser("supplements")
    sub.add_parser("manifest")
    sub.add_parser("groups")
    args = parser.parse_args()
    if args.cmd == "fetch":
        fetch_labels()
        fetch_pixels(args.threads)
        fetch_commons()
        fetch_supplements()
        manifest()
    elif args.cmd == "supplements":
        fetch_commons()
        fetch_supplements()
        manifest()
    elif args.cmd == "manifest":
        manifest()
    else:
        for s, c in CONCEPTS.items():
            print(s)
            for k, v in c.items():
                print(f"  {k:10} {', '.join(v)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
