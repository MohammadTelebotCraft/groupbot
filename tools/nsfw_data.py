#!/usr/bin/env python3
"""The NSFW training corpus: which groups exist, what each one is, and how the drawn half is fetched.

Every tool that trains or measures the NSFW lock (`vision_embed.py`, `train_nsfw_head.py`,
`nsfw_bench.py`) reads the group lists below rather than carrying its own, so «what counts as
pornography» is decided once. The lists are the record of two measured mistakes:

* **The group named `anime_nsfw` was ordinary anime.** `AdwolfCzar/nsfw_scene_animes` is
  fight scenes and girls in dresses — not one explicit frame in 429. A head trained with it as a
  positive learned that anime is pornography and that hentai is not, and vetoed a live hentai gif
  at 0.41 on 2026-09-02. It is `anime_scenes` now, and it is a *negative*.
* **PAD3's `safe` rows named `NSFW_Multidomain_*` are pornography** and are excluded from every
  safe set (`EXCLUDE_SAFE_SOURCES`).

The drawn half comes from `deepghs/anime_dbrating` (Danbooru ratings, CC-BY-4.0, not gated).
The archives are 14–19 GB each and nothing here downloads one: `HfFileSystem` serves HTTP
range reads, so `zipfile` reads the central directory (5 s) and then only the members it was
asked for (~0.8 s each). Danbooru's four ratings map onto the lock's three bands:

    explicit      -> drawn_explicit   positive
    questionable  -> drawn_soft       the drawn «softporn» — measured, never trained on
    sensitive     -> drawn_sensitive  pin-ups, fetish, underwear: measured, never trained on
    general       -> drawn_safe       negative: the ordinary anime that must not be deleted

`sensitive` was a negative for one training run. The head's worst «false positives» were then
looked at: a fetish scene, a pin-up in a painted-on bodysuit, a woman in bed under a suggestive
thought bubble — Danbooru's own definition of the rating is «sexualised but not nude». A lock
about explicitness is right to score those high, and a cut set to spare every one of them
spares real pornography too. So it is measured like `softporn`, and only `general` is what
the head is told is ordinary.

    ../bin/python tools/nsfw_data.py sample            # fills corpus/drawn_* and eval/drawn_*
    ../bin/python tools/nsfw_data.py manifest          # rewrites corpus/manifest.tsv
    ../bin/python tools/nsfw_data.py groups            # prints the lists

Images are stored like the rest of the corpus — RGB JPEG, longest side 800 — and the split is
the same rule the corpus was built with: `heldout` when the SHA-1 of the *original* bytes starts
with a, b or c, `train` otherwise. The eval slice is drawn from a disjoint set of members and
never enters `corpus/`.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import pathlib
import random
import sys
import time
import zipfile
from concurrent.futures import ThreadPoolExecutor

HOME = pathlib.Path.home()
CORPUS = HOME / ".cache/nsfw-train/corpus"
EVAL = HOME / ".cache/nsfw-eval"
EMB = HOME / ".cache/nsfw-train/emb"

# What the head is trained to say «explicit» to.
POSITIVE = ["porn", "drawn_explicit"]
# Revealing rather than explicit, photographic and drawn. Excluded from head training because
# the boundary is what a chat's `SOFT` setting chooses; measured separately.
AMBIGUOUS = ["softporn", "drawn_soft", "drawn_sensitive"]
# Everything a group actually sends that must not be deleted. Stickers, memes, emoji and
# ordinary anime are here because live traffic is those, not photographs.
NEGATIVE = [
    "anime_faces", "anime_scenes", "betting", "cartoon_faces", "drawn_safe", "emoji", "faces",
    "food", "memes", "people", "safe", "screenshots", "stickers", "terrorist", "tg_stickers",
    "violence", "weapon",
]
# The eval directory keeps its own historical names; `nsfw` is x1101/nsfw-full (photographic).
EVAL_POSITIVE = ["porn", "nsfw", "drawn_explicit"]
EVAL_AMBIGUOUS = ["softporn", "drawn_soft", "drawn_sensitive"]
EVAL_NEGATIVE = ["people", "faces", "food", "drawn_safe"]

EXCLUDE_SAFE_SOURCES = ("NSFW_Multidomain_",)

REPO = "datasets/deepghs/anime_dbrating"
ARCHIVES = {
    "drawn_explicit": ["explicit_278119.zip"],
    "drawn_soft": ["questionable_320107.zip"],
    "drawn_sensitive": ["sensitive_341769.zip"],
    "drawn_safe": ["general_341506.zip"],
}
# corpus count, eval count
WANT = {"drawn_explicit": (3000, 300), "drawn_soft": (1500, 150), "drawn_sensitive": (1000, 150), "drawn_safe": (1000, 150)}
MAX_SIDE = 800


def as_jpeg(data: bytes) -> bytes | None:
    from PIL import Image

    try:
        im = Image.open(io.BytesIO(data))
        im.load()
        im = im.convert("RGB")
    except Exception:
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


def split_of(original: bytes) -> str:
    return "heldout" if hashlib.sha1(original).hexdigest()[0] in "abc" else "train"


def open_archive(name: str):
    from huggingface_hub import HfFileSystem

    fs = HfFileSystem()
    handle = fs.open(f"{REPO}/{name}", "rb", block_size=1 << 20)
    return zipfile.ZipFile(handle)


def sample(threads: int, seed: int) -> None:
    rng = random.Random(seed)
    for group, archives in ARCHIVES.items():
        want_corpus, want_eval = WANT[group]
        corpus_dir = CORPUS / group
        eval_dir = EVAL / group
        corpus_dir.mkdir(parents=True, exist_ok=True)
        eval_dir.mkdir(parents=True, exist_ok=True)
        have_corpus = len(list(corpus_dir.glob("*.jpg")))
        have_eval = len(list(eval_dir.glob("*.jpg")))
        if have_corpus >= want_corpus and have_eval >= want_eval:
            print(f"{group}: already {have_corpus}/{have_eval}", flush=True)
            continue
        per_archive_corpus = -(-want_corpus // len(archives))
        per_archive_eval = -(-want_eval // len(archives))
        rows_corpus: list[tuple[str, str]] = []
        rows_eval: list[str] = []
        for name in archives:
            t = time.time()
            z = open_archive(name)
            members = [m for m in z.namelist() if not m.endswith("/")]
            rng.shuffle(members)
            print(f"{group}: {name} {len(members)} members, index {time.time() - t:.0f}s", flush=True)
            # Over-ask by a quarter: some members fail to decode or are too small.
            budget = int((per_archive_corpus + per_archive_eval) * 1.25)
            chosen = members[:budget]
            # One ZipFile handle is not thread safe; each worker opens its own.
            local = {}

            def fetch(member: str) -> tuple[str, bytes | None]:
                import threading

                key = threading.get_ident()
                if key not in local:
                    local[key] = open_archive(name)
                for attempt in range(4):
                    try:
                        return member, local[key].read(member)
                    except Exception as e:  # noqa: BLE001
                        time.sleep(2 * (attempt + 1))
                        local[key] = open_archive(name)
                        last = e
                print(f"  failed {member}: {last}", flush=True)
                return member, None

            got_corpus = got_eval = 0
            t = time.time()
            with ThreadPoolExecutor(threads) as pool:
                for member, data in pool.map(fetch, chosen):
                    if data is None:
                        continue
                    jpeg = as_jpeg(data)
                    if jpeg is None:
                        continue
                    # Eval first, so the untouched set is filled from a disjoint prefix.
                    if got_eval < per_archive_eval and have_eval + got_eval < want_eval:
                        at = have_eval + got_eval
                        (eval_dir / f"{at:05d}.jpg").write_bytes(jpeg)
                        rows_eval.append(f"{group}/{at:05d}.jpg\t{group}\tanime_dbrating\ttest")
                        got_eval += 1
                    elif got_corpus < per_archive_corpus and have_corpus + got_corpus < want_corpus:
                        at = have_corpus + got_corpus
                        (corpus_dir / f"{at:05d}.jpg").write_bytes(jpeg)
                        rows_corpus.append((f"{group}/{at:05d}.jpg", split_of(data)))
                        got_corpus += 1
                    if got_corpus >= per_archive_corpus and got_eval >= per_archive_eval:
                        break
            have_corpus += got_corpus
            have_eval += got_eval
            print(
                f"{group}: {name} corpus +{got_corpus} eval +{got_eval} in {time.time() - t:.0f}s",
                flush=True,
            )
        # The split of a corpus row is kept beside the file so `manifest` can rebuild it.
        with (corpus_dir / "splits.tsv").open("a", encoding="utf-8") as f:
            for path, split in rows_corpus:
                f.write(f"{path}\t{split}\n")
        with (EVAL / "manifest.tsv").open("a", encoding="utf-8") as f:
            for row in rows_eval:
                f.write(row + "\n")
        print(f"{group}: total corpus {have_corpus} eval {have_eval}", flush=True)


def manifest() -> None:
    """Rewrites `corpus/manifest.tsv`: existing rows kept (with `anime_nsfw` renamed), the drawn
    groups appended from their `splits.tsv`."""
    path = CORPUS / "manifest.tsv"
    rows = ["path\tgroup\tsource\tsplit"]
    seen = set()
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines()[1:]:
            cols = line.split("\t")
            if len(cols) < 4 or cols[0] in seen:
                continue
            cols[0] = cols[0].replace("anime_nsfw/", "anime_scenes/")
            cols[1] = "anime_scenes" if cols[1] == "anime_nsfw" else cols[1]
            if cols[1].startswith("drawn_"):
                continue
            if not (CORPUS / cols[0]).exists():
                continue
            seen.add(cols[0])
            rows.append("\t".join(cols[:4]))
    for group in ARCHIVES:
        splits = CORPUS / group / "splits.tsv"
        if not splits.exists():
            continue
        for line in splits.read_text(encoding="utf-8").splitlines():
            p, split = line.split("\t")
            if p in seen or not (CORPUS / p).exists():
                continue
            seen.add(p)
            rows.append(f"{p}\t{group}\tanime_dbrating\t{split}")
    path.write_text("\n".join(rows) + "\n", encoding="utf-8")
    from collections import Counter

    groups = Counter(r.split("\t")[1] for r in rows[1:])
    print(f"{len(rows) - 1} rows")
    for g, n in sorted(groups.items()):
        print(f"  {n:6d} {g}")


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sample")
    s.add_argument("--threads", type=int, default=16)
    s.add_argument("--seed", type=int, default=0)
    sub.add_parser("manifest")
    sub.add_parser("groups")
    args = parser.parse_args()
    if args.cmd == "sample":
        old = CORPUS / "anime_nsfw"
        if old.exists() and not (CORPUS / "anime_scenes").exists():
            old.rename(CORPUS / "anime_scenes")
            print("renamed anime_nsfw -> anime_scenes")
        sample(args.threads, args.seed)
        manifest()
    elif args.cmd == "manifest":
        manifest()
    else:
        print("positive ", POSITIVE)
        print("ambiguous", AMBIGUOUS)
        print("negative ", NEGATIVE)
    return 0


if __name__ == "__main__":
    sys.exit(main())
