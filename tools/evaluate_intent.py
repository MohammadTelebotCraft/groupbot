#!/usr/bin/env python3
"""Measures «قفل خرید و فروش» against a labeled set, and prints where the limit should sit.

The lock's whole promise is precision, so the eval set is built around the failure that
matters: `tools/data/intent_eval.tsv` holds three classes —

    sell    a transaction: offering, seeking, renting, ordering, taking payment
    hard    mentions commerce without transacting — yesterday's purchase, price talk,
            news, a quoted ad inside a warning, a refusal
    safe    ordinary chat

`hard` is the class the margin has to clear. A sweep that only looked at `safe` would repeat
the food-photographs mistake the NSFW evaluation already paid for: a 0% false-positive rate
against negatives that share nothing with the real complaint.

The battery (`intent_battery.tsv`) carries a third column, the *kind* of message, and when a
file has one the report breaks recall and false positives down by kind.

Every constant is read back out of the Rust source rather than retyped — `trade.rs`'s limit,
its band, its floor and its lexical net — so this report cannot drift from the code it
describes. The model, the vocabulary and the frame names are read from the *shipped files*
(`--files`, default `target/release`), not from the checkpoint, for the same reason. The whole
runtime pipeline is mirrored here — descramble, net, marker, evidence floor, the small
classifier, the escalation band into the big one, the bridge — and the Rust battery test runs
the same rows through the real one, which is what keeps the two honest.

    ../bin/python tools/evaluate_intent.py --files ./out
    ../bin/python tools/evaluate_intent.py --files ./out --data tools/data/intent_battery.tsv
"""

from __future__ import annotations

import argparse
import pathlib
import re
import statistics
import sys

import numpy as np

TOOLS = pathlib.Path(__file__).resolve().parent
ROOT = TOOLS.parent
sys.path.insert(0, str(TOOLS))

from export_intent import PREFIX, TOKENS, Unigram  # noqa: E402


# ---------------------------------------------------------------------------------------
# the Rust source as the single source of truth
# ---------------------------------------------------------------------------------------


def rust_source(name: str) -> str:
    return (ROOT / "src" / "handlers" / name).read_text(encoding="utf-8")


def rust_const(source: str, name: str) -> float:
    found = re.search(rf"const {name}: \w+ = (-?[0-9.]+)", source)
    if not found:
        sys.exit(f"could not find `{name}` in the Rust source")
    return float(found.group(1))


def rust_range(source: str, name: str) -> tuple[int, int]:
    found = re.search(rf"const {name}: \(u32, u32\) = \((\d+), (\d+)\)", source)
    if not found:
        sys.exit(f"could not find `{name}` in the Rust source")
    return int(found.group(1)), int(found.group(2))


def rust_marks(source: str) -> list[str]:
    """The lexical net, lifted from `trade.rs` so the recall measured here is the recall the
    bot has."""
    found = re.search(r"const MARKS: &\[&str\] = &\[(.*?)\];", source, re.S)
    if not found:
        sys.exit("could not find `MARKS` in trade.rs")
    return re.findall(r'"([^"]+)"', found.group(1))


def rust_listing_words(source: str) -> list[str]:
    """`trade::LISTING_WORDS`, lifted from the source like `MARKS` — a hardcoded mirror here
    silently under-marked the moment the Rust list grew."""
    found = re.search(r"const LISTING_WORDS: &\[&str\] = &\[(.*?)\];", source, re.S)
    if not found:
        sys.exit("could not find `LISTING_WORDS` in trade.rs")
    return re.findall(r'"([^"]+)"', found.group(1))


class Constants:
    """Everything `trade.rs` decides with, read once."""

    def __init__(self):
        trade = rust_source("trade.rs")
        self.marks = rust_marks(trade)
        self.listing_words = rust_listing_words(trade)
        self.floor = int(rust_const(trade, "BRIDGE_FLOOR"))
        self.default_limit = int(rust_const(trade, "DEFAULT_LIMIT"))
        self.limit_range = rust_range(trade, "LIMIT_RANGE")
        self.escalate_from = rust_const(trade, "ESCALATE_FROM")
        self.escalate_under = int(rust_const(trade, "ESCALATE_UNDER"))


# ---------------------------------------------------------------------------------------
# the deterministic layers, mirrored
# ---------------------------------------------------------------------------------------

PERSIAN_DIGITS = str.maketrans("۰۱۲۳۴۵۶۷۸۹٠١٢٣٤٥٦٧٨٩", "01234567890123456789")


def luhn(digits: list[int]) -> bool:
    total = 0
    for at, d in enumerate(reversed(digits)):
        if at % 2 == 1:
            d *= 2
            if d > 9:
                d -= 9
        total += d
    return total % 10 == 0


def has_card_number(digits: str) -> bool:
    """`trade::has_card_number`: exactly sixteen Luhn-valid digits across single separators."""
    run: list[int] = []
    between = False

    def flush() -> bool:
        card = len(run) == 16 and luhn(run)
        run.clear()
        return card

    for c in digits:
        if c.isdigit() and ord(c) < 128:
            run.append(int(c))
            between = False
            continue
        if c in " -_." and not between and run:
            between = True
            continue
        between = False
        if flush():
            return True
    return flush()


ARABIC_DIGITS = set("٠١٢٣٤٥٦٧٨٩۰۱۲۳۴۵۶۷۸۹")
SEPARATORS = set(".·-_*`'٬")


def _arabic_letter(c: str) -> bool:
    return (
        "؀" <= c <= "ۿ"
        or "ݐ" <= c <= "ݿ"
        or "ࢠ" <= c <= "ࣿ"
        or "ﭐ" <= c <= "﷿"
        or "ﹰ" <= c <= "﻿"
    ) and c not in ARABIC_DIGITS


def descramble(text: str) -> tuple[str, bool]:
    """`intent::descramble`, mirrored: strip in-word separators and tatweel, collapse
    held-down letters, join spaced-out spelling — and say whether the text was tampered."""
    chars = list(text)
    out: list[str] = []
    tampered = False
    fold = {"ي": "ی", "ى": "ی", "ك": "ک"}
    for at, c in enumerate(chars):
        c = fold.get(c, c)
        if c == "ـ":
            tampered = True
            continue
        if c in SEPARATORS and at > 0:
            prev = next((p for p in reversed(chars[:at]) if p not in SEPARATORS), "")
            nxt = next((n for n in chars[at + 1 :] if n not in SEPARATORS), "")
            if _arabic_letter(prev) and _arabic_letter(nxt):
                tampered = True
                continue
        out.append(c)
    squeezed: list[str] = []
    at = 0
    while at < len(out):
        end = at
        while end < len(out) and out[end] == out[at]:
            end += 1
        squeezed.extend(out[at:end] if end - at < 3 else out[at])
        at = end
    joined = "".join(squeezed)
    words = joined.split(" ")
    single = lambda w: len(w) == 1 and _arabic_letter(w)
    if sum(1 for w in words if single(w)) >= 3:
        rebuilt: list[str] = []
        run: list[str] = []
        for word in words + [""]:
            if single(word):
                run.append(word)
                continue
            if len(run) >= 3:
                tampered = True
                rebuilt.append("".join(run))
            else:
                rebuilt.extend(run)
            run = []
            if word:
                rebuilt.append(word)
        joined = " ".join(rebuilt)
    return joined, tampered


def scrubbed(text: str) -> str:
    """The lowered, joiner-free, descrambled form the watcher computes once."""
    return descramble(text.lower().replace("‌", ""))[0]


def suspicious(text: str, marks: list[str]) -> bool:
    lowered, tampered = descramble(text.lower().replace("‌", ""))
    return (
        tampered
        or any(mark in lowered for mark in marks)
        or has_card_number(text.translate(PERSIAN_DIGITS))
    )


def standalone(text: str, word: str) -> int | None:
    """`trade::standalone`, reimplemented: the end of the first word-boundary occurrence."""
    start = 0
    while (at := text.find(word, start)) >= 0:
        end = at + len(word)
        before = text[at - 1] if at > 0 else " "
        after = text[end] if end < len(text) else " "
        if not before.isalnum() and not after.isalnum():
            return end
        start = at + 1
    return None


def listing_marker(text: str, listing_words: list[str]) -> bool:
    """`trade::listing_in`: the seller register — first person present, the bare «فروشی»
    (unless refused, questioned or pluralised into the shop noun), «خریدارم», or a card
    number; the rhetorical «مگه» and the report vocabulary silence the word markers."""
    stripped = scrubbed(text)
    digits = text.translate(PERSIAN_DIGITS)
    reporting = any(w in stripped for w in ("گزارش", "کلاهبردار", "اسکم", "بلاکش", "پولشویی"))
    rhetorical = standalone(stripped, "مگه") is not None or reporting
    quotatives = ("میگه", "گفت", "میگفت", "نوشته", "زده", "فرستاده", "گفته")

    def reported(upto):
        return any(w in quotatives for w in stripped[:upto].split())

    if not rhetorical:
        for word in listing_words:
            end = standalone(stripped, word)
            if end is None:
                continue
            before = stripped[: end - len(word)].rstrip()
            if before.count("«") > before.count("»") or before.count('"') % 2 == 1:
                continue
            prev = before.rsplit(" ", 1)[-1]
            quoted = before.endswith(("«", '"', "'")) or reported(end - len(word))
            if quoted or prev == "نه" or "ناز" in prev:
                continue
            return True
        for verb in ("میفروشم", "میفروشیم"):
            start = 0
            while (at := stripped.find(verb, start)) >= 0:
                end = at + len(verb)
                free_right = end >= len(stripped) or not stripped[end].isalnum()
                glued = 0
                back = at - 1
                while back >= 0 and stripped[back].isalnum():
                    glued += 1
                    back -= 1
                negated = stripped[:at].endswith("ن")
                if free_right and glued > 1 and not negated:
                    return True
                start = at + 1
        end = standalone(stripped, "فروشی")
        if end is not None and reported(end - len("فروشی")):
            return has_card_number(digits)
        if end is not None:
            head = stripped[: end - len("فروشی")].rstrip()
            compound = any(head.endswith(h) for h in ("کم", "گران", "ارزان", "تن", "وطن"))
            tail = stripped[end:].lstrip()
            asking = tail.startswith(("؟", "?"))
            if not compound and not asking and not tail.startswith("نیست") and not tail.startswith("ها"):
                return True
        end = standalone(stripped, "فروشیه")
        if end is not None and reported(end - len("فروشیه")):
            return has_card_number(digits)
        if end is not None:
            tail = stripped[end:].lstrip()
            if not tail.startswith(("؟", "?")):
                return True
    return not rhetorical and has_card_number(digits)


def is_terse(text: str, marked: bool) -> bool:
    """`trade::is_terse`: one word never deletes, two words only with the register."""
    words = len(scrubbed(text).split())
    return words <= 1 or (not marked and words <= 2)


def deletes(margin: float, limit: int, marked: bool, terse: bool, floor: int) -> bool:
    """`trade::deletes`, with margins already in thousandths."""
    if terse:
        return False
    return margin >= limit or (marked and margin >= floor)


# ---------------------------------------------------------------------------------------
# the shipped files
# ---------------------------------------------------------------------------------------


def unescape(piece: str) -> str:
    out, chars = [], iter(piece)
    for c in chars:
        if c != "\\":
            out.append(c)
            continue
        n = next(chars, None)
        out.append({"n": "\n", "r": "\r", "s": " ", "\\": "\\"}.get(n, "\\" + (n or "")))
    return "".join(out)


def read_vocab(path: pathlib.Path) -> Unigram:
    pieces = []
    for line in path.read_text(encoding="utf-8").splitlines():
        piece, score = line.rsplit(" ", 1)
        pieces.append((unescape(piece), float(score)))
    return Unigram(pieces, 3)


def read_rows(path: pathlib.Path) -> list[tuple[str, str, str]]:
    """`(label, kind, text)` per line; a two-column file gets kind `-`."""
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split("\t")
        if len(parts) == 2:
            rows.append((parts[0], "-", parts[1]))
        elif len(parts) == 3:
            rows.append((parts[0], parts[1], parts[2]))
        else:
            sys.exit(f"malformed row: {line!r}")
    return rows


class Judge:
    """The scoring half of the runtime: both classifiers, the band between them, and the
    frame each one read. `margins` are in thousandths, the limit's own unit."""

    def __init__(self, files: pathlib.Path, constants: Constants):
        import onnxruntime as ort

        self.constants = constants
        self.unigram = read_vocab(files / "intent_vocab.txt")
        self.frames = (files / "intent_frames.txt").read_text(encoding="utf-8").split()
        self.small = ort.InferenceSession(str(files / "intent.onnx"), providers=["CPUExecutionProvider"])
        big = files / "intent_big.onnx"
        self.big = (
            ort.InferenceSession(str(big), providers=["CPUExecutionProvider"]) if big.exists() else None
        )

    def encode(self, texts: list[str]) -> tuple[np.ndarray, np.ndarray, list[float]]:
        ids = np.ones((len(texts), TOKENS), dtype=np.int64)
        mask = np.zeros((len(texts), TOKENS), dtype=np.int64)
        unk = []
        for at, text in enumerate(texts):
            # The runtime embeds the descrambled text (`intent::canonical`), so the mirror must.
            body = self.unigram.body(PREFIX + descramble(text.strip())[0][:512])[: TOKENS - 2]
            unk.append(sum(1 for i in body if i == 3) / max(len(body), 1))
            framed = [0] + body + [2]
            ids[at, : len(framed)] = framed
            mask[at, : len(framed)] = 1
        return ids, mask, unk

    def run(self, session, texts: list[str], batch: int = 32) -> tuple[np.ndarray, np.ndarray, list[float]]:
        """Margins in thousandths, the frame index each text was read as, `<unk>` shares."""
        margins, frames, unks = [], [], []
        for at in range(0, len(texts), batch):
            ids, mask, unk = self.encode(texts[at : at + batch])
            score, logits = session.run(None, {"input_ids": ids, "attention_mask": mask})
            margins.append(score[:, 0] * 1000.0)
            frames.append(logits.argmax(axis=1))
            unks.extend(unk)
        return np.concatenate(margins), np.concatenate(frames), unks

    def pipeline(self, texts: list[str]) -> tuple[np.ndarray, np.ndarray, list[float], int]:
        """The final margin and frame per text, after the escalation band."""
        margins, frames, unks = self.run(self.small, texts)
        escalated = 0
        if self.big is not None:
            band = [
                at
                for at, m in enumerate(margins)
                if self.constants.escalate_from <= m < self.constants.escalate_under
            ]
            if band:
                bigger, big_frames, _ = self.run(self.big, [texts[at] for at in band])
                for at, m, f in zip(band, bigger, big_frames):
                    margins[at] = m
                    frames[at] = f
            escalated = len(band)
        return margins, frames, unks, escalated


def band(name: str, values: list[float]) -> None:
    values = sorted(values)
    pick = lambda q: values[min(int(q * len(values)), len(values) - 1)]
    print(
        f"  {name:6} n={len(values):3}  min {values[0]:+.1f}  p5 {pick(0.05):+.1f}  "
        f"median {pick(0.5):+.1f}  p95 {pick(0.95):+.1f}  max {values[-1]:+.1f}  "
        f"mean {statistics.mean(values):+.1f}"
    )


# ---------------------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--files", default="target/release", help="where intent.onnx, intent_vocab.txt and intent_frames.txt sit")
    parser.add_argument("--data", default=str(TOOLS / "data" / "intent_eval.tsv"))
    parser.add_argument("--near", type=int, default=15, help="how many nearest negatives and lowest sells to print")
    args = parser.parse_args()

    constants = Constants()
    judge = Judge(pathlib.Path(args.files), constants)
    rows = read_rows(pathlib.Path(args.data))
    texts = [text for _, _, text in rows]
    labels = [label for label, _, _ in rows]
    kinds = [kind for _, kind, _ in rows]

    print(f"eval set: {len(rows)} rows ({labels.count('sell')} sell, {labels.count('hard')} hard, {labels.count('safe')} safe)")
    print(f"frames: {' '.join(judge.frames)}")

    margins, frames, unk_shares, escalated = judge.pipeline(texts)
    marked = np.array([listing_marker(text, constants.listing_words) for text in texts])
    terse = np.array([is_terse(text, mark) for text, mark in zip(texts, marked)])
    caught_net = np.array([suspicious(text, constants.marks) for text in texts])
    print(f"escalation: {escalated} of {len(texts)} rows re-judged by intent_big.onnx" if judge.big else "escalation: intent_big.onnx not present, small model only")

    print("\nmargins, in thousandths (the limit's own scale):")
    by_class = {label: [] for label in ("sell", "hard", "safe")}
    for label, margin in zip(labels, margins):
        by_class[label].append(float(margin))
    for label in ("sell", "hard", "safe"):
        if by_class[label]:
            band(label, by_class[label])

    print(f"\n<unk> share: max {max(unk_shares):.2f} (the Rust guard refuses above 0.30)")

    # The net's recall over the selling class. A miss here is unrecoverable: the model never
    # sees the message. Misses are printed one by one, because each is a row to fix in MARKS.
    total = labels.count("sell")
    caught = sum(1 for label, c in zip(labels, caught_net) if label == "sell" and c)
    print(f"\nlexical net: {caught}/{total} selling messages caught ({caught / total:.1%})")
    for (label, _, text), c in zip(rows, caught_net):
        if label == "sell" and not c:
            print(f"  MISSED: {text}")

    # The marked negatives are the bridge's entire risk surface, so they are printed in
    # full: every one must sit clearly under the floor.
    print(f"\nmarked non-selling rows (the bridge floor is {constants.floor}):")
    for (label, _, text), margin, mark in zip(rows, margins, marked):
        if mark and label != "sell":
            danger = "  <- OVER THE FLOOR" if margin >= constants.floor else ""
            print(f"  {label} {margin:+7.1f} {text[:90]}{danger}")

    # The sweep, with the net, the evidence floor and the bridge in force. False positives
    # are counted over hard+safe together, but the hard column is printed separately
    # because it is the one that moves.
    lo, hi = constants.limit_range
    print(f"\nsweep (default {constants.default_limit}, range {constants.limit_range}, floor {constants.floor}):")
    print("  limit  recall   FP(hard)  FP(safe)  FP(all)")
    zero_fp = None
    negatives = labels.count("hard") + labels.count("safe")

    def hits_at(limit: int) -> np.ndarray:
        return np.array(
            [
                c and deletes(m, limit, mark, shy, constants.floor)
                for m, mark, shy, c in zip(margins, marked, terse, caught_net)
            ]
        )

    for limit in range(max(lo - 10, 0), hi + 11, 5):
        hit = hits_at(limit)
        rows_hit = [label for label, h in zip(labels, hit) if h]
        hits = rows_hit.count("sell")
        fp_hard = rows_hit.count("hard")
        fp_safe = rows_hit.count("safe")
        marker = " <- default" if limit == constants.default_limit else ""
        if zero_fp is None and fp_hard + fp_safe == 0:
            zero_fp = limit
            marker += " <- first zero-FP row"
        print(
            f"  {limit:5}  {hits / total:6.1%}  {fp_hard:4}/{labels.count('hard'):3}  "
            f"{fp_safe:4}/{labels.count('safe'):3}  {(fp_hard + fp_safe) / max(negatives, 1):6.2%}{marker}"
        )

    # Where the lines are: the negatives nearest the limit and the sells furthest under it,
    # each with the frame the model read — the list the next corpus family comes from.
    print(f"\nnearest negatives (frame the model read, marker M, terse T):")
    order = np.argsort(-margins)
    shown = 0
    for at in order:
        if labels[at] == "sell" or not caught_net[at]:
            continue
        flags = ("M" if marked[at] else " ") + ("T" if terse[at] else " ")
        print(f"  {labels[at]:4} {margins[at]:+7.1f} {judge.frames[frames[at]]:12} {flags} {texts[at][:90]}")
        shown += 1
        if shown >= args.near:
            break
    print(f"\nlowest sells:")
    shown = 0
    for at in np.argsort(margins):
        if labels[at] != "sell":
            continue
        flags = ("M" if marked[at] else " ") + ("T" if terse[at] else " ") + (" " if caught_net[at] else "N")
        print(f"  {margins[at]:+7.1f} {judge.frames[frames[at]]:12} {flags} {texts[at][:90]}")
        shown += 1
        if shown >= args.near:
            break

    # The per-kind table, for a battery file.
    if any(kind != "-" for kind in kinds):
        for limit in (constants.default_limit, lo):
            hit = hits_at(limit)
            print(f"\nby kind at limit {limit}:")
            print(f"  {'kind':12} {'sell':>5} {'caught':>7} {'recall':>7} | {'neg':>4} {'FP':>3}")
            per: dict[str, list[int]] = {}
            for label, kind, h in zip(labels, kinds, hit):
                p = per.setdefault(kind, [0, 0, 0, 0])
                if label == "sell":
                    p[0] += 1
                    p[1] += int(h)
                else:
                    p[2] += 1
                    p[3] += int(h)
            for kind, (s, sh, n, nf) in per.items():
                recall = f"{sh / s:6.0%}" if s else "     -"
                print(f"  {kind:12} {s:5} {sh:7} {recall:>7} | {n:4} {nf:3}")
            fp = sum(p[3] for p in per.values())
            print(f"  TOTAL recall {sum(p[1] for p in per.values())}/{total}  FP {fp}/{negatives}")
            for label, kind, text, h, m, f in zip(labels, kinds, texts, hit, margins, frames):
                if h and label != "sell":
                    print(f"    FP {kind:10} {m:+6.1f} {judge.frames[f]:12} {text[:100]}")


if __name__ == "__main__":
    main()
