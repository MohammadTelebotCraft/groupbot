#!/usr/bin/env python3
"""Move selected chat-scoped rows between shard PostgreSQL databases.

The command is deliberately dry-run by default.  It reads DSNs from environment variables rather
than accepting secrets on the command line, checks the target's hard capacity limits, copies one
bounded chat batch at a time, verifies the target rows, and only then deletes the source rows.
PostgreSQL cannot make a transaction spanning two independent databases, so the source and target
services must be stopped during an apply.  If the process dies after a target commit, rerunning the
same command is safe when the existing target rows are an exact match; a partial or conflicting
target row is rejected instead of being overwritten.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
from typing import Any, Iterable, Sequence


class MigrationError(ValueError):
    """The migration input or its capacity plan is unsafe."""


@dataclass(frozen=True)
class TableSpec:
    name: str
    columns: tuple[str, ...]
    order_by: tuple[str, ...]


# These are the only tables owned by a chat.  started_users and calibration are intentionally
# absent: the former is account-wide user state and the latter is a fleet traffic reservoir.
CHAT_TABLES = (
    TableSpec("settings", ("chat_id", "key", "value"), ("chat_id", "key")),
    TableSpec(
        "counters",
        (
            "chat_id",
            "user_id",
            "name",
            "total",
            "today",
            "day",
            "week",
            "week_at",
            "month",
            "month_at",
            "seen",
            "adds",
            "awarded",
            "warns",
            "strikes",
            "struck",
        ),
        ("chat_id", "user_id"),
    ),
    TableSpec("notes", ("chat_id", "user_id", "value"), ("chat_id", "user_id")),
    TableSpec("tallies", ("chat_id", "counter", "day", "count"), ("chat_id", "counter")),
    TableSpec(
        "pending_deletes",
        ("chat_id", "message_id", "due_at"),
        ("chat_id", "message_id"),
    ),
    TableSpec(
        "image_filters",
        ("chat_id", "name", "vec", "scale", "cut", "rate", "live", "samples", "calibrated"),
        ("chat_id", "name"),
    ),
)

MIN_CHAT_ID = -(1 << 63)
MAX_CHAT_ID = (1 << 63) - 1
DEFAULT_MAX_CHATS = 50_000
DEFAULT_MAX_SETTINGS_ROWS = 5_000_000
DEFAULT_MAX_SETTINGS_BYTES = 512 * 1024 * 1024
DEFAULT_MAX_COUNTER_ROWS = 5_000_000
DEFAULT_MAX_NOTE_ROWS = 5_000_000
DEFAULT_BATCH_CHATS = 100
DEFAULT_MAX_ROWS_PER_BATCH = 250_000


def parse_chat_ids(lines: Iterable[str], source: str = "input", shard: str | None = None) -> list[int]:
    """Parse one chat id per line, or the first column of shard_route TSV output."""

    ids: list[int] = []
    for line_number, line in enumerate(lines, 1):
        value = line.strip()
        if not value or value.startswith("#"):
            continue
        fields = value.split()
        if len(fields) > 2:
            raise MigrationError(f"{source}:{line_number}: expected chat id or chat_id<TAB>shard")
        if len(fields) == 2 and shard is not None and fields[1] != shard:
            continue
        try:
            chat_id = int(fields[0], 10)
        except ValueError as error:
            raise MigrationError(f"{source}:{line_number}: expected an integer chat id") from error
        if not MIN_CHAT_ID <= chat_id <= MAX_CHAT_ID:
            raise MigrationError(f"{source}:{line_number}: chat id is outside BIGINT range")
        if chat_id == 0:
            raise MigrationError(f"{source}:{line_number}: chat id 0 is global state, not a shard chat")
        ids.append(chat_id)
    if len(set(ids)) != len(ids):
        raise MigrationError(f"{source}: duplicate chat id")
    if not ids:
        raise MigrationError(f"{source}: no chat ids")
    return ids


def validate_limits(
    *,
    target_chat_count: int,
    target_settings_rows: int,
    moving_chat_count: int,
    moving_settings_rows: int,
    target_settings_bytes: int,
    moving_settings_bytes: int,
    target_existing_moving_chats: int,
    target_existing_moving_settings_rows: int,
    target_existing_moving_settings_bytes: int,
    target_counter_rows: int = 0,
    moving_counter_rows: int = 0,
    target_existing_moving_counter_rows: int = 0,
    target_note_rows: int = 0,
    moving_note_rows: int = 0,
    target_existing_moving_note_rows: int = 0,
    max_chats: int = DEFAULT_MAX_CHATS,
    max_settings_rows: int = DEFAULT_MAX_SETTINGS_ROWS,
    max_settings_bytes: int = DEFAULT_MAX_SETTINGS_BYTES,
    max_counter_rows: int = DEFAULT_MAX_COUNTER_ROWS,
    max_note_rows: int = DEFAULT_MAX_NOTE_ROWS,
) -> dict[str, int]:
    """Return the post-migration capacity numbers or reject the plan."""

    for value, label in (
        (target_chat_count, "target_chat_count"),
        (target_settings_rows, "target_settings_rows"),
        (moving_chat_count, "moving_chat_count"),
        (moving_settings_rows, "moving_settings_rows"),
        (target_settings_bytes, "target_settings_bytes"),
        (moving_settings_bytes, "moving_settings_bytes"),
        (target_existing_moving_chats, "target_existing_moving_chats"),
        (target_existing_moving_settings_rows, "target_existing_moving_settings_rows"),
        (target_existing_moving_settings_bytes, "target_existing_moving_settings_bytes"),
        (target_counter_rows, "target_counter_rows"),
        (moving_counter_rows, "moving_counter_rows"),
        (target_existing_moving_counter_rows, "target_existing_moving_counter_rows"),
        (target_note_rows, "target_note_rows"),
        (moving_note_rows, "moving_note_rows"),
        (target_existing_moving_note_rows, "target_existing_moving_note_rows"),
        (max_chats, "max_chats"),
        (max_settings_rows, "max_settings_rows"),
        (max_settings_bytes, "max_settings_bytes"),
        (max_counter_rows, "max_counter_rows"),
        (max_note_rows, "max_note_rows"),
    ):
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise MigrationError(f"{label} must be a non-negative integer")
    if target_existing_moving_chats > moving_chat_count:
        raise MigrationError("target has more moving chats than the input list")
    if target_existing_moving_settings_rows > moving_settings_rows:
        raise MigrationError("target has more moving settings rows than the source")
    if target_existing_moving_settings_bytes > moving_settings_bytes:
        raise MigrationError("target has more moving setting bytes than the source")
    if target_existing_moving_counter_rows > moving_counter_rows:
        raise MigrationError("target has more moving counter rows than the source")
    if target_existing_moving_note_rows > moving_note_rows:
        raise MigrationError("target has more moving note rows than the source")

    new_chats = moving_chat_count - target_existing_moving_chats
    new_settings_rows = moving_settings_rows - target_existing_moving_settings_rows
    new_settings_bytes = moving_settings_bytes - target_existing_moving_settings_bytes
    new_counter_rows = moving_counter_rows - target_existing_moving_counter_rows
    new_note_rows = moving_note_rows - target_existing_moving_note_rows
    projected_chats = target_chat_count + new_chats
    projected_settings_rows = target_settings_rows + new_settings_rows
    projected_settings_bytes = target_settings_bytes + new_settings_bytes
    projected_counter_rows = target_counter_rows + new_counter_rows
    projected_note_rows = target_note_rows + new_note_rows
    if projected_chats > max_chats:
        raise MigrationError(
            f"target would have {projected_chats} chats, above MAX_SHARD_CHATS={max_chats}"
        )
    if projected_settings_rows > max_settings_rows:
        raise MigrationError(
            "target would have "
            f"{projected_settings_rows} settings rows, above "
            f"MAX_SHARD_SETTINGS_ROWS={max_settings_rows}"
        )
    if projected_settings_bytes > max_settings_bytes:
        raise MigrationError(
            "target would have "
            f"{projected_settings_bytes} setting bytes, above "
            f"MAX_SHARD_SETTINGS_BYTES={max_settings_bytes}"
        )
    if projected_counter_rows > max_counter_rows:
        raise MigrationError(
            "target would have "
            f"{projected_counter_rows} counter rows, above "
            f"MAX_SHARD_COUNTER_ROWS={max_counter_rows}"
        )
    if projected_note_rows > max_note_rows:
        raise MigrationError(
            "target would have "
            f"{projected_note_rows} note rows, above "
            f"MAX_SHARD_NOTE_ROWS={max_note_rows}"
        )
    return {
        "target_chat_count": target_chat_count,
        "target_settings_rows": target_settings_rows,
        "target_settings_bytes": target_settings_bytes,
        "moving_chat_count": moving_chat_count,
        "moving_settings_rows": moving_settings_rows,
        "moving_settings_bytes": moving_settings_bytes,
        "target_existing_moving_chats": target_existing_moving_chats,
        "target_existing_moving_settings_rows": target_existing_moving_settings_rows,
        "target_existing_moving_settings_bytes": target_existing_moving_settings_bytes,
        "new_chats": new_chats,
        "new_settings_rows": new_settings_rows,
        "new_settings_bytes": new_settings_bytes,
        "target_counter_rows": target_counter_rows,
        "moving_counter_rows": moving_counter_rows,
        "target_existing_moving_counter_rows": target_existing_moving_counter_rows,
        "new_counter_rows": new_counter_rows,
        "projected_counter_rows": projected_counter_rows,
        "target_note_rows": target_note_rows,
        "moving_note_rows": moving_note_rows,
        "target_existing_moving_note_rows": target_existing_moving_note_rows,
        "new_note_rows": new_note_rows,
        "projected_note_rows": projected_note_rows,
        "projected_chats": projected_chats,
        "projected_settings_rows": projected_settings_rows,
        "projected_settings_bytes": projected_settings_bytes,
        "max_chats": max_chats,
        "max_settings_rows": max_settings_rows,
        "max_settings_bytes": max_settings_bytes,
        "max_counter_rows": max_counter_rows,
        "max_note_rows": max_note_rows,
    }


def _placeholder_count(count: int) -> str:
    return ", ".join("%s" for _ in range(count))


def _any_query(table: TableSpec, columns: str = "*") -> str:
    return f"SELECT {columns} FROM {table.name} WHERE chat_id = ANY(%s)"


def fetch_rows(connection: Any, table: TableSpec, chat_ids: Sequence[int]) -> list[tuple[Any, ...]]:
    order = ", ".join(table.order_by)
    with connection.cursor() as cursor:
        cursor.execute(f"{_any_query(table, ', '.join(table.columns))} ORDER BY {order}", (list(chat_ids),))
        return list(cursor.fetchall())


def fetch_count(connection: Any, table: TableSpec, chat_ids: Sequence[int]) -> int:
    with connection.cursor() as cursor:
        cursor.execute(f"SELECT count(*) FROM {table.name} WHERE chat_id = ANY(%s)", (list(chat_ids),))
        return int(cursor.fetchone()[0])


def fetch_setting_bytes(connection: Any, chat_ids: Sequence[int]) -> int:
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT COALESCE(sum(octet_length(key) + octet_length(value)), 0) "
            "FROM settings WHERE chat_id = ANY(%s)",
            (list(chat_ids),),
        )
        return int(cursor.fetchone()[0])


def fetch_chat_ids(connection: Any, table: TableSpec, chat_ids: Sequence[int]) -> set[int]:
    with connection.cursor() as cursor:
        cursor.execute(f"SELECT DISTINCT chat_id FROM {table.name} WHERE chat_id = ANY(%s)", (list(chat_ids),))
        return {int(row[0]) for row in cursor.fetchall()}


def fetch_target_capacity(
    connection: Any, chat_ids: Sequence[int]
) -> tuple[int, int, int, int, int, set[int]]:
    """Return target settings counts and all moving chat ids already present in any table."""

    with connection.cursor() as cursor:
        cursor.execute("SELECT count(DISTINCT chat_id) FROM settings WHERE chat_id <> 0")
        target_chat_count = int(cursor.fetchone()[0])
        cursor.execute("SELECT count(*) FROM settings")
        target_settings_rows = int(cursor.fetchone()[0])
        cursor.execute(
            "SELECT COALESCE(sum(octet_length(key) + octet_length(value)), 0) "
            "FROM settings"
        )
        target_settings_bytes = int(cursor.fetchone()[0])
        cursor.execute("SELECT count(*) FROM counters")
        target_counter_rows = int(cursor.fetchone()[0])
        cursor.execute("SELECT count(*) FROM notes")
        target_note_rows = int(cursor.fetchone()[0])
    existing: set[int] = set()
    for table in CHAT_TABLES:
        existing.update(fetch_chat_ids(connection, table, chat_ids))
    return (
        target_chat_count,
        target_settings_rows,
        target_settings_bytes,
        target_counter_rows,
        target_note_rows,
        existing,
    )


def preflight(
    source: Any,
    target: Any,
    chat_ids: Sequence[int],
    *,
    max_chats: int = DEFAULT_MAX_CHATS,
    max_settings_rows: int = DEFAULT_MAX_SETTINGS_ROWS,
    max_settings_bytes: int = DEFAULT_MAX_SETTINGS_BYTES,
    max_counter_rows: int = DEFAULT_MAX_COUNTER_ROWS,
    max_note_rows: int = DEFAULT_MAX_NOTE_ROWS,
) -> dict[str, Any]:
    source_settings_rows = fetch_count(source, CHAT_TABLES[0], chat_ids)
    source_settings_bytes = fetch_setting_bytes(source, chat_ids)
    (
        target_chat_count,
        target_settings_rows,
        target_settings_bytes,
        target_counter_rows,
        target_note_rows,
        existing_ids,
    ) = fetch_target_capacity(target, chat_ids)
    target_existing_settings_rows = fetch_count(target, CHAT_TABLES[0], chat_ids)
    target_existing_settings_bytes = fetch_setting_bytes(target, chat_ids)
    target_existing_counter_rows = fetch_count(target, CHAT_TABLES[1], chat_ids)
    moving_counter_rows = fetch_count(source, CHAT_TABLES[1], chat_ids)
    target_existing_note_rows = fetch_count(target, CHAT_TABLES[2], chat_ids)
    moving_note_rows = fetch_count(source, CHAT_TABLES[2], chat_ids)
    limits = validate_limits(
        target_chat_count=target_chat_count,
        target_settings_rows=target_settings_rows,
        moving_chat_count=len(chat_ids),
        moving_settings_rows=source_settings_rows,
        target_settings_bytes=target_settings_bytes,
        moving_settings_bytes=source_settings_bytes,
        target_existing_moving_chats=len(existing_ids),
        target_existing_moving_settings_rows=target_existing_settings_rows,
        target_existing_moving_settings_bytes=target_existing_settings_bytes,
        target_counter_rows=target_counter_rows,
        moving_counter_rows=moving_counter_rows,
        target_existing_moving_counter_rows=target_existing_counter_rows,
        target_note_rows=target_note_rows,
        moving_note_rows=moving_note_rows,
        target_existing_moving_note_rows=target_existing_note_rows,
        max_chats=max_chats,
        max_settings_rows=max_settings_rows,
        max_settings_bytes=max_settings_bytes,
        max_counter_rows=max_counter_rows,
        max_note_rows=max_note_rows,
    )
    source_counts = {table.name: fetch_count(source, table, chat_ids) for table in CHAT_TABLES}
    target_counts = {table.name: fetch_count(target, table, chat_ids) for table in CHAT_TABLES}
    return {
        "chat_ids": len(chat_ids),
        "source_rows": source_counts,
        "target_rows_for_chat_ids": target_counts,
        "capacity": limits,
    }


def _set_isolation(connection: Any, level: str) -> None:
    with connection.cursor() as cursor:
        cursor.execute(f"SET TRANSACTION ISOLATION LEVEL {level}")


def _insert_rows(connection: Any, table: TableSpec, rows: Sequence[tuple[Any, ...]]) -> None:
    if not rows:
        return
    columns = ", ".join(table.columns)
    values = _placeholder_count(len(table.columns))
    with connection.cursor() as cursor:
        cursor.executemany(
            f"INSERT INTO {table.name} ({columns}) VALUES ({values}) ON CONFLICT DO NOTHING",
            rows,
        )


def migrate_batch(source: Any, target: Any, chat_ids: Sequence[int], max_rows: int) -> dict[str, int]:
    """Copy, verify, and remove one chat batch; retries accept exact target copies."""

    with source.transaction():
        _set_isolation(source, "REPEATABLE READ")
        source_rows = {table.name: fetch_rows(source, table, chat_ids) for table in CHAT_TABLES}
        total_rows = sum(len(rows) for rows in source_rows.values())
        if total_rows > max_rows:
            raise MigrationError(
                f"batch of {len(chat_ids)} chats contains {total_rows} rows, above "
                f"--max-rows-per-batch={max_rows}"
            )

        with target.transaction():
            _set_isolation(target, "SERIALIZABLE")
            for table in CHAT_TABLES:
                existing_rows = fetch_rows(target, table, chat_ids)
                expected_rows = source_rows[table.name]
                if existing_rows and existing_rows != expected_rows:
                    raise MigrationError(
                        f"target {table.name} rows for this batch are not an exact source match"
                    )
                if not existing_rows:
                    _insert_rows(target, table, expected_rows)
                verified_rows = fetch_rows(target, table, chat_ids)
                if verified_rows != expected_rows:
                    raise MigrationError(f"target verification failed for {table.name}")

        deleted: dict[str, int] = {}
        for table in CHAT_TABLES:
            with source.cursor() as cursor:
                cursor.execute(f"DELETE FROM {table.name} WHERE chat_id = ANY(%s)", (list(chat_ids),))
                deleted[table.name] = int(cursor.rowcount)
        return {"rows_copied": total_rows, "rows_deleted": sum(deleted.values()), **deleted}


def _open_connection(dsn_env: str) -> Any:
    dsn = os.environ.get(dsn_env)
    if not dsn:
        raise MigrationError(f"environment variable {dsn_env} is not set")
    try:
        import psycopg  # type: ignore[import-not-found]
    except ImportError as error:
        raise MigrationError(
            "psycopg is required for database migration; install "
            '`psycopg[binary]>=3.1,<4`'
        ) from error
    return psycopg.connect(dsn)


def _read_input(path: Path | None, chat_ids: Sequence[int] | None, shard: str | None) -> list[int]:
    ids = list(chat_ids or [])
    for chat_id in ids:
        if not isinstance(chat_id, int) or not MIN_CHAT_ID <= chat_id <= MAX_CHAT_ID:
            raise MigrationError("chat id is outside BIGINT range")
        if chat_id == 0:
            raise MigrationError("chat id 0 is global state, not a shard chat")
    if path is not None:
        try:
            ids.extend(parse_chat_ids(path.read_text(encoding="utf-8").splitlines(), str(path), shard))
        except OSError as error:
            raise MigrationError(str(error)) from error
    if not ids:
        raise MigrationError("provide --chat-id or --input")
    if len(set(ids)) != len(ids):
        raise MigrationError("duplicate chat id")
    return ids


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="chat_ids.txt or shard_route TSV output")
    parser.add_argument("--shard", help="when input is TSV, migrate only rows assigned to this shard")
    parser.add_argument("--chat-id", action="append", type=int, dest="chat_ids")
    parser.add_argument("--source-dsn-env", default="SOURCE_DATABASE_URL")
    parser.add_argument("--target-dsn-env", default="TARGET_DATABASE_URL")
    parser.add_argument("--max-chats", type=int, default=DEFAULT_MAX_CHATS)
    parser.add_argument("--max-settings-rows", type=int, default=DEFAULT_MAX_SETTINGS_ROWS)
    parser.add_argument(
        "--max-settings-bytes", type=int, default=DEFAULT_MAX_SETTINGS_BYTES
    )
    parser.add_argument("--max-counter-rows", type=int, default=DEFAULT_MAX_COUNTER_ROWS)
    parser.add_argument("--max-note-rows", type=int, default=DEFAULT_MAX_NOTE_ROWS)
    parser.add_argument("--batch-chats", type=int, default=DEFAULT_BATCH_CHATS)
    parser.add_argument("--max-rows-per-batch", type=int, default=DEFAULT_MAX_ROWS_PER_BATCH)
    parser.add_argument("--apply", action="store_true", help="copy and delete rows; default is read-only preflight")
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if (
            args.max_chats < 1
            or args.max_settings_rows < 1
            or args.max_settings_bytes < 1
            or args.max_counter_rows < 1
            or args.max_note_rows < 1
        ):
            raise MigrationError("capacity limits must be positive")
        if not 1 <= args.batch_chats <= 1_000:
            raise MigrationError("--batch-chats must be between 1 and 1000")
        if not 1 <= args.max_rows_per_batch <= 1_000_000:
            raise MigrationError("--max-rows-per-batch must be between 1 and 1000000")
        chat_ids = _read_input(args.input, args.chat_ids, args.shard)
        source = _open_connection(args.source_dsn_env)
        target = _open_connection(args.target_dsn_env)
        try:
            report = preflight(
                source,
                target,
                chat_ids,
                max_chats=args.max_chats,
                max_settings_rows=args.max_settings_rows,
                max_settings_bytes=args.max_settings_bytes,
                max_counter_rows=args.max_counter_rows,
                max_note_rows=args.max_note_rows,
            )
            if args.apply:
                # The read-only preflight opened a transaction on each connection.  End it before
                # migrate_batch starts its repeatable-read source and serializable target txns.
                source.commit()
                target.commit()
                batches: list[dict[str, int]] = []
                for start in range(0, len(chat_ids), args.batch_chats):
                    batches.append(
                        migrate_batch(
                            source,
                            target,
                            chat_ids[start : start + args.batch_chats],
                            args.max_rows_per_batch,
                        )
                    )
                report["batches"] = batches
                report["applied"] = True
            else:
                report["applied"] = False
        finally:
            source.close()
            target.close()
    except (MigrationError, OSError) as error:
        print(f"shard migrate: invalid: {error}")
        return 2
    if args.json:
        print(json.dumps(report, sort_keys=True))
    else:
        mode = "applied" if report["applied"] else "preflight only"
        capacity = report["capacity"]
        print(
            f"{mode}: {report['chat_ids']} chats, "
            f"{sum(report['source_rows'].values())} source rows; "
            f"target projects to {capacity['projected_chats']} chats and "
            f"{capacity['projected_settings_rows']} settings rows"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
