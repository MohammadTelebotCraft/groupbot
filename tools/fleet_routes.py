#!/usr/bin/env python3
"""Validate a complete set of per-shard ownership files before starting the fleet.

The runtime can reject a chat that is not in its own route file, but this offline check catches a
missing or duplicated id before any service is restarted. It checks group, settings, and resident
counter capacities. It never reads secrets or contacts Telegram/Postgres.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Mapping, Sequence

from fleet_manifest import validate_manifest
from shard_route import read_counter_weights, read_ids, read_note_weights, read_weights


class RouteSetError(ValueError):
    """The route files do not form one complete, capacity-safe ownership plan."""


def validate_route_sets(
    expected_ids: Sequence[int],
    routes: Mapping[str, Sequence[int]],
    declared_counts: Mapping[str, int],
    capacity: int,
    *,
    weights: Mapping[int, tuple[int, int]] | None = None,
    max_settings_rows: int | None = None,
    max_settings_bytes: int | None = None,
    counter_rows_per_group: int = 0,
    max_counter_rows: int | None = None,
    counter_weights: Mapping[int, int] | None = None,
    note_rows_per_group: int = 0,
    max_note_rows: int | None = None,
    note_weights: Mapping[int, int] | None = None,
) -> dict[str, object]:
    expected = set(expected_ids)
    if len(expected) != len(expected_ids):
        raise RouteSetError("input chat list contains duplicates")
    if not expected:
        raise RouteSetError("input chat list is empty")
    if capacity < 1:
        raise RouteSetError("capacity must be positive")
    if max_settings_rows is not None and max_settings_rows < 1:
        raise RouteSetError("max_settings_rows must be positive")
    if max_settings_bytes is not None and max_settings_bytes < 1:
        raise RouteSetError("max_settings_bytes must be positive")
    if counter_rows_per_group < 0:
        raise RouteSetError("counter_rows_per_group cannot be negative")
    if max_counter_rows is not None and max_counter_rows < 1:
        raise RouteSetError("max_counter_rows must be positive")
    if note_rows_per_group < 0:
        raise RouteSetError("note_rows_per_group cannot be negative")
    if max_note_rows is not None and max_note_rows < 1:
        raise RouteSetError("max_note_rows must be positive")
    if counter_weights is not None:
        counter_ids = set(counter_weights)
        if counter_ids != expected:
            raise RouteSetError(
                f"counter weights coverage mismatch: missing={len(expected - counter_ids)}, "
                f"extra={len(counter_ids - expected)}"
            )
        if any(value < 0 for value in counter_weights.values()):
            raise RouteSetError("counter row weights must be non-negative")
    if note_weights is not None:
        note_ids = set(note_weights)
        if note_ids != expected:
            raise RouteSetError(
                f"note weights coverage mismatch: missing={len(expected - note_ids)}, "
                f"extra={len(note_ids - expected)}"
            )
        if any(value < 0 for value in note_weights.values()):
            raise RouteSetError("note row weights must be non-negative")
    if weights is not None:
        weighted = set(weights)
        if weighted != expected:
            raise RouteSetError(
                f"weights coverage mismatch: missing={len(expected - weighted)}, "
                f"extra={len(weighted - expected)}"
            )
        if any(rows < 0 or bytes_ < 0 for rows, bytes_ in weights.values()):
            raise RouteSetError("weights must be non-negative")
    if set(routes) != set(declared_counts):
        raise RouteSetError("route and manifest shard names differ")

    owners: dict[int, str] = {}
    counts: dict[str, int] = {}
    setting_rows: dict[str, int] = {}
    setting_bytes: dict[str, int] = {}
    counter_rows: dict[str, int] = {}
    note_rows: dict[str, int] = {}
    for shard, ids in routes.items():
        ids = list(ids)
        if len(ids) > capacity:
            raise RouteSetError(f"{shard} has {len(ids)} chats, above capacity {capacity}")
        if len(ids) != declared_counts[shard]:
            raise RouteSetError(
                f"{shard} has {len(ids)} chats, manifest declares {declared_counts[shard]}"
            )
        for chat_id in ids:
            previous = owners.get(chat_id)
            if previous is not None:
                raise RouteSetError(f"chat {chat_id} appears in both {previous} and {shard}")
            owners[chat_id] = shard
        counts[shard] = len(ids)
        member_rows = sum(
            counter_weights[chat_id] if counter_weights is not None else counter_rows_per_group
            for chat_id in ids
        )
        if max_counter_rows is not None and member_rows > max_counter_rows:
            raise RouteSetError(
                f"{shard} has {member_rows} counter rows, above capacity {max_counter_rows}"
            )
        counter_rows[shard] = member_rows
        resident_notes = sum(
            note_weights[chat_id] if note_weights is not None else note_rows_per_group
            for chat_id in ids
        )
        if max_note_rows is not None and resident_notes > max_note_rows:
            raise RouteSetError(
                f"{shard} has {resident_notes} note rows, above capacity {max_note_rows}"
            )
        note_rows[shard] = resident_notes
        if weights is not None:
            rows = sum(weights[chat_id][0] for chat_id in ids)
            bytes_ = sum(weights[chat_id][1] for chat_id in ids)
            if max_settings_rows is not None and rows > max_settings_rows:
                raise RouteSetError(
                    f"{shard} has {rows} settings rows, above capacity {max_settings_rows}"
                )
            if max_settings_bytes is not None and bytes_ > max_settings_bytes:
                raise RouteSetError(
                    f"{shard} has {bytes_} setting bytes, above capacity {max_settings_bytes}"
                )
            setting_rows[shard] = rows
            setting_bytes[shard] = bytes_

    actual = set(owners)
    if actual != expected:
        missing = len(expected - actual)
        extra = len(actual - expected)
        raise RouteSetError(f"route coverage mismatch: missing={missing}, extra={extra}")
    return {
        "groups": len(actual),
        "shards": len(counts),
        "min_groups_per_shard": min(counts.values()),
        "max_groups_per_shard": max(counts.values()),
        "groups_per_shard": counts,
        "counter_rows_per_shard": counter_rows,
        "note_rows_per_shard": note_rows,
        "settings_rows_per_shard": setting_rows,
        "settings_bytes_per_shard": setting_bytes,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--input", type=Path, required=True, help="complete one-id-per-line chat list")
    parser.add_argument("--routes-dir", type=Path, required=True)
    parser.add_argument(
        "--weights",
        type=Path,
        help="chat_id settings_rows settings_bytes inventory for weighted validation",
    )
    parser.add_argument(
        "--counter-weights",
        type=Path,
        help="chat_id counter_rows inventory for resident counter validation",
    )
    parser.add_argument(
        "--note-weights",
        type=Path,
        help="chat_id note_rows inventory for resident note validation",
    )
    parser.add_argument(
        "--pattern",
        default="{shard}.chat_ids.txt",
        help="route filename pattern inside --routes-dir (must contain {shard})",
    )
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        document = json.loads(args.manifest.read_text(encoding="utf-8"))
        checked = validate_manifest(document)
        if "{shard}" not in args.pattern:
            raise RouteSetError("--pattern must contain {shard}")
        expected = read_ids(args.input)
        weights = read_weights(args.weights) if args.weights else None
        counter_weights = (
            read_counter_weights(args.counter_weights) if args.counter_weights else None
        )
        note_weights = read_note_weights(args.note_weights) if args.note_weights else None
        routes: dict[str, list[int]] = {}
        declared: dict[str, int] = {}
        for shard in document["shards"]:
            name = shard["name"]
            path = args.routes_dir / args.pattern.format(shard=name)
            routes[name] = read_ids(path)
            declared[name] = shard["groups"]
        report = validate_route_sets(
            expected,
            routes,
            declared,
            checked["required"]["groups_per_shard"],
            weights=weights,
            max_settings_rows=checked["required"]["max_settings_rows_per_shard"]
            if weights
            else None,
            max_settings_bytes=checked["required"]["max_settings_bytes_per_shard"]
            if weights
            else None,
            counter_rows_per_group=checked["required"]["counter_rows_per_group"],
            max_counter_rows=checked["required"]["max_counter_rows_per_shard"],
            counter_weights=counter_weights,
            note_rows_per_group=checked["required"]["note_rows_per_group"],
            max_note_rows=checked["required"]["max_note_rows_per_shard"],
            note_weights=note_weights,
        )
    except (OSError, json.JSONDecodeError, KeyError, ValueError) as error:
        print(f"fleet routes: invalid: {error}")
        return 2
    if args.json:
        print(json.dumps(report, sort_keys=True))
    else:
        print(
            f"valid: {report['groups']} groups across {report['shards']} shards; "
            f"per-shard range {report['min_groups_per_shard']}..{report['max_groups_per_shard']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
