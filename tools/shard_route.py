#!/usr/bin/env python3
"""Assign chat ids to one deterministic fleet shard.

This is a planning tool only. It does not call Telegram and does not copy database rows. It uses
rendezvous preference, but when a real group list is supplied it also enforces the manifest's
per-shard group, settings, and resident-counter capacities. Pure hashing is naturally a little
uneven and can otherwise overflow a shard whose declared average is already at its hard limit.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
from typing import Iterable, Mapping

from fleet_manifest import ROUTING_STRATEGY, validate_manifest


def owner(chat_id: int, shards: Iterable[str], seed: str) -> str:
    return ranked(chat_id, shards, seed)[0]


def _score(chat_id: int, name: str, seed: str) -> bytes:
    return hashlib.blake2b(
        f"{seed}\0{chat_id}\0{name}".encode("utf-8"), digest_size=8
    ).digest()


def ranked(chat_id: int, shards: Iterable[str], seed: str) -> tuple[str, ...]:
    names = tuple(shards)
    if not names:
        raise ValueError("at least one shard is required")
    return tuple(
        sorted(
            names,
            key=lambda name: _score(chat_id, name, seed),
            reverse=True,
        )
    )


def assign(
    chat_ids: Iterable[int],
    shards: Iterable[str],
    seed: str,
    capacity: int,
    *,
    row_weights: Mapping[int, int] | None = None,
    byte_weights: Mapping[int, int] | None = None,
    max_rows: int | None = None,
    max_bytes: int | None = None,
    counter_rows_per_group: int = 0,
    max_counter_rows: int | None = None,
    counter_row_weights: Mapping[int, int] | None = None,
    note_rows_per_group: int = 0,
    max_note_rows: int | None = None,
    note_row_weights: Mapping[int, int] | None = None,
) -> dict[int, str]:
    names = tuple(shards)
    if not names:
        raise ValueError("at least one shard is required")
    if capacity < 1:
        raise ValueError("capacity must be positive")
    if counter_rows_per_group < 0:
        raise ValueError("counter_rows_per_group cannot be negative")
    if max_counter_rows is not None and max_counter_rows < 1:
        raise ValueError("max_counter_rows must be positive")
    if note_rows_per_group < 0:
        raise ValueError("note_rows_per_group cannot be negative")
    if max_note_rows is not None and max_note_rows < 1:
        raise ValueError("max_note_rows must be positive")
    if counter_row_weights is not None and any(
        value < 0 for value in counter_row_weights.values()
    ):
        raise ValueError("counter row weights must be non-negative")
    if note_row_weights is not None and any(
        value < 0 for value in note_row_weights.values()
    ):
        raise ValueError("note row weights must be non-negative")
    for label, weights, limit in (
        ("row", row_weights, max_rows),
        ("byte", byte_weights, max_bytes),
    ):
        if limit is not None and limit < 1:
            raise ValueError(f"max_{label}s must be positive")
        if weights is not None and any(value < 0 for value in weights.values()):
            raise ValueError(f"{label} weights must be non-negative")

    counts = Counter[str]()
    rows = Counter[str]()
    bytes_ = Counter[str]()
    counter_rows = Counter[str]()
    note_rows = Counter[str]()
    assignments: dict[int, str] = {}
    # Sorting makes a complete plan independent of the order in which the input file happened
    # to be produced. The rendezvous ranking is still the first choice for every chat; only a
    # full shard falls through to its next-ranked candidate. Scanning for the highest score among
    # non-full shards is exactly the first available item in that ranking, without sorting all
    # shard scores for every group. That matters when a 500k-group route is regenerated.
    for chat_id in sorted(set(chat_ids)):
        chat_rows = row_weights.get(chat_id, 0) if row_weights is not None else 0
        chat_bytes = byte_weights.get(chat_id, 0) if byte_weights is not None else 0
        chat_counter_rows = (
            counter_row_weights.get(chat_id, counter_rows_per_group)
            if counter_row_weights is not None
            else counter_rows_per_group
        )
        chat_note_rows = (
            note_row_weights.get(chat_id, note_rows_per_group)
            if note_row_weights is not None
            else note_rows_per_group
        )
        target = None
        best_score = b""
        for name in names:
            if counts[name] >= capacity:
                continue
            if max_rows is not None and rows[name] + chat_rows > max_rows:
                continue
            if max_bytes is not None and bytes_[name] + chat_bytes > max_bytes:
                continue
            if (
                max_counter_rows is not None
                and counter_rows[name] + chat_counter_rows > max_counter_rows
            ):
                continue
            if (
                max_note_rows is not None
                and note_rows[name] + chat_note_rows > max_note_rows
            ):
                continue
            score = _score(chat_id, name, seed)
            if target is None or score > best_score:
                target = name
                best_score = score
        if target is None:
            raise ValueError(
                f"{len(assignments) + 1} chats exceed declared count/weighted capacity"
            )
        assignments[chat_id] = target
        counts[target] += 1
        rows[target] += chat_rows
        bytes_[target] += chat_bytes
        counter_rows[target] += chat_counter_rows
        note_rows[target] += chat_note_rows
    return assignments


def read_weights(path: Path) -> dict[int, tuple[int, int]]:
    """Read `chat_id settings_rows settings_bytes` weights from a reviewed inventory."""

    weights: dict[int, tuple[int, int]] = {}
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        value = line.strip()
        if not value or value.startswith("#"):
            continue
        fields = value.split()
        if len(fields) != 3:
            raise ValueError(
                f"{path}:{line_number}: expected chat id, settings rows, and settings bytes"
            )
        try:
            chat_id, setting_rows, setting_bytes = map(int, fields)
        except ValueError as error:
            raise ValueError(f"{path}:{line_number}: expected integer weights") from error
        if setting_rows < 0 or setting_bytes < 0:
            raise ValueError(f"{path}:{line_number}: weights must be non-negative")
        if chat_id in weights:
            raise ValueError(f"{path}:{line_number}: duplicate chat id")
        weights[chat_id] = (setting_rows, setting_bytes)
    if not weights:
        raise ValueError(f"{path}: weights file is empty")
    return weights


def read_counter_weights(path: Path) -> dict[int, int]:
    """Read `chat_id counter_rows` from a reviewed member-row inventory."""

    weights: dict[int, int] = {}
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        value = line.strip()
        if not value or value.startswith("#"):
            continue
        fields = value.split()
        if len(fields) != 2:
            raise ValueError(f"{path}:{line_number}: expected chat id and counter rows")
        try:
            chat_id, counter_rows = map(int, fields)
        except ValueError as error:
            raise ValueError(f"{path}:{line_number}: expected integer weights") from error
        if counter_rows < 0:
            raise ValueError(f"{path}:{line_number}: counter rows must be non-negative")
        if chat_id in weights:
            raise ValueError(f"{path}:{line_number}: duplicate chat id")
        weights[chat_id] = counter_rows
    if not weights:
        raise ValueError(f"{path}: counter weights file is empty")
    return weights


def read_note_weights(path: Path) -> dict[int, int]:
    """Read `chat_id note_rows` from a reviewed note inventory."""

    weights: dict[int, int] = {}
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        value = line.strip()
        if not value or value.startswith("#"):
            continue
        fields = value.split()
        if len(fields) != 2:
            raise ValueError(f"{path}:{line_number}: expected chat id and note rows")
        try:
            chat_id, note_rows = map(int, fields)
        except ValueError as error:
            raise ValueError(f"{path}:{line_number}: expected integer weights") from error
        if note_rows < 0:
            raise ValueError(f"{path}:{line_number}: note rows must be non-negative")
        if chat_id in weights:
            raise ValueError(f"{path}:{line_number}: duplicate chat id")
        weights[chat_id] = note_rows
    if not weights:
        raise ValueError(f"{path}: note weights file is empty")
    return weights


def read_ids(path: Path) -> list[int]:
    ids: list[int] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        value = line.strip()
        if not value or value.startswith("#"):
            continue
        try:
            ids.append(int(value))
        except ValueError as error:
            raise ValueError(f"{path}:{line_number}: expected one integer chat id") from error
    if len(set(ids)) != len(ids):
        raise ValueError(f"{path}: duplicate chat id")
    return ids


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--chat-id", action="append", type=int, dest="chat_ids")
    parser.add_argument("--input", type=Path, help="newline-delimited chat ids")
    parser.add_argument(
        "--weights",
        type=Path,
        help="chat_id settings_rows settings_bytes inventory for weighted routing",
    )
    parser.add_argument(
        "--counter-weights",
        type=Path,
        help="chat_id counter_rows inventory for resident counter routing",
    )
    parser.add_argument(
        "--note-weights",
        type=Path,
        help="chat_id note_rows inventory for resident note routing",
    )
    parser.add_argument("--shard", help="emit only the chats assigned to this declared shard")
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        document = json.loads(args.manifest.read_text(encoding="utf-8"))
        checked = validate_manifest(document)
        ids = list(args.chat_ids or [])
        if args.input:
            ids.extend(read_ids(args.input))
        if not ids:
            raise ValueError("provide --chat-id or --input")
        if len(set(ids)) != len(ids):
            raise ValueError("duplicate chat id")
        weighted = read_weights(args.weights) if args.weights else None
        counter_weighted = (
            read_counter_weights(args.counter_weights) if args.counter_weights else None
        )
        note_weighted = read_note_weights(args.note_weights) if args.note_weights else None
        if weighted is not None:
            input_ids = set(ids)
            weighted_ids = set(weighted)
            if input_ids != weighted_ids:
                raise ValueError(
                    f"weights coverage mismatch: missing={len(input_ids - weighted_ids)}, "
                    f"extra={len(weighted_ids - input_ids)}"
                )
        if counter_weighted is not None:
            input_ids = set(ids)
            counter_ids = set(counter_weighted)
            if input_ids != counter_ids:
                raise ValueError(
                    f"counter weights coverage mismatch: missing={len(input_ids - counter_ids)}, "
                    f"extra={len(counter_ids - input_ids)}"
                )
        if note_weighted is not None:
            input_ids = set(ids)
            note_ids = set(note_weighted)
            if input_ids != note_ids:
                raise ValueError(
                    f"note weights coverage mismatch: missing={len(input_ids - note_ids)}, "
                    f"extra={len(note_ids - input_ids)}"
                )
        shards = checked["shards"]
        seed = checked["routing"]["seed"]
        if checked["routing"]["strategy"] != ROUTING_STRATEGY:
            raise ValueError("unsupported routing strategy")
        if args.shard is not None and args.shard not in shards:
            raise ValueError(f"unknown shard: {args.shard}")
        planned = assign(
            ids,
            shards,
            seed,
            checked["required"]["groups_per_shard"],
            row_weights={chat: values[0] for chat, values in weighted.items()}
            if weighted
            else None,
            byte_weights={chat: values[1] for chat, values in weighted.items()}
            if weighted
            else None,
            max_rows=checked["required"]["max_settings_rows_per_shard"]
            if weighted
            else None,
            max_bytes=checked["required"]["max_settings_bytes_per_shard"]
            if weighted
            else None,
            counter_rows_per_group=checked["required"]["counter_rows_per_group"],
            max_counter_rows=checked["required"]["max_counter_rows_per_shard"],
            counter_row_weights=counter_weighted,
            note_rows_per_group=checked["required"]["note_rows_per_group"],
            max_note_rows=checked["required"]["max_note_rows_per_shard"],
            note_row_weights=note_weighted,
        )
        assignments = [
            {"chat_id": chat_id, "shard": planned[chat_id]}
            for chat_id in ids
            if args.shard is None or planned[chat_id] == args.shard
        ]
    except (OSError, json.JSONDecodeError, ValueError, KeyError) as error:
        print(f"shard route: invalid: {error}")
        return 2
    if args.json:
        print(json.dumps(assignments, sort_keys=True))
    elif args.shard is not None:
        for assignment in assignments:
            print(assignment["chat_id"])
    else:
        for assignment in assignments:
            print(f"{assignment['chat_id']}\t{assignment['shard']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
