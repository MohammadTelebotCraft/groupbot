#!/usr/bin/env python3
"""Evaluate the actual Rust transaction pipeline, including context, with shipped models."""
from __future__ import annotations

import argparse
import collections
import json
import os
import pathlib
import subprocess
import tempfile

from intent_normalize import descramble

ROOT = pathlib.Path(__file__).resolve().parents[1]


def evaluate(files: pathlib.Path, data: pathlib.Path) -> list[dict]:
    with tempfile.TemporaryDirectory(prefix="trade-evaluation-") as temporary:
        result = pathlib.Path(temporary) / "result.json"
        env = dict(os.environ, VISION_FILES=str(files.resolve()), TRADE_EVAL_INPUT=str(data.resolve()),
                   TRADE_EVAL_OUTPUT=str(result))
        subprocess.run([os.environ.get("CARGO", "cargo"), "test", "--locked", "handlers::trade::tests::export_runtime_evaluation",
                        "--", "--exact", "--ignored"], cwd=ROOT, env=env, check=True)
        rows = json.loads(result.read_text(encoding="utf-8"))
    if not rows:
        raise ValueError("the evaluation must contain labeled rows")
    for row in rows:
        if row["label"] not in ("sell", "safe", "hard"):
            raise ValueError(f"unexpected label: {row['label']}")
        assert descramble(row["text"])[0] == row["normalized"], f"training/runtime normalization drift: {row['text']}"
    return rows


def main(default_data="intent_eval.tsv") -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--files", type=pathlib.Path, default=ROOT / "target/release")
    parser.add_argument("--data", type=pathlib.Path, default=ROOT / "tools/data" / default_data)
    parser.add_argument("--json", type=pathlib.Path, help="save the per-message evidence and decisions")
    parser.add_argument("--near", type=int, default=15, help="maximum uncertain misses to print")
    args = parser.parse_args()
    rows = evaluate(args.files, args.data)
    failed = False
    for decision in ("deleted", "deleted_at_minimum"):
        print(f"\n{args.data.name}: {decision}")
        counts = collections.defaultdict(lambda: [0, 0, 0, 0])
        for row in rows:
            selling = row["label"] == "sell"
            count = counts[row["kind"]]
            count[0 if selling else 2] += 1
            count[1 if selling else 3] += int(row[decision])
        for kind, (selling, caught, safe, fp) in sorted(counts.items()):
            print(f"  {kind:20} caught {caught:3}/{selling:<3}  false positives {fp}/{safe}")
        totals = [sum(count[i] for count in counts.values()) for i in range(4)]
        print(f"TOTAL: caught {totals[1]}/{totals[0]}; false positives {totals[3]}/{totals[2]}")
        for row in rows:
            if row["label"] != "sell" and row[decision]:
                print("FALSE POSITIVE:", row["text"], row["frame"], row["margin"])
        failed |= totals[3] != 0
    print("\nUncertain misses at the default:")
    for row in [row for row in rows if row["label"] == "sell" and not row["deleted"]][:args.near]:
        print(f"  {row['kind']}: {row['text']} ({row['frame']}, {row['margin']})")
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(rows, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
