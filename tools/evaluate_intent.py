#!/usr/bin/env python3
"""Evaluate shipped intent models through the Rust production pipeline.

The small Python screening helpers are also used during fine-tuning. They are not the
release judge: the CLI delegates to intent_runtime_eval.py, which runs Rust's actual
normalization, grammatical interpretation, context handling and deletion decision.

    ../bin/python tools/evaluate_intent.py --files target/release
    ../bin/python tools/evaluate_intent.py --data tools/data/intent_context.tsv
"""

from __future__ import annotations

import pathlib
import re
import sys

import numpy as np

TOOLS = pathlib.Path(__file__).resolve().parent
ROOT = TOOLS.parent
sys.path.insert(0, str(TOOLS))


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
    """Constants used by training-time screening, read from the Rust source."""

    def __init__(self):
        trade = rust_source("trade.rs")
        self.marks = rust_marks(trade)
        self.listing_words = rust_listing_words(trade)
        self.floor = int(rust_const(trade, "DEFAULT_LIMIT"))
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


from intent_normalize import descramble  # noqa: E402


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
    """Threshold-only helper for training diagnostics; release decisions run in Rust."""
    if terse:
        return False
    return bool(np.isfinite(margin) and margin >= limit)


# ---------------------------------------------------------------------------------------
# the shipped files
# ---------------------------------------------------------------------------------------


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


def main() -> None:
    # Training helpers above remain lightweight; release evaluation must execute the Rust
    # pipeline so context, evidence gates and failure handling cannot drift from production.
    from intent_runtime_eval import main as runtime_main
    raise SystemExit(runtime_main())


if __name__ == "__main__":
    main()
