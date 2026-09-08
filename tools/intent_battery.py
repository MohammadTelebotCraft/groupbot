#!/usr/bin/env python3
"""Run the held-out battery through the actual Rust production pipeline.

Exits non-zero on any false positive at the default or minimum admin threshold,
or on missing model files or disagreement with training-time normalization.
"""
from intent_runtime_eval import main

if __name__ == "__main__":
    raise SystemExit(main("intent_battery.tsv"))
