#!/usr/bin/env python3
"""Validate a secret-free shard manifest before starting a bot fleet.

The manifest describes ownership and references to secrets; it never contains a bot token or a
database password.  This catches the two deployment mistakes that static capacity arithmetic
cannot see: two shards sharing a token/database, and a fleet whose declared shard count cannot
carry its group/action/counter budget.
"""

from __future__ import annotations

import argparse
import json
import math
import re
from dataclasses import asdict
from pathlib import Path
from typing import Any

from fleet_capacity import plan


NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,31}$")
ROUTING_STRATEGY = "rendezvous-blake2b-v1"


class ManifestError(ValueError):
    """The manifest is unsafe or cannot carry the declared fleet."""


def _positive_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise ManifestError(f"{label} must be a positive integer")
    return value


def _nonnegative_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ManifestError(f"{label} must be a non-negative integer")
    return value


def _nonnegative_number(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or value < 0:
        raise ManifestError(f"{label} must be a non-negative number")
    return float(value)


def _fleet_values(fleet: dict[str, Any]) -> dict[str, Any]:
    groups = _positive_int(fleet.get("groups"), "fleet.groups")
    groups_per_shard = _positive_int(
        fleet.get("groups_per_shard", 50_000), "fleet.groups_per_shard"
    )
    updates = _nonnegative_number(
        fleet.get("updates_per_second", 0), "fleet.updates_per_second"
    )
    updates_per_shard = _positive_int(
        fleet.get("updates_per_shard_per_second", 500),
        "fleet.updates_per_shard_per_second",
    )
    actions = _nonnegative_number(
        fleet.get("actions_per_second", 0), "fleet.actions_per_second"
    )
    telegram_actions = _positive_int(
        fleet.get("telegram_actions_per_second", 25),
        "fleet.telegram_actions_per_second",
    )
    counter_rows = _nonnegative_number(
        fleet.get("counter_rows_per_second", 0),
        "fleet.counter_rows_per_second",
    )
    counter_rows_per_shard = _positive_int(
        fleet.get("counter_rows_per_shard_per_second", 1_000),
        "fleet.counter_rows_per_shard_per_second",
    )
    counter_rows_per_group = _nonnegative_int(
        fleet.get("counter_rows_per_group", 100),
        "fleet.counter_rows_per_group",
    )
    max_counter_rows = _positive_int(
        fleet.get("max_counter_rows_per_shard", 5_000_000),
        "fleet.max_counter_rows_per_shard",
    )
    note_rows_per_group = _nonnegative_int(
        fleet.get("note_rows_per_group", 100), "fleet.note_rows_per_group"
    )
    max_note_rows = _positive_int(
        fleet.get("max_note_rows_per_shard", 5_000_000),
        "fleet.max_note_rows_per_shard",
    )
    settings_rows_per_group = _positive_int(
        fleet.get("settings_rows_per_group", 100),
        "fleet.settings_rows_per_group",
    )
    max_settings_rows = _positive_int(
        fleet.get("max_settings_rows_per_shard", 5_000_000),
        "fleet.max_settings_rows_per_shard",
    )
    settings_bytes_per_group = _positive_int(
        fleet.get("settings_bytes_per_group", 10_240),
        "fleet.settings_bytes_per_group",
    )
    max_settings_bytes = _positive_int(
        fleet.get("max_settings_bytes_per_shard", 512 * 1024 * 1024),
        "fleet.max_settings_bytes_per_shard",
    )
    db_pool = _positive_int(fleet.get("db_pool", 8), "fleet.db_pool")
    db_max = _positive_int(
        fleet.get("db_max_connections", 100), "fleet.db_max_connections"
    )
    reserve = fleet.get("db_reserve_ratio", 0.20)
    if isinstance(reserve, bool) or not isinstance(reserve, (int, float)) or not 0 <= reserve < 1:
        raise ManifestError("fleet.db_reserve_ratio must be in [0, 1)")
    memory_per_shard = _positive_int(
        fleet.get("memory_per_shard_mb", 2_048), "fleet.memory_per_shard_mb"
    )
    host_memory = _positive_int(
        fleet.get("host_memory_mb", 22_000), "fleet.host_memory_mb"
    )
    host_cpu_cores = _nonnegative_int(
        fleet.get("host_cpu_cores", 0), "fleet.host_cpu_cores"
    )
    cpu_cores_per_shard = _nonnegative_number(
        fleet.get("cpu_cores_per_shard", 0), "fleet.cpu_cores_per_shard"
    )
    hosts_available = _positive_int(
        fleet.get("hosts_available", 1), "fleet.hosts_available"
    )
    return {
        "groups": groups,
        "groups_per_shard": groups_per_shard,
        "updates_per_second": updates,
        "updates_per_shard_per_second": updates_per_shard,
        "actions_per_second": actions,
        "telegram_actions_per_second": telegram_actions,
        "counter_rows_per_second": counter_rows,
        "counter_rows_per_shard_per_second": counter_rows_per_shard,
        "counter_rows_per_group": counter_rows_per_group,
        "max_counter_rows_per_shard": max_counter_rows,
        "note_rows_per_group": note_rows_per_group,
        "max_note_rows_per_shard": max_note_rows,
        "settings_rows_per_group": settings_rows_per_group,
        "max_settings_rows_per_shard": max_settings_rows,
        "settings_bytes_per_group": settings_bytes_per_group,
        "max_settings_bytes_per_shard": max_settings_bytes,
        "db_pool": db_pool,
        "db_max_connections": db_max,
        "db_reserve_ratio": float(reserve),
        "memory_per_shard_mb": memory_per_shard,
        "host_memory_mb": host_memory,
        "host_cpu_cores": host_cpu_cores,
        "cpu_cores_per_shard": cpu_cores_per_shard,
        "hosts_available": hosts_available,
    }


def validate_manifest(document: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(document, dict):
        raise ManifestError("manifest root must be an object")
    fleet = document.get("fleet")
    shards = document.get("shards")
    if not isinstance(fleet, dict):
        raise ManifestError("manifest.fleet must be an object")
    if not isinstance(shards, list) or not shards:
        raise ManifestError("manifest.shards must be a non-empty array")
    routing = document.get("routing", {})
    if not isinstance(routing, dict):
        raise ManifestError("manifest.routing must be an object")
    strategy = routing.get("strategy", ROUTING_STRATEGY)
    seed = routing.get("seed", "groupbot-v1")
    if strategy != ROUTING_STRATEGY:
        raise ManifestError(f"routing.strategy must be {ROUTING_STRATEGY!r}")
    if not isinstance(seed, str) or not seed or len(seed) > 128:
        raise ManifestError("routing.seed must be a non-empty string of at most 128 characters")

    values = _fleet_values(fleet)
    required = plan(**values)
    names: set[str] = set()
    env_files: set[str] = set()
    token_refs: set[str] = set()
    database_refs: set[str] = set()
    host_shards: dict[str, int] = {}
    assigned_groups = 0

    for index, shard in enumerate(shards):
        prefix = f"shards[{index}]"
        if not isinstance(shard, dict):
            raise ManifestError(f"{prefix} must be an object")
        name = shard.get("name")
        if not isinstance(name, str) or not NAME.fullmatch(name):
            raise ManifestError(f"{prefix}.name must match {NAME.pattern!r}")
        if name in names:
            raise ManifestError(f"duplicate shard name: {name}")
        names.add(name)

        groups = _positive_int(shard.get("groups"), f"{prefix}.groups")
        if groups > values["groups_per_shard"]:
            raise ManifestError(
                f"{prefix}.groups exceeds fleet.groups_per_shard "
                f"({values['groups_per_shard']})"
            )
        estimated_settings_rows = groups * values["settings_rows_per_group"]
        if estimated_settings_rows > values["max_settings_rows_per_shard"]:
            raise ManifestError(
                f"{prefix} estimates {estimated_settings_rows} settings rows, "
                f"above max_settings_rows_per_shard "
                f"({values['max_settings_rows_per_shard']})"
            )
        estimated_counter_rows = groups * values["counter_rows_per_group"]
        if estimated_counter_rows > values["max_counter_rows_per_shard"]:
            raise ManifestError(
                f"{prefix} estimates {estimated_counter_rows} counter rows, "
                f"above max_counter_rows_per_shard "
                f"({values['max_counter_rows_per_shard']})"
            )
        estimated_note_rows = groups * values["note_rows_per_group"]
        if estimated_note_rows > values["max_note_rows_per_shard"]:
            raise ManifestError(
                f"{prefix} estimates {estimated_note_rows} note rows, "
                f"above max_note_rows_per_shard "
                f"({values['max_note_rows_per_shard']})"
            )
        estimated_settings_bytes = groups * values["settings_bytes_per_group"]
        if estimated_settings_bytes > values["max_settings_bytes_per_shard"]:
            raise ManifestError(
                f"{prefix} estimates {estimated_settings_bytes} setting bytes, "
                f"above max_settings_bytes_per_shard "
                f"({values['max_settings_bytes_per_shard']})"
            )
        assigned_groups += groups

        host = shard.get("host")
        if not isinstance(host, str) or not host.strip():
            raise ManifestError(f"{prefix}.host must be a non-empty host reference")
        host = host.strip()
        host_shards[host] = host_shards.get(host, 0) + 1

        for field, seen, label in (
            ("env_file", env_files, "environment file"),
            ("token_ref", token_refs, "token reference"),
            ("database_ref", database_refs, "database reference"),
        ):
            value = shard.get(field)
            if not isinstance(value, str) or not value.strip():
                raise ManifestError(f"{prefix}.{field} must be a non-empty reference")
            value = value.strip()
            if value in seen:
                raise ManifestError(f"duplicate {label}: {value}")
            seen.add(value)

    if assigned_groups != values["groups"]:
        raise ManifestError(
            f"shard group assignments total {assigned_groups}, expected {values['groups']}"
        )
    if len(shards) < required.shards_required:
        raise ManifestError(
            f"{len(shards)} shards declared, but {required.shards_required} are required"
        )

    actual_db = len(shards) * values["db_pool"]
    actual_db_with_reserve = math.ceil(actual_db / (1 - values["db_reserve_ratio"]))
    actual_memory = len(shards) * values["memory_per_shard_mb"]
    if actual_db_with_reserve > values["db_max_connections"]:
        raise ManifestError(
            f"declared shards need {actual_db_with_reserve} DB connections with reserve, "
            f"but max is {values['db_max_connections']}"
        )
    if len(host_shards) > values["hosts_available"]:
        raise ManifestError(
            f"manifest uses {len(host_shards)} hosts, "
            f"but only {values['hosts_available']} are available"
        )
    for host, count in host_shards.items():
        host_memory = count * values["memory_per_shard_mb"]
        if host_memory > values["host_memory_mb"]:
            raise ManifestError(
                f"host {host} needs {host_memory} MB, "
                f"but host budget is {values['host_memory_mb']} MB"
            )
        if (
            values["host_cpu_cores"]
            and count * values["cpu_cores_per_shard"] > values["host_cpu_cores"]
        ):
            raise ManifestError(
                f"host {host} needs "
                f"{count * values['cpu_cores_per_shard']:.2f} CPU cores, "
                f"but host has {values['host_cpu_cores']}"
            )

    return {
        "required": asdict(required),
        "declared_shards": len(shards),
        "assigned_groups": assigned_groups,
        "db_connections_used": actual_db,
        "db_connections_with_reserve": actual_db_with_reserve,
        "memory_used_mb": actual_memory,
        "hosts": {host: host_shards[host] for host in sorted(host_shards)},
        "routing": {"strategy": strategy, "seed": seed},
        "fits_database": True,
        "fits_memory": True,
        "fits_cpu": True,
        "fits_fleet": True,
        "shards": [shard["name"] for shard in shards],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        document = json.loads(args.manifest.read_text(encoding="utf-8"))
        result = validate_manifest(document)
    except (OSError, json.JSONDecodeError, ManifestError) as error:
        print(f"fleet manifest: invalid: {error}")
        return 2
    if args.json:
        print(json.dumps(result, sort_keys=True))
    else:
        print(f"fleet manifest: valid ({result['declared_shards']} shards)")
        print(
            f"groups={result['assigned_groups']} "
            f"db_connections={result['db_connections_with_reserve']} "
            f"memory_mb={result['memory_used_mb']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
