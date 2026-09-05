#!/usr/bin/env python3
"""Estimate the minimum shard count and shared-resource budget for a bot fleet.

This is intentionally a small, dependency-free model. It does not guess traffic: operators
must provide measured incoming update and outgoing action rates for their workload. Group count,
Telegram action capacity, update dispatch capacity, resident counter/settings rows, database
 connections, host memory, and resident note rows are separate ceilings.
"""

from __future__ import annotations

import argparse
import json
import math
from dataclasses import asdict, dataclass


@dataclass(frozen=True)
class FleetPlan:
    groups: int
    groups_per_shard: int
    updates_per_second: float
    updates_per_shard_per_second: float
    actions_per_second: float
    telegram_actions_per_second: float
    counter_rows_per_second: float
    counter_rows_per_shard_per_second: float
    counter_rows_per_group: int
    max_counter_rows_per_shard: int
    counter_rows_total: int
    settings_rows_per_group: int
    max_settings_rows_per_shard: int
    settings_rows_total: int
    settings_bytes_per_group: int
    max_settings_bytes_per_shard: int
    settings_bytes_total: int
    db_pool: int
    db_max_connections: int
    db_reserve_ratio: float
    memory_per_shard_mb: int
    host_memory_mb: int
    host_cpu_cores: int
    cpu_cores_per_shard: float
    hosts_available: int
    shards_for_groups: int
    shards_for_updates: int
    shards_for_actions: int
    shards_for_counters: int
    shards_for_counter_storage: int
    note_rows_per_group: int
    max_note_rows_per_shard: int
    note_rows_total: int
    shards_for_note_storage: int
    shards_for_settings: int
    shards_for_settings_bytes: int
    shards_required: int
    db_connections_used: int
    db_connections_with_reserve: int
    memory_used_mb: int
    hosts_for_memory: int
    hosts_for_cpu: int
    hosts_required: int
    fits_database: bool
    fits_memory: bool
    fits_cpu: bool
    fits_fleet: bool


def ceil_ratio(value: float, divisor: float) -> int:
    if value <= 0:
        return 0
    return math.ceil(value / divisor)


def plan(
    *,
    groups: int,
    groups_per_shard: int = 50_000,
    updates_per_second: float = 0.0,
    updates_per_shard_per_second: float = 500.0,
    actions_per_second: float = 0.0,
    telegram_actions_per_second: float = 30.0,
    counter_rows_per_second: float = 0.0,
    counter_rows_per_shard_per_second: float = 1_000.0,
    counter_rows_per_group: int = 100,
    max_counter_rows_per_shard: int = 5_000_000,
    note_rows_per_group: int = 100,
    max_note_rows_per_shard: int = 5_000_000,
    settings_rows_per_group: int = 100,
    max_settings_rows_per_shard: int = 5_000_000,
    settings_bytes_per_group: int = 10_240,
    max_settings_bytes_per_shard: int = 512 * 1024 * 1024,
    db_pool: int = 8,
    db_max_connections: int = 100,
    db_reserve_ratio: float = 0.20,
    memory_per_shard_mb: int = 2_048,
    host_memory_mb: int = 22_000,
    host_cpu_cores: int = 0,
    cpu_cores_per_shard: float = 0.0,
    hosts_available: int = 1,
) -> FleetPlan:
    if groups < 1:
        raise ValueError("groups must be positive")
    if groups_per_shard < 1:
        raise ValueError("groups_per_shard must be positive")
    if updates_per_second < 0:
        raise ValueError("updates_per_second cannot be negative")
    if updates_per_shard_per_second <= 0:
        raise ValueError("updates_per_shard_per_second must be positive")
    if actions_per_second < 0:
        raise ValueError("actions_per_second cannot be negative")
    if telegram_actions_per_second <= 0:
        raise ValueError("telegram_actions_per_second must be positive")
    if counter_rows_per_second < 0:
        raise ValueError("counter_rows_per_second cannot be negative")
    if counter_rows_per_shard_per_second <= 0:
        raise ValueError("counter_rows_per_shard_per_second must be positive")
    if counter_rows_per_group < 0:
        raise ValueError("counter_rows_per_group cannot be negative")
    if max_counter_rows_per_shard < 1:
        raise ValueError("max_counter_rows_per_shard must be positive")
    if note_rows_per_group < 0:
        raise ValueError("note_rows_per_group cannot be negative")
    if max_note_rows_per_shard < 1:
        raise ValueError("max_note_rows_per_shard must be positive")
    if settings_rows_per_group < 1:
        raise ValueError("settings_rows_per_group must be positive")
    if max_settings_rows_per_shard < 1:
        raise ValueError("max_settings_rows_per_shard must be positive")
    if settings_bytes_per_group < 1:
        raise ValueError("settings_bytes_per_group must be positive")
    if max_settings_bytes_per_shard < 1:
        raise ValueError("max_settings_bytes_per_shard must be positive")
    if not 0 <= db_reserve_ratio < 1:
        raise ValueError("db_reserve_ratio must be in [0, 1)")
    if db_pool < 1 or db_max_connections < 1:
        raise ValueError("database connection limits must be positive")
    if memory_per_shard_mb < 1 or host_memory_mb < 1:
        raise ValueError("memory limits must be positive")
    if host_cpu_cores < 0:
        raise ValueError("host_cpu_cores cannot be negative")
    if cpu_cores_per_shard < 0:
        raise ValueError("cpu_cores_per_shard cannot be negative")
    if hosts_available < 1:
        raise ValueError("hosts_available must be positive")

    shards_for_groups = ceil_ratio(groups, groups_per_shard)
    shards_for_updates = ceil_ratio(updates_per_second, updates_per_shard_per_second)
    shards_for_actions = ceil_ratio(actions_per_second, telegram_actions_per_second)
    shards_for_counters = ceil_ratio(
        counter_rows_per_second, counter_rows_per_shard_per_second
    )
    counter_rows_total = groups * counter_rows_per_group
    shards_for_counter_storage = ceil_ratio(
        counter_rows_total, max_counter_rows_per_shard
    )
    note_rows_total = groups * note_rows_per_group
    shards_for_note_storage = ceil_ratio(note_rows_total, max_note_rows_per_shard)
    settings_rows_total = groups * settings_rows_per_group
    settings_bytes_total = groups * settings_bytes_per_group
    shards_for_settings = ceil_ratio(settings_rows_total, max_settings_rows_per_shard)
    shards_for_settings_bytes = ceil_ratio(
        settings_bytes_total, max_settings_bytes_per_shard
    )
    shards_required = max(
        shards_for_groups,
        shards_for_updates,
        shards_for_actions,
        shards_for_counters,
        shards_for_counter_storage,
        shards_for_note_storage,
        shards_for_settings,
        shards_for_settings_bytes,
        1,
    )
    db_connections_used = shards_required * db_pool
    db_connections_with_reserve = math.ceil(db_connections_used / (1 - db_reserve_ratio))
    memory_used_mb = shards_required * memory_per_shard_mb
    hosts_for_memory = ceil_ratio(memory_used_mb, host_memory_mb)
    hosts_for_cpu = (
        ceil_ratio(shards_required * cpu_cores_per_shard, host_cpu_cores)
        if host_cpu_cores and cpu_cores_per_shard
        else 0
    )
    hosts_required = max(hosts_for_memory, hosts_for_cpu, 1)
    fits_database = db_connections_with_reserve <= db_max_connections
    fits_memory = hosts_for_memory <= hosts_available
    fits_cpu = hosts_for_cpu == 0 or hosts_for_cpu <= hosts_available

    return FleetPlan(
        groups=groups,
        groups_per_shard=groups_per_shard,
        updates_per_second=updates_per_second,
        updates_per_shard_per_second=updates_per_shard_per_second,
        actions_per_second=actions_per_second,
        telegram_actions_per_second=telegram_actions_per_second,
        counter_rows_per_second=counter_rows_per_second,
        counter_rows_per_shard_per_second=counter_rows_per_shard_per_second,
        counter_rows_per_group=counter_rows_per_group,
        max_counter_rows_per_shard=max_counter_rows_per_shard,
        counter_rows_total=counter_rows_total,
        note_rows_per_group=note_rows_per_group,
        max_note_rows_per_shard=max_note_rows_per_shard,
        note_rows_total=note_rows_total,
        shards_for_note_storage=shards_for_note_storage,
        settings_rows_per_group=settings_rows_per_group,
        max_settings_rows_per_shard=max_settings_rows_per_shard,
        settings_rows_total=settings_rows_total,
        settings_bytes_per_group=settings_bytes_per_group,
        max_settings_bytes_per_shard=max_settings_bytes_per_shard,
        settings_bytes_total=settings_bytes_total,
        db_pool=db_pool,
        db_max_connections=db_max_connections,
        db_reserve_ratio=db_reserve_ratio,
        memory_per_shard_mb=memory_per_shard_mb,
        host_memory_mb=host_memory_mb,
        host_cpu_cores=host_cpu_cores,
        cpu_cores_per_shard=cpu_cores_per_shard,
        hosts_available=hosts_available,
        shards_for_groups=shards_for_groups,
        shards_for_updates=shards_for_updates,
        shards_for_actions=shards_for_actions,
        shards_for_counters=shards_for_counters,
        shards_for_counter_storage=shards_for_counter_storage,
        shards_for_settings=shards_for_settings,
        shards_for_settings_bytes=shards_for_settings_bytes,
        shards_required=shards_required,
        db_connections_used=db_connections_used,
        db_connections_with_reserve=db_connections_with_reserve,
        memory_used_mb=memory_used_mb,
        hosts_for_memory=hosts_for_memory,
        hosts_for_cpu=hosts_for_cpu,
        hosts_required=hosts_required,
        fits_database=fits_database,
        fits_memory=fits_memory,
        fits_cpu=fits_cpu,
        fits_fleet=fits_database and fits_memory and fits_cpu,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--groups", type=int, required=True)
    parser.add_argument("--groups-per-shard", type=int, default=50_000)
    parser.add_argument("--updates-per-second", type=float, default=0.0)
    parser.add_argument("--updates-per-shard-per-second", type=float, default=500.0)
    parser.add_argument("--actions-per-second", type=float, default=0.0)
    parser.add_argument("--telegram-actions-per-second", type=float, default=30.0)
    parser.add_argument("--counter-rows-per-second", type=float, default=0.0)
    parser.add_argument("--counter-rows-per-shard-per-second", type=float, default=1_000.0)
    parser.add_argument("--counter-rows-per-group", type=int, default=100)
    parser.add_argument("--max-counter-rows-per-shard", type=int, default=5_000_000)
    parser.add_argument("--note-rows-per-group", type=int, default=100)
    parser.add_argument("--max-note-rows-per-shard", type=int, default=5_000_000)
    parser.add_argument("--settings-rows-per-group", type=int, default=100)
    parser.add_argument("--max-settings-rows-per-shard", type=int, default=5_000_000)
    parser.add_argument("--settings-bytes-per-group", type=int, default=10_240)
    parser.add_argument(
        "--max-settings-bytes-per-shard", type=int, default=512 * 1024 * 1024
    )
    parser.add_argument("--db-pool", type=int, default=8)
    parser.add_argument("--db-max-connections", type=int, default=100)
    parser.add_argument("--db-reserve-ratio", type=float, default=0.20)
    parser.add_argument("--memory-per-shard-mb", type=int, default=2_048)
    parser.add_argument("--host-memory-mb", type=int, default=22_000)
    parser.add_argument(
        "--host-cpu-cores",
        type=int,
        default=0,
        help="physical cores per host; 0 disables CPU placement checks",
    )
    parser.add_argument(
        "--cpu-cores-per-shard",
        type=float,
        default=0.0,
        help="reserved host cores per shard; 0 disables CPU placement checks",
    )
    parser.add_argument("--hosts-available", type=int, default=1)
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    values = vars(args).copy()
    values.pop("json", None)
    result = plan(**values)
    if args.json:
        print(json.dumps(asdict(result), sort_keys=True))
    else:
        for key, value in asdict(result).items():
            print(f"{key}={value}")
    if not result.fits_fleet:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
