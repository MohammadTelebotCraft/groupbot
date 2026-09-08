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
import hashlib
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
    TableSpec(
        "durable_chats",
        ("chat_id", "access_hash", "admitted_at"),
        ("chat_id",),
    ),
    TableSpec("settings", ("chat_id", "key", "value"), ("chat_id", "key")),
    TableSpec(
        "default_rights_state",
        (
            "chat_id",
            "base_mask",
            "seeded",
            "manual_lock",
            "timed_until",
            "night_from",
            "night_to",
            "intent_revision",
            "intent_fingerprint",
            "applied_revision",
            "applied_fingerprint",
            "remote_unknown",
            "delivery_state",
            "lease_token",
            "lease_revision",
            "lease_until",
            "retry_at",
            "attempts",
            "last_error",
            "next_transition_at",
            "intent_notice",
            "notice_kind",
            "notice_revision",
            "notice_token",
            "notice_lease_until",
            "notice_retry_at",
            "notice_attempts",
            "notice_error",
        ),
        ("chat_id",),
    ),
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
        "pending_captchas",
        (
            "chat_id",
            "user_id",
            "answer",
            "source_message_id",
            "message_id",
            "due_at",
            "retry_at",
            "failure_action",
            "state",
            "attempts",
            "generation",
            "version",
            "lease_token",
            "restriction_until",
            "kick_until",
            "quarantined_at",
            "terminal_reason",
        ),
        ("chat_id", "user_id"),
    ),
    TableSpec(
        "pending_warn_actions",
        (
            "chat_id",
            "user_id",
            "penalty",
            "created_at",
            "claimed_until",
            "attempts",
            "generation",
            "version",
            "lease_token",
            "awaiting_rejoin",
            "last_error",
        ),
        ("chat_id", "user_id"),
    ),
    TableSpec(
        "pending_strict_actions",
        (
            "chat_id",
            "user_id",
            "action",
            "duration_seconds",
            "until_date",
            "threshold",
            "target_name",
            "wipe_history",
            "created_at",
            "available_at",
            "attempts",
            "generation",
            "version",
            "lease_token",
            "awaiting_rejoin",
            "terminal_at",
            "last_error",
        ),
        ("chat_id", "user_id"),
    ),
    TableSpec(
        "pending_rank_awards",
        (
            "chat_id",
            "user_id",
            "name",
            "total",
            "awarded",
            "milestone",
            "version",
            "lease_token",
            "lease_until",
            "attempts",
            "terminal_reason",
            "terminal_at",
            "created_at",
        ),
        ("chat_id", "user_id"),
    ),
    TableSpec(
        "moderation_cases",
        (
            "id",
            "chat_id",
            "subject_user_id",
            "subject_name",
            "source",
            "rule_key",
            "reason",
            "message_id",
            "media_kind",
            "evidence_text",
            "evidence_hash",
            "primary_action",
            "action_until",
            "status",
            "actor_id",
            "actor_name",
            "created_at",
            "updated_at",
            "workflow_key",
        ),
        ("chat_id", "id"),
    ),
    TableSpec(
        "moderation_case_events",
        (
            "id",
            "chat_id",
            "case_id",
            "kind",
            "actor_id",
            "actor_name",
            "action",
            "note",
            "created_at",
        ),
        ("chat_id", "id"),
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
DEFAULT_MAX_TALLY_ROWS = 1_000_000
MIN_MAX_TALLY_ROWS = 10_000
ABSOLUTE_MAX_TALLY_ROWS = 10_000_000
DEFAULT_MAX_NOTE_ROWS = 5_000_000
DEFAULT_MAX_PENDING_CAPTCHA_ROWS = 500_000
DEFAULT_BATCH_CHATS = 100
DEFAULT_MAX_ROWS_PER_BATCH = 250_000
MAX_COUNTER_ROWS_PER_CHAT = 20_000
MAX_TALLY_ROWS_PER_CHAT = 64
MAX_TALLY_COUNTER_BYTES = 64
MAX_PENDING_CAPTCHAS_PER_CHAT = 256
MAX_TELEGRAM_USER_ID = 0xFFFFFFFFFF


def _storage_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise MigrationError(f"{label} must be a database integer")
    if not -(1 << 63) <= value <= (1 << 63) - 1:
        raise MigrationError(f"{label} is outside PostgreSQL BIGINT range")
    return value


def _valid_group_chat(chat: int) -> bool:
    return (
        -999_999_999_999 <= chat <= -1
        or -1_997_852_516_352 <= chat <= -1_000_000_000_001
        or -4_000_000_000_000 <= chat <= -2_002_147_483_649
    )

# Kept in parity with `state.rs::VALIDATED_SETTING_KEYS`; the Python regression test compares
# the complete key inventory. The validator is intentionally local so preflight can reject a
# corrupt source without starting either groupbot binary or writing the target.
TYPED_SETTING_KEYS = frozenset(
    {
        "strict_limit", "strict_time", "strict_action",
        "captcha_timeout", "captcha_choices", "captcha_action",
        "warn_limit", "warn_action", "add_required", "gate_every", "gate_ttl",
        "pin_kept", "owner", "hash", "night", "night_state", "glock_until",
        "report_at", "report_day", "auto_purge_at", "auto_purge_count", "auto_purge_day",
        "cln_checked_slot", "raid_limit", "raid_window", "raid_time",
        "flood_limit", "flood_window", "cq_lim", "tmed_min", "trade_lim",
    }
)


def _bounded_int(key: str, value: str, minimum: int, maximum: int) -> int:
    digits = value[1:] if value.startswith(("+", "-")) else value
    if not digits or not digits.isascii() or not digits.isdigit():
        raise MigrationError(f"setting {key!r} contains non-integer {value!r}")
    try:
        parsed = int(value, 10)
    except ValueError as error:
        raise MigrationError(f"setting {key!r} contains non-integer {value!r}") from error
    if not minimum <= parsed <= maximum:
        raise MigrationError(
            f"setting {key!r}={value!r} is outside {minimum}..={maximum}"
        )
    return parsed


def validate_typed_setting_rows(rows: Sequence[tuple[Any, ...]], label: str) -> None:
    """Reject every migrated high-impact setting that Rust startup would reject."""

    ranges = {
        "strict_limit": (1, 20), "strict_time": (0, 10_080),
        "captcha_timeout": (30, 900), "captcha_choices": (2, 6),
        "warn_limit": (1, 100), "add_required": (0, 1_000),
        "gate_every": (0, 3_600), "gate_ttl": (0, 3_600),
        "report_at": (0, 1_439), "auto_purge_at": (0, 1_439),
        "auto_purge_count": (0, 100_000), "raid_limit": (2, 200),
        "raid_window": (5, 600), "raid_time": (1, 10_080),
        "flood_limit": (2, 50), "flood_window": (2, 120),
        "cq_lim": (5, 60), "tmed_min": (1, 1_440), "trade_lim": (30, 60),
    }
    unsigned_64 = {"report_day", "auto_purge_day", "cln_checked_slot"}
    positive_i64 = {"owner", "glock_until"}
    actions = {
        "strict_action": {"ban", "mute"},
        "captcha_action": {"kick", "mute"},
        "warn_action": {"ban", "mute"},
        "night_state": {"on", "off", "pending_on", "pending_off"},
    }
    for row in rows:
        chat, key, value = int(row[0]), str(row[1]), str(row[2])
        if key not in TYPED_SETTING_KEYS:
            continue
        try:
            if key in ranges:
                _bounded_int(key, value, *ranges[key])
            elif key in unsigned_64:
                _bounded_int(key, value, 0, (1 << 64) - 1)
            elif key in positive_i64:
                _bounded_int(key, value, 1, (1 << 63) - 1)
            elif key == "pin_kept":
                _bounded_int(key, value, 1, (1 << 31) - 1)
            elif key == "hash":
                _bounded_int(key, value, -(1 << 63), (1 << 63) - 1)
            elif key in actions:
                if value not in actions[key]:
                    raise MigrationError(f"setting {key!r} contains invalid action {value!r}")
            elif key == "night":
                parts = value.split("|")
                if len(parts) != 2:
                    raise MigrationError("setting 'night' must be FROM|TO")
                start = _bounded_int("night.from", parts[0], 0, 1_439)
                end = _bounded_int("night.to", parts[1], 0, 1_439)
                if start == end:
                    raise MigrationError("setting 'night' endpoints must differ")
            else:
                raise AssertionError(f"typed setting {key!r} has no migration rule")
        except MigrationError as error:
            raise MigrationError(f"{label} chat {chat}: {error}") from error


def validate_counter_rows(rows: Sequence[tuple[Any, ...]], label: str) -> None:
    """Validate the complete counter identity/value domain before target writes."""

    per_chat: dict[int, int] = {}
    for row in rows:
        chat = _storage_int(row[0], f"{label} counter chat_id")
        user = _storage_int(row[1], f"{label} chat {chat} counter user_id")
        if not _valid_group_chat(chat):
            raise MigrationError(f"{label} counter chat_id {chat} is not a Telegram group")
        if not 1 <= user <= MAX_TELEGRAM_USER_ID:
            raise MigrationError(f"{label} chat {chat} counter user_id {user} is invalid")
        per_chat[chat] = per_chat.get(chat, 0) + 1
        if per_chat[chat] > MAX_COUNTER_ROWS_PER_CHAT:
            raise MigrationError(
                f"{label} chat {chat} has more than {MAX_COUNTER_ROWS_PER_CHAT} counter rows"
            )
        for index, name in enumerate(
            (
                "total", "today", "day", "week", "week_at", "month", "month_at",
                "seen", "adds", "awarded", "warns", "strikes", "struck",
            ),
            start=3,
        ):
            value = _storage_int(row[index], f"{label} chat {chat} counter {name}")
            maximum = (1 << 32) - 1 if name in {"warns", "strikes"} else (1 << 63) - 1
            if not 0 <= value <= maximum:
                raise MigrationError(
                    f"{label} chat {chat} counter {name} contains corrupt value {value}"
                )


def validate_tally_rows(rows: Sequence[tuple[Any, ...]], label: str) -> None:
    per_chat: dict[int, int] = {}
    for row in rows:
        chat = _storage_int(row[0], f"{label} tally chat_id")
        if not _valid_group_chat(chat):
            raise MigrationError(f"{label} tally chat_id {chat} is not a Telegram group")
        counter = row[1]
        if (
            not isinstance(counter, str)
            or not 1 <= len(counter.encode("utf-8")) <= MAX_TALLY_COUNTER_BYTES
            or ":" in counter
        ):
            raise MigrationError(
                f"{label} chat {chat} tally counter must be 1..={MAX_TALLY_COUNTER_BYTES} "
                "UTF-8 bytes without ':'"
            )
        per_chat[chat] = per_chat.get(chat, 0) + 1
        if per_chat[chat] > MAX_TALLY_ROWS_PER_CHAT:
            raise MigrationError(
                f"{label} chat {chat} has more than {MAX_TALLY_ROWS_PER_CHAT} tally rows"
            )
        for index, name in ((2, "day"), (3, "count")):
            value = _storage_int(row[index], f"{label} chat {chat} tally {name}")
            if value < 0:
                raise MigrationError(
                    f"{label} chat {chat} tally {name} contains corrupt value {value}"
                )


def validate_default_rights_rows(rows: Sequence[tuple[Any, ...]], label: str) -> None:
    for row in rows:
        chat = _storage_int(row[0], f"{label} default-rights chat_id")
        if not _valid_group_chat(chat):
            raise MigrationError(f"{label} default-rights chat_id {chat} is invalid")
        for index, name, maximum in (
            (1, "base_mask", 16_383),
            (8, "intent_fingerprint", 32_767),
        ):
            value = _storage_int(row[index], f"{label} chat {chat} {name}")
            if not 0 <= value <= maximum:
                raise MigrationError(
                    f"{label} chat {chat} default-rights {name} is invalid: {value}"
                )
        if row[10] is not None:
            applied = _storage_int(row[10], f"{label} chat {chat} applied_fingerprint")
            if not 0 <= applied <= 32_767:
                raise MigrationError(
                    f"{label} chat {chat} default-rights applied_fingerprint is invalid: {applied}"
                )
        start, end = row[5], row[6]
        if (start is None) != (end is None):
            raise MigrationError(f"{label} chat {chat} default-rights night pair is incomplete")
        if start is not None:
            start = _storage_int(start, f"{label} chat {chat} night_from")
            end = _storage_int(end, f"{label} chat {chat} night_to")
            if not 0 <= start <= 1_439 or not 0 <= end <= 1_439 or start == end:
                raise MigrationError(
                    f"{label} chat {chat} default-rights night endpoints are invalid: {start}|{end}"
                )


def validate_pending_captcha_rows(rows: Sequence[tuple[Any, ...]], label: str) -> None:
    per_chat: dict[int, int] = {}
    for row in rows:
        chat = int(row[0])
        per_chat[chat] = per_chat.get(chat, 0) + 1
        if per_chat[chat] > MAX_PENDING_CAPTCHAS_PER_CHAT:
            raise MigrationError(
                f"{label} chat {chat} has more than {MAX_PENDING_CAPTCHAS_PER_CHAT} pending captchas"
            )


def validate_registry_ownership(
    registry_rows: Sequence[tuple[Any, ...]], chat_ids: Sequence[int], label: str
) -> None:
    """Require the authoritative admission row for every requested chat."""

    registered: set[int] = set()
    for row in registry_rows:
        chat = _storage_int(row[0], f"{label} durable_chats.chat_id")
        if not _valid_group_chat(chat):
            raise MigrationError(f"{label} durable_chats has invalid group chat id {chat}")
        _storage_int(row[1], f"{label} durable_chats.access_hash")
        admitted_at = _storage_int(row[2], f"{label} durable_chats.admitted_at")
        if admitted_at < 0:
            raise MigrationError(
                f"{label} durable_chats chat {chat} has negative admitted_at {admitted_at}"
            )
        registered.add(chat)
    missing = sorted(set(chat_ids) - registered)
    if missing:
        preview = ", ".join(str(chat) for chat in missing[:5])
        raise MigrationError(
            f"{label} durable_chats is missing {len(missing)} requested chat(s): {preview}"
        )


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
        if not _valid_group_chat(chat_id):
            raise MigrationError(
                f"{source}:{line_number}: chat id is not a valid Telegram group dialog id"
            )
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
    target_tally_rows: int = 0,
    moving_tally_rows: int = 0,
    target_existing_moving_tally_rows: int = 0,
    target_note_rows: int = 0,
    moving_note_rows: int = 0,
    target_existing_moving_note_rows: int = 0,
    target_captcha_rows: int = 0,
    moving_captcha_rows: int = 0,
    target_existing_moving_captcha_rows: int = 0,
    max_chats: int = DEFAULT_MAX_CHATS,
    max_settings_rows: int = DEFAULT_MAX_SETTINGS_ROWS,
    max_settings_bytes: int = DEFAULT_MAX_SETTINGS_BYTES,
    max_counter_rows: int = DEFAULT_MAX_COUNTER_ROWS,
    max_tally_rows: int = DEFAULT_MAX_TALLY_ROWS,
    max_note_rows: int = DEFAULT_MAX_NOTE_ROWS,
    max_pending_captcha_rows: int = DEFAULT_MAX_PENDING_CAPTCHA_ROWS,
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
        (target_tally_rows, "target_tally_rows"),
        (moving_tally_rows, "moving_tally_rows"),
        (target_existing_moving_tally_rows, "target_existing_moving_tally_rows"),
        (target_note_rows, "target_note_rows"),
        (moving_note_rows, "moving_note_rows"),
        (target_existing_moving_note_rows, "target_existing_moving_note_rows"),
        (target_captcha_rows, "target_captcha_rows"),
        (moving_captcha_rows, "moving_captcha_rows"),
        (target_existing_moving_captcha_rows, "target_existing_moving_captcha_rows"),
        (max_chats, "max_chats"),
        (max_settings_rows, "max_settings_rows"),
        (max_settings_bytes, "max_settings_bytes"),
        (max_counter_rows, "max_counter_rows"),
        (max_tally_rows, "max_tally_rows"),
        (max_note_rows, "max_note_rows"),
        (max_pending_captcha_rows, "max_pending_captcha_rows"),
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
    if target_existing_moving_tally_rows > moving_tally_rows:
        raise MigrationError("target has more moving tally rows than the source")
    if target_existing_moving_note_rows > moving_note_rows:
        raise MigrationError("target has more moving note rows than the source")
    if target_existing_moving_captcha_rows > moving_captcha_rows:
        raise MigrationError("target has more moving captcha rows than the source")

    new_chats = moving_chat_count - target_existing_moving_chats
    new_settings_rows = moving_settings_rows - target_existing_moving_settings_rows
    new_settings_bytes = moving_settings_bytes - target_existing_moving_settings_bytes
    new_counter_rows = moving_counter_rows - target_existing_moving_counter_rows
    new_tally_rows = moving_tally_rows - target_existing_moving_tally_rows
    new_note_rows = moving_note_rows - target_existing_moving_note_rows
    new_captcha_rows = moving_captcha_rows - target_existing_moving_captcha_rows
    projected_chats = target_chat_count + new_chats
    projected_settings_rows = target_settings_rows + new_settings_rows
    projected_settings_bytes = target_settings_bytes + new_settings_bytes
    projected_counter_rows = target_counter_rows + new_counter_rows
    projected_tally_rows = target_tally_rows + new_tally_rows
    projected_note_rows = target_note_rows + new_note_rows
    projected_captcha_rows = target_captcha_rows + new_captcha_rows
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
    if projected_tally_rows > max_tally_rows:
        raise MigrationError(
            "target would have "
            f"{projected_tally_rows} tally rows, above "
            f"MAX_SHARD_TALLY_ROWS={max_tally_rows}"
        )
    if projected_note_rows > max_note_rows:
        raise MigrationError(
            "target would have "
            f"{projected_note_rows} note rows, above "
            f"MAX_SHARD_NOTE_ROWS={max_note_rows}"
        )
    if projected_captcha_rows > max_pending_captcha_rows:
        raise MigrationError(
            "target would have "
            f"{projected_captcha_rows} pending captcha rows, above "
            f"MAX_SHARD_PENDING_CAPTCHAS={max_pending_captcha_rows}"
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
        "target_tally_rows": target_tally_rows,
        "moving_tally_rows": moving_tally_rows,
        "target_existing_moving_tally_rows": target_existing_moving_tally_rows,
        "new_tally_rows": new_tally_rows,
        "projected_tally_rows": projected_tally_rows,
        "target_note_rows": target_note_rows,
        "moving_note_rows": moving_note_rows,
        "target_existing_moving_note_rows": target_existing_moving_note_rows,
        "new_note_rows": new_note_rows,
        "projected_note_rows": projected_note_rows,
        "target_captcha_rows": target_captcha_rows,
        "moving_captcha_rows": moving_captcha_rows,
        "target_existing_moving_captcha_rows": target_existing_moving_captcha_rows,
        "new_captcha_rows": new_captcha_rows,
        "projected_captcha_rows": projected_captcha_rows,
        "projected_chats": projected_chats,
        "projected_settings_rows": projected_settings_rows,
        "projected_settings_bytes": projected_settings_bytes,
        "max_chats": max_chats,
        "max_settings_rows": max_settings_rows,
        "max_settings_bytes": max_settings_bytes,
        "max_counter_rows": max_counter_rows,
        "max_tally_rows": max_tally_rows,
        "max_note_rows": max_note_rows,
        "max_pending_captcha_rows": max_pending_captcha_rows,
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
) -> tuple[int, int, int, int, int, int, int, set[int]]:
    """Return target settings counts and all moving chat ids already present in any table."""

    with connection.cursor() as cursor:
        cursor.execute("SELECT count(*) FROM durable_chats")
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
        cursor.execute("SELECT count(*) FROM tallies")
        target_tally_rows = int(cursor.fetchone()[0])
        cursor.execute("SELECT count(*) FROM notes")
        target_note_rows = int(cursor.fetchone()[0])
        cursor.execute("SELECT count(*) FROM pending_captchas")
        target_captcha_rows = int(cursor.fetchone()[0])
    existing: set[int] = set()
    for table in CHAT_TABLES:
        existing.update(fetch_chat_ids(connection, table, chat_ids))
    return (
        target_chat_count,
        target_settings_rows,
        target_settings_bytes,
        target_counter_rows,
        target_tally_rows,
        target_note_rows,
        target_captcha_rows,
        existing,
    )


def _fingerprint_value(value: Any) -> Any:
    if isinstance(value, bytes):
        return {"bytes": value.hex()}
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return {"typed": type(value).__name__, "value": str(value)}


def rows_fingerprint(rows: dict[str, list[tuple[Any, ...]]], chat: int) -> str:
    material = {
        table.name: [
            [_fingerprint_value(value) for value in row]
            for row in rows[table.name]
            if int(row[table.columns.index("chat_id")]) == chat
        ]
        for table in CHAT_TABLES
    }
    encoded = json.dumps(material, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def fetch_receipts(connection: Any, chat_ids: Sequence[int]) -> dict[int, str]:
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT chat_id, fingerprint FROM shard_migration_receipts "
            "WHERE chat_id = ANY(%s)",
            (list(chat_ids),),
        )
        return {int(chat): str(fingerprint) for chat, fingerprint in cursor.fetchall()}


def classify_requested_chats(
    source: Any, target: Any, chat_ids: Sequence[int]
) -> tuple[list[int], list[int]]:
    """Separate source-owned work from provably completed target receipts."""

    registry = next(table for table in CHAT_TABLES if table.name == "durable_chats")
    source_registry = fetch_rows(source, registry, chat_ids)
    target_registry = fetch_rows(target, registry, chat_ids)
    source_owned = {int(row[0]) for row in source_registry}
    target_owned = {int(row[0]) for row in target_registry}
    validate_registry_ownership(source_registry, sorted(source_owned), "source")
    validate_registry_ownership(target_registry, sorted(target_owned), "target")
    source_present: set[int] = set()
    for table in CHAT_TABLES:
        source_present.update(fetch_chat_ids(source, table, chat_ids))
    receipts = fetch_receipts(target, chat_ids)
    pending: list[int] = []
    completed: list[int] = []
    for chat in chat_ids:
        if chat in source_owned:
            pending.append(chat)
            continue
        if chat in source_present:
            raise MigrationError(
                f"source chat {chat} has durable rows but no authoritative durable_chats owner"
            )
        receipt = receipts.get(chat)
        if chat not in target_owned or receipt is None:
            raise MigrationError(
                f"chat {chat} is neither source-owned nor covered by a target migration receipt"
            )
        target_rows = {table.name: fetch_rows(target, table, [chat]) for table in CHAT_TABLES}
        if rows_fingerprint(target_rows, chat) != receipt:
            raise MigrationError(
                f"target chat {chat} no longer matches its completed migration receipt"
            )
        completed.append(chat)
    return pending, completed


def record_migration_receipts(
    connection: Any, rows: dict[str, list[tuple[Any, ...]]], chat_ids: Sequence[int]
) -> None:
    receipts = [(chat, rows_fingerprint(rows, chat)) for chat in chat_ids]
    with connection.cursor() as cursor:
        cursor.executemany(
            "INSERT INTO shard_migration_receipts (chat_id, completed_at, fingerprint) "
            "VALUES (%s, floor(extract(epoch FROM clock_timestamp()))::BIGINT, %s) "
            "ON CONFLICT (chat_id) DO UPDATE SET "
            "completed_at = EXCLUDED.completed_at, fingerprint = EXCLUDED.fingerprint",
            receipts,
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
    max_tally_rows: int = DEFAULT_MAX_TALLY_ROWS,
    max_note_rows: int = DEFAULT_MAX_NOTE_ROWS,
    max_pending_captcha_rows: int = DEFAULT_MAX_PENDING_CAPTCHA_ROWS,
) -> dict[str, Any]:
    pending_chat_ids, completed_chat_ids = classify_requested_chats(source, target, chat_ids)
    table_by_name = {table.name: table for table in CHAT_TABLES}
    registry_table = table_by_name["durable_chats"]
    settings_table = table_by_name["settings"]
    validate_registry_ownership(
        fetch_rows(source, registry_table, pending_chat_ids), pending_chat_ids, "source"
    )
    source_setting_rows = fetch_rows(source, settings_table, pending_chat_ids)
    target_setting_rows = fetch_rows(target, settings_table, pending_chat_ids)
    validate_typed_setting_rows(source_setting_rows, "source")
    validate_typed_setting_rows(target_setting_rows, "target")
    validate_counter_rows(fetch_rows(source, table_by_name["counters"], pending_chat_ids), "source")
    validate_counter_rows(fetch_rows(target, table_by_name["counters"], pending_chat_ids), "target")
    validate_tally_rows(fetch_rows(source, table_by_name["tallies"], pending_chat_ids), "source")
    validate_tally_rows(fetch_rows(target, table_by_name["tallies"], pending_chat_ids), "target")
    validate_default_rights_rows(
        fetch_rows(source, table_by_name["default_rights_state"], pending_chat_ids), "source"
    )
    validate_default_rights_rows(
        fetch_rows(target, table_by_name["default_rights_state"], pending_chat_ids), "target"
    )
    validate_pending_captcha_rows(
        fetch_rows(source, table_by_name["pending_captchas"], pending_chat_ids), "source"
    )
    validate_pending_captcha_rows(
        fetch_rows(target, table_by_name["pending_captchas"], pending_chat_ids), "target"
    )
    source_settings_rows = fetch_count(source, settings_table, pending_chat_ids)
    source_settings_bytes = fetch_setting_bytes(source, pending_chat_ids)
    (
        target_chat_count,
        target_settings_rows,
        target_settings_bytes,
        target_counter_rows,
        target_tally_rows,
        target_note_rows,
        target_captcha_rows,
        existing_ids,
    ) = fetch_target_capacity(target, pending_chat_ids)
    target_existing_settings_rows = fetch_count(target, settings_table, pending_chat_ids)
    target_existing_settings_bytes = fetch_setting_bytes(target, pending_chat_ids)
    target_existing_counter_rows = fetch_count(target, table_by_name["counters"], pending_chat_ids)
    moving_counter_rows = fetch_count(source, table_by_name["counters"], pending_chat_ids)
    target_existing_tally_rows = fetch_count(target, table_by_name["tallies"], pending_chat_ids)
    moving_tally_rows = fetch_count(source, table_by_name["tallies"], pending_chat_ids)
    target_existing_note_rows = fetch_count(target, table_by_name["notes"], pending_chat_ids)
    moving_note_rows = fetch_count(source, table_by_name["notes"], pending_chat_ids)
    target_existing_captcha_rows = fetch_count(target, table_by_name["pending_captchas"], pending_chat_ids)
    moving_captcha_rows = fetch_count(source, table_by_name["pending_captchas"], pending_chat_ids)
    limits = validate_limits(
        target_chat_count=target_chat_count,
        target_settings_rows=target_settings_rows,
        moving_chat_count=len(pending_chat_ids),
        moving_settings_rows=source_settings_rows,
        target_settings_bytes=target_settings_bytes,
        moving_settings_bytes=source_settings_bytes,
        target_existing_moving_chats=len(existing_ids),
        target_existing_moving_settings_rows=target_existing_settings_rows,
        target_existing_moving_settings_bytes=target_existing_settings_bytes,
        target_counter_rows=target_counter_rows,
        moving_counter_rows=moving_counter_rows,
        target_existing_moving_counter_rows=target_existing_counter_rows,
        target_tally_rows=target_tally_rows,
        moving_tally_rows=moving_tally_rows,
        target_existing_moving_tally_rows=target_existing_tally_rows,
        target_note_rows=target_note_rows,
        moving_note_rows=moving_note_rows,
        target_existing_moving_note_rows=target_existing_note_rows,
        target_captcha_rows=target_captcha_rows,
        moving_captcha_rows=moving_captcha_rows,
        target_existing_moving_captcha_rows=target_existing_captcha_rows,
        max_chats=max_chats,
        max_settings_rows=max_settings_rows,
        max_settings_bytes=max_settings_bytes,
        max_counter_rows=max_counter_rows,
        max_tally_rows=max_tally_rows,
        max_note_rows=max_note_rows,
        max_pending_captcha_rows=max_pending_captcha_rows,
    )
    source_counts = {table.name: fetch_count(source, table, pending_chat_ids) for table in CHAT_TABLES}
    target_counts = {table.name: fetch_count(target, table, pending_chat_ids) for table in CHAT_TABLES}
    return {
        "chat_ids": len(chat_ids),
        "pending_chat_ids": pending_chat_ids,
        "completed_chat_ids": completed_chat_ids,
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


def _advance_serial_sequence(connection: Any, table: TableSpec) -> None:
    if table.name not in {"moderation_cases", "moderation_case_events"}:
        return
    with connection.cursor() as cursor:
        cursor.execute(
            f"SELECT setval(pg_get_serial_sequence(%s, 'id'), "
            f"(SELECT max(id) FROM {table.name}), true)",
            (table.name,),
        )


def _advance_durable_work_sequence(connection: Any) -> None:
    """Keep imported generation/lease fencing identities below every future nextval."""

    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT setval('durable_work_token_seq', GREATEST("
            "(SELECT last_value FROM durable_work_token_seq), "
            "COALESCE((SELECT max(generation) FROM pending_captchas), 1), "
            "COALESCE((SELECT max(lease_token) FROM pending_captchas), 1), "
            "COALESCE((SELECT max(generation) FROM pending_warn_actions), 1), "
            "COALESCE((SELECT max(lease_token) FROM pending_warn_actions), 1), "
            "COALESCE((SELECT max(generation) FROM pending_strict_actions), 1), "
            "COALESCE((SELECT max(lease_token) FROM pending_strict_actions), 1), "
            "COALESCE((SELECT max(lease_token) FROM default_rights_state), 1), "
            "COALESCE((SELECT max(notice_token) FROM default_rights_state), 1)), true)"
        )


def _refresh_durable_counts(connection: Any) -> None:
    """Make singleton capacity accounting agree with rows copied/deleted in this transaction."""

    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE durable_counts SET "
            "counter_rows = (SELECT count(*) FROM counters), "
            "tally_rows = (SELECT count(*) FROM tallies), "
            "note_rows = (SELECT count(*) FROM notes), "
            "captcha_rows = (SELECT count(*) FROM pending_captchas) "
            "WHERE id = 0"
        )


def require_stopped_owner(connection: Any, label: str) -> None:
    """Prove that no live groupbot owns the database before moving leased work."""

    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT pg_try_advisory_lock(hashtextextended('groupbot:process', 0))"
        )
        row = cursor.fetchone()
    if not row or row[0] is not True:
        raise MigrationError(f"{label} groupbot owner is still running")


def validate_moderation_relations(rows: dict[str, list[tuple[Any, ...]]]) -> None:
    """Validate composite case/event ownership before relying on the target FK."""

    cases = {(int(row[1]), int(row[0])) for row in rows["moderation_cases"]}
    for event in rows["moderation_case_events"]:
        parent = (int(event[1]), int(event[2]))
        if parent not in cases:
            raise MigrationError(
                f"moderation event {(event[1], event[0])} has no case {parent} in its chat batch"
            )


def require_idle_transaction(connection: Any, label: str) -> None:
    """Reject a caller-owned implicit transaction that would make our block a savepoint."""

    info = getattr(connection, "info", None)
    status = getattr(info, "transaction_status", None)
    # psycopg.pq.TransactionStatus.IDLE has value zero. Keep this optional so the small
    # protocol fakes used by the offline validation suite need not depend on psycopg.
    if status is not None and int(status) != 0:
        raise MigrationError(
            f"{label} connection has an active transaction before migration batch"
        )


def migrate_batch(source: Any, target: Any, chat_ids: Sequence[int], max_rows: int) -> dict[str, int]:
    """Copy, verify, and remove one chat batch; retries accept exact target copies."""

    require_idle_transaction(source, "source")
    require_idle_transaction(target, "target")
    with source.transaction():
        _set_isolation(source, "REPEATABLE READ")
        # Establish the outer target transaction before the first target query. Psycopg starts
        # an implicit transaction on reads; classifying outside this block would turn this
        # context into a savepoint and could leave the copy/receipt uncommitted while source
        # deletion proceeds.
        with target.transaction():
            _set_isolation(target, "SERIALIZABLE")
            chat_ids, completed = classify_requested_chats(source, target, chat_ids)
            if not chat_ids:
                return {
                    "rows_copied": 0,
                    "rows_deleted": 0,
                    "already_completed": len(completed),
                }
            source_rows = {
                table.name: fetch_rows(source, table, chat_ids) for table in CHAT_TABLES
            }
            validate_registry_ownership(source_rows["durable_chats"], chat_ids, "source")
            validate_typed_setting_rows(source_rows["settings"], "source")
            validate_counter_rows(source_rows["counters"], "source")
            validate_tally_rows(source_rows["tallies"], "source")
            validate_default_rights_rows(source_rows["default_rights_state"], "source")
            validate_pending_captcha_rows(source_rows["pending_captchas"], "source")
            validate_moderation_relations(source_rows)
            total_rows = sum(len(rows) for rows in source_rows.values())
            if total_rows > max_rows:
                raise MigrationError(
                    f"batch of {len(chat_ids)} chats contains {total_rows} rows, above "
                    f"--max-rows-per-batch={max_rows}"
                )

            validate_typed_setting_rows(
                fetch_rows(target, next(table for table in CHAT_TABLES if table.name == "settings"), chat_ids),
                "target",
            )
            validate_counter_rows(
                fetch_rows(target, next(table for table in CHAT_TABLES if table.name == "counters"), chat_ids),
                "target",
            )
            validate_tally_rows(
                fetch_rows(target, next(table for table in CHAT_TABLES if table.name == "tallies"), chat_ids),
                "target",
            )
            validate_default_rights_rows(
                fetch_rows(
                    target,
                    next(table for table in CHAT_TABLES if table.name == "default_rights_state"),
                    chat_ids,
                ),
                "target",
            )
            validate_pending_captcha_rows(
                fetch_rows(
                    target,
                    next(table for table in CHAT_TABLES if table.name == "pending_captchas"),
                    chat_ids,
                ),
                "target",
            )
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
                if expected_rows:
                    _advance_serial_sequence(target, table)
            if (
                source_rows["pending_captchas"]
                or source_rows["pending_warn_actions"]
                or source_rows["pending_strict_actions"]
                or source_rows["default_rights_state"]
            ):
                _advance_durable_work_sequence(target)
            _refresh_durable_counts(target)
            # This receipt commits atomically with a complete, verified target copy. It is not
            # part of CHAT_TABLES: moving the chat onward writes a fresh target receipt and the
            # source receipt disappears with its old durable owner.
            record_migration_receipts(target, source_rows, chat_ids)

        deleted: dict[str, int] = {}
        with source.cursor() as cursor:
            cursor.execute(
                "DELETE FROM shard_migration_receipts WHERE chat_id = ANY(%s)",
                (list(chat_ids),),
            )
        for table in reversed(CHAT_TABLES):
            with source.cursor() as cursor:
                cursor.execute(f"DELETE FROM {table.name} WHERE chat_id = ANY(%s)", (list(chat_ids),))
                deleted[table.name] = int(cursor.rowcount)
        _refresh_durable_counts(source)
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
        if not _valid_group_chat(chat_id):
            raise MigrationError("chat id is not a valid Telegram group dialog id")
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


def require_empty_stats_spool(path: Path | None, label: str) -> Path:
    """Fail closed unless a stopped shard's durable statistics spool exists and is empty."""

    if path is None:
        raise MigrationError(f"{label} statistics spool was not configured")
    try:
        if path.is_symlink() or not path.is_dir():
            raise MigrationError(f"{label} statistics spool is absent or is not a directory: {path}")
        if next(path.iterdir(), None) is not None:
            raise MigrationError(f"{label} statistics spool is not drained: {path}")
    except OSError as error:
        raise MigrationError(f"cannot inspect {label} statistics spool {path}: {error}") from error
    return path.resolve()


def _tally_row_limit(value: str) -> int:
    try:
        parsed = int(value, 10)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if not MIN_MAX_TALLY_ROWS <= parsed <= ABSOLUTE_MAX_TALLY_ROWS:
        raise argparse.ArgumentTypeError(
            f"must be between {MIN_MAX_TALLY_ROWS} and {ABSOLUTE_MAX_TALLY_ROWS}"
        )
    return parsed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="chat_ids.txt or shard_route TSV output")
    parser.add_argument("--shard", help="when input is TSV, migrate only rows assigned to this shard")
    parser.add_argument("--chat-id", action="append", type=int, dest="chat_ids")
    parser.add_argument("--source-dsn-env", default="SOURCE_DATABASE_URL")
    parser.add_argument("--target-dsn-env", default="TARGET_DATABASE_URL")
    parser.add_argument(
        "--source-stats-dir",
        type=Path,
        default=(
            Path(os.environ["SOURCE_STATS_DIRECTORY"])
            if "SOURCE_STATS_DIRECTORY" in os.environ
            else None
        ),
        help="empty durable-work stats directory for the stopped source shard",
    )
    parser.add_argument(
        "--target-stats-dir",
        type=Path,
        default=(
            Path(os.environ["TARGET_STATS_DIRECTORY"])
            if "TARGET_STATS_DIRECTORY" in os.environ
            else None
        ),
        help="empty durable-work stats directory for the stopped target shard",
    )
    parser.add_argument("--max-chats", type=int, default=DEFAULT_MAX_CHATS)
    parser.add_argument("--max-settings-rows", type=int, default=DEFAULT_MAX_SETTINGS_ROWS)
    parser.add_argument(
        "--max-settings-bytes", type=int, default=DEFAULT_MAX_SETTINGS_BYTES
    )
    parser.add_argument("--max-counter-rows", type=int, default=DEFAULT_MAX_COUNTER_ROWS)
    parser.add_argument(
        "--max-tally-rows",
        type=_tally_row_limit,
        default=os.environ.get("MAX_SHARD_TALLY_ROWS", str(DEFAULT_MAX_TALLY_ROWS)),
    )
    parser.add_argument("--max-note-rows", type=int, default=DEFAULT_MAX_NOTE_ROWS)
    parser.add_argument(
        "--max-pending-captchas", type=int, default=DEFAULT_MAX_PENDING_CAPTCHA_ROWS
    )
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
            or args.max_pending_captchas < 1
        ):
            raise MigrationError("capacity limits must be positive")
        if not MIN_MAX_TALLY_ROWS <= args.max_tally_rows <= ABSOLUTE_MAX_TALLY_ROWS:
            raise MigrationError(
                f"--max-tally-rows must be between {MIN_MAX_TALLY_ROWS} "
                f"and {ABSOLUTE_MAX_TALLY_ROWS}"
            )
        if not 1 <= args.batch_chats <= 1_000:
            raise MigrationError("--batch-chats must be between 1 and 1000")
        if not 1 <= args.max_rows_per_batch <= 1_000_000:
            raise MigrationError("--max-rows-per-batch must be between 1 and 1000000")
        if not args.apply:
            source_stats = require_empty_stats_spool(args.source_stats_dir, "source")
            target_stats = require_empty_stats_spool(args.target_stats_dir, "target")
            if source_stats == target_stats:
                raise MigrationError(
                    "source and target statistics spools must be different directories"
                )
        chat_ids = _read_input(args.input, args.chat_ids, args.shard)
        source = _open_connection(args.source_dsn_env)
        target = _open_connection(args.target_dsn_env)
        try:
            if args.apply:
                # Session advisory locks are retained by these connections through preflight and
                # every migration batch.  Capacity and type validation must happen after both
                # locks: otherwise a live target can admit rows after a stale preflight, stop,
                # and let the migration exceed a hard cap.
                require_stopped_owner(source, "source")
                require_stopped_owner(target, "target")
                # Spool emptiness is part of the same stopped-owner proof. Checking it before the
                # database locks would let a live source stage counters between inspection and
                # shutdown, then replay them into the shard that no longer owns the chats.
                source_stats = require_empty_stats_spool(args.source_stats_dir, "source")
                target_stats = require_empty_stats_spool(args.target_stats_dir, "target")
                if source_stats == target_stats:
                    raise MigrationError(
                        "source and target statistics spools must be different directories"
                    )
            report = preflight(
                source,
                target,
                chat_ids,
                max_chats=args.max_chats,
                max_settings_rows=args.max_settings_rows,
                max_settings_bytes=args.max_settings_bytes,
                max_counter_rows=args.max_counter_rows,
                max_tally_rows=args.max_tally_rows,
                max_note_rows=args.max_note_rows,
                max_pending_captcha_rows=args.max_pending_captchas,
            )
            if args.apply:
                # The locked read-only preflight opened a transaction on each connection. End it
                # before migrate_batch starts its repeatable-read source and serializable target
                # transactions. Session advisory locks survive these commits.
                source.commit()
                target.commit()
                batches: list[dict[str, int]] = []
                pending_chat_ids = report.get("pending_chat_ids", chat_ids)
                for start in range(0, len(pending_chat_ids), args.batch_chats):
                    batches.append(
                        migrate_batch(
                            source,
                            target,
                            pending_chat_ids[start : start + args.batch_chats],
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
