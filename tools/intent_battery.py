#!/usr/bin/env python3
"""The adversarial battery, as a gate.

`tools/data/intent_battery.tsv` is the second held-out judge of «قفل خرید و فروش»: hand-written
rows in every shape a group produces — obvious ads, terse listings, slang, typos, evasion,
finglish, mixed script, indirect offers, contact-and-price ads, wanted-posts against
inquiries, quoted ads inside warnings, reported speech, jokes, price talk, news, past
transactions, hypotheticals, refusals, talk about the lock itself, advice, tutorials,
third-person narration, charity with card numbers, phone-number runs, job posts, lost and
found, football transfers, complaints, and the deliberately confusing — each tagged with its
kind, and none of it ever trained on (`gen_intent_corpus.py`'s leak guard drops any row that
resembles one).

This runs every row through the full runtime mirror in `evaluate_intent.py`, prints recall
and false positives per kind at `DEFAULT_LIMIT` and at the bottom of `LIMIT_RANGE`, lists
every false positive verbatim with the frame the model read, and exits non-zero on any false
positive at either limit. `trade::tests::the_battery_holds_in_the_real_pipeline` runs the same
rows through the Rust pipeline, which is what keeps this mirror honest.

    ../bin/python tools/intent_battery.py --files ./out
"""

from __future__ import annotations

import argparse
import pathlib
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import evaluate_intent as ev  # noqa: E402

BATTERY = HERE / "data" / "intent_battery.tsv"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--files", default="target/release")
    parser.add_argument("--data", default=str(BATTERY))
    args = parser.parse_args()

    constants = ev.Constants()
    judge = ev.Judge(pathlib.Path(args.files), constants)
    rows = ev.read_rows(pathlib.Path(args.data))
    texts = [t for _, _, t in rows]
    labels = [l for l, _, _ in rows]
    kinds = [k for _, k, _ in rows]
    print(f"battery: {len(rows)} rows ({labels.count('sell')} sell, {len(rows) - labels.count('sell')} not)")

    margins, frames, _, escalated = judge.pipeline(texts)
    marked = [ev.listing_marker(t, constants.listing_words) for t in texts]
    terse = [ev.is_terse(t, m) for t, m in zip(texts, marked)]
    caught = [ev.suspicious(t, constants.marks) for t in texts]
    print(f"escalated {escalated} rows to the big model")

    failed = False
    for limit in (constants.default_limit, constants.limit_range[0]):
        hit = [
            c and ev.deletes(m, limit, mark, shy, constants.floor)
            for m, mark, shy, c in zip(margins, marked, terse, caught)
        ]
        per: dict[str, list[int]] = {}
        for label, kind, h in zip(labels, kinds, hit):
            p = per.setdefault(kind, [0, 0, 0, 0])
            if label == "sell":
                p[0] += 1
                p[1] += int(h)
            else:
                p[2] += 1
                p[3] += int(h)
        print(f"\nlimit {limit}{' (default)' if limit == constants.default_limit else ' (range floor)'}:")
        print(f"  {'kind':12} {'sell':>5} {'caught':>7} {'recall':>7} | {'neg':>4} {'FP':>3}")
        for kind, (s, sh, n, nf) in per.items():
            recall = f"{sh / s:6.0%}" if s else "     -"
            flag = "  <-" if nf else ""
            print(f"  {kind:12} {s:5} {sh:7} {recall:>7} | {n:4} {nf:3}{flag}")
        sells = sum(p[0] for p in per.values())
        fp = sum(p[3] for p in per.values())
        negatives = sum(p[2] for p in per.values())
        print(f"  TOTAL recall {sum(p[1] for p in per.values())}/{sells} = {sum(p[1] for p in per.values()) / sells:.1%}   FP {fp}/{negatives}")
        for label, kind, text, h, m, f in zip(labels, kinds, texts, hit, margins, frames):
            if h and label != "sell":
                print(f"    FP {kind:10} {m:+6.1f} {judge.frames[f]:12} {text[:110]}")
        failed = failed or fp > 0

    # The nearest misses either way, for the next corpus family.
    order = np.argsort(-margins)
    print("\nnearest negatives:")
    shown = 0
    for at in order:
        if labels[at] == "sell" or not caught[at] or terse[at]:
            continue
        print(f"  {kinds[at]:10} {margins[at]:+6.1f} {judge.frames[frames[at]]:12} {texts[at][:100]}")
        shown += 1
        if shown >= 12:
            break
    print("\nlowest sells:")
    shown = 0
    for at in np.argsort(margins):
        if labels[at] != "sell":
            continue
        note = "" if caught[at] else " (outside the net)"
        print(f"  {kinds[at]:10} {margins[at]:+6.1f} {judge.frames[frames[at]]:12} {texts[at][:100]}{note}")
        shown += 1
        if shown >= 12:
            break

    print("\nFAILED: false positives above" if failed else "\nclean: no false positives at either limit")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
