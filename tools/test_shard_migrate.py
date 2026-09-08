import unittest
import tempfile
import contextlib
import inspect
import io
import os
import re
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from shard_migrate import (
    CHAT_TABLES,
    DEFAULT_MAX_TALLY_ROWS,
    TYPED_SETTING_KEYS,
    MigrationError,
    _advance_durable_work_sequence,
    _advance_serial_sequence,
    _refresh_durable_counts,
    fetch_target_capacity,
    _read_input,
    main,
    migrate_batch,
    parse_chat_ids,
    parse_args,
    preflight,
    require_idle_transaction,
    require_empty_stats_spool,
    require_stopped_owner,
    validate_limits,
    validate_moderation_relations,
    validate_counter_rows,
    validate_default_rights_rows,
    validate_registry_ownership,
    validate_tally_rows,
    validate_typed_setting_rows,
    rows_fingerprint,
)


class ShardMigrateTests(unittest.TestCase):
    def test_every_moved_chat_requires_its_authoritative_registry_row(self):
        validate_registry_ownership([(-100, 42, 0)], [-100], "source")
        with self.assertRaisesRegex(MigrationError, "durable_chats is missing"):
            validate_registry_ownership([(-100, 42, 0)], [-100, -200], "source")
        for row in ((0, 42, 0), (7, 42, 0), (-100, 42, -1)):
            with self.subTest(row=row), self.assertRaises(MigrationError):
                validate_registry_ownership([row], [row[0]], "source")

    def test_validation_precedes_registry_cast_and_every_target_write(self):
        source = (Path(__file__).parents[1] / "src" / "state.rs").read_text(encoding="utf-8")
        startup = source.split("async fn connect_inner(", 1)[1].split(
            "let durable_chats = sqlx::query_as", 1
        )[0]
        self.assertLess(
            startup.index("Self::validate_persisted_settings"),
            startup.index('"INSERT INTO durable_chats'),
        )
        self.assertLess(
            startup.index("Self::validate_durable_chat_ids"),
            startup.index('"INSERT INTO durable_chats'),
        )

        migration = inspect.getsource(migrate_batch)
        first_target_write = migration.index("_insert_rows(target")
        self.assertLess(
            migration.rindex("validate_typed_setting_rows", 0, first_target_write),
            first_target_write,
        )
        self.assertLess(
            migration.rindex("validate_counter_rows", 0, first_target_write),
            first_target_write,
        )
        self.assertLess(
            migration.rindex("validate_tally_rows", 0, first_target_write),
            first_target_write,
        )
        self.assertLess(
            migration.rindex("validate_default_rights_rows", 0, first_target_write),
            first_target_write,
        )
        self.assertLess(
            migration.rindex("validate_pending_captcha_rows", 0, first_target_write),
            first_target_write,
        )

    def test_batch_rejects_an_implicit_transaction_before_copy(self):
        idle = SimpleNamespace(info=SimpleNamespace(transaction_status=0))
        active = SimpleNamespace(info=SimpleNamespace(transaction_status=2))
        require_idle_transaction(idle, "source")
        with self.assertRaisesRegex(MigrationError, "target connection has an active"):
            require_idle_transaction(active, "target")

        migration = inspect.getsource(migrate_batch)
        self.assertLess(
            migration.index('require_idle_transaction(target, "target")'),
            migration.index("with source.transaction()"),
        )
        self.assertLess(
            migration.index("with target.transaction()"),
            migration.index("classify_requested_chats"),
        )
        self.assertLess(
            migration.index("record_migration_receipts"),
            migration.index("DELETE FROM shard_migration_receipts"),
        )

    def test_typed_setting_inventory_matches_rust_startup_contract(self):
        source = (Path(__file__).parents[1] / "src" / "state.rs").read_text(encoding="utf-8")
        body = source.split("const VALIDATED_SETTING_KEYS: &[&str] = &[", 1)[1].split("];", 1)[0]
        self.assertEqual(set(re.findall(r'"([a-z0-9_]+)"', body)), set(TYPED_SETTING_KEYS))

    def test_typed_settings_are_validated_before_migration(self):
        valid = [
            (-100, "strict_limit", "20"),
            (-100, "hash", "-9223372036854775808"),
            (-100, "night", "1380|360"),
            (-100, "captcha_action", "kick"),
        ]
        validate_typed_setting_rows(valid, "source")
        for key, value in [
            ("strict_limit", "0"),
            ("hash", "9223372036854775808"),
            ("night", "60|60"),
            ("captcha_action", "allow"),
            ("report_day", "-1"),
        ]:
            with self.subTest(key=key, value=value), self.assertRaisesRegex(
                MigrationError, "source chat -100"
            ):
                validate_typed_setting_rows([(-100, key, value)], "source")

    def test_counter_migration_rejects_every_invalid_identity_and_value(self):
        row = [-100, 7, "member"] + [0] * 13
        validate_counter_rows([tuple(row)], "source")
        for index in range(3, 16):
            corrupt = row.copy()
            corrupt[index] = -1
            with self.subTest(index=index), self.assertRaisesRegex(
                MigrationError, "corrupt value -1"
            ):
                validate_counter_rows([tuple(corrupt)], "source")
        for chat in (0, 7, -1_000_000_000_000, -2_000_000_000_000):
            corrupt = row.copy()
            corrupt[0] = chat
            with self.subTest(chat=chat), self.assertRaises(MigrationError):
                validate_counter_rows([tuple(corrupt)], "source")
        for user in (0, -1, 0x10000000000):
            corrupt = row.copy()
            corrupt[1] = user
            with self.subTest(user=user), self.assertRaises(MigrationError):
                validate_counter_rows([tuple(corrupt)], "source")
        corrupt = row.copy()
        corrupt[3] = str(1)
        with self.assertRaisesRegex(MigrationError, "database integer"):
            validate_counter_rows([tuple(corrupt)], "source")

    def test_tally_and_default_rights_domains_are_preflighted(self):
        validate_tally_rows([(-100, "messages", 1, 2)], "source")
        for row in [
            (-100, "", 1, 2),
            (-100, "bad:name", 1, 2),
            (-100, "x" * 65, 1, 2),
            (-100, "é" * 33, 1, 2),
            (-100, "messages", -1, 2),
            (7, "messages", 1, 2),
        ]:
            with self.subTest(row=row), self.assertRaises(MigrationError):
                validate_tally_rows([row], "source")
        with self.assertRaisesRegex(MigrationError, "more than 64 tally rows"):
            validate_tally_rows(
                [(-100, f"counter-{index}", 1, 2) for index in range(65)], "source"
            )

        rights = [-100, 0, True, False, None, None, None, 1, 0, 1, 0] + [None] * 17
        validate_default_rights_rows([tuple(rights)], "source")
        for index, value in ((1, -1), (1, 16_384), (8, 32_768), (10, -1)):
            corrupt = rights.copy()
            corrupt[index] = value
            with self.subTest(index=index), self.assertRaises(MigrationError):
                validate_default_rights_rows([tuple(corrupt)], "source")
        corrupt = rights.copy()
        corrupt[5:7] = [60, 60]
        with self.assertRaisesRegex(MigrationError, "night endpoints"):
            validate_default_rights_rows([tuple(corrupt)], "source")

    def test_migration_receipt_fingerprint_covers_every_chat_table(self):
        rows = {table.name: [] for table in CHAT_TABLES}
        rows["durable_chats"] = [(-100, 42, 7)]
        rows["settings"] = [(-100, "owner", "1")]
        first = rows_fingerprint(rows, -100)
        rows["settings"] = [(-100, "owner", "2")]
        self.assertNotEqual(first, rows_fingerprint(rows, -100))
        rows["settings"] = [(-100, "owner", "1")]
        rows["counters"] = [(-100, 7, "member") + (0,) * 13]
        self.assertNotEqual(first, rows_fingerprint(rows, -100))

    def test_target_chat_capacity_unions_all_chat_identity_tables(self):
        statements = []

        class Cursor:
            def __init__(self):
                self.statement = ""

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def execute(self, statement, _parameters=None):
                self.statement = statement
                statements.append(statement)

            def fetchone(self):
                if "count(*) FROM durable_chats" in self.statement:
                    return (2,)
                if "count(*) FROM tallies" in self.statement:
                    return (7,)
                return (0,)

            @staticmethod
            def fetchall():
                return []

        class Connection:
            @staticmethod
            def cursor():
                return Cursor()

        capacity = fetch_target_capacity(Connection(), [-100])
        self.assertEqual(capacity[0], 2)
        self.assertEqual(capacity[4], 7)
        self.assertTrue(
            any(
                "count(*) FROM durable_chats" in statement
                for statement in statements
            )
        )
        self.assertTrue(any("count(*) FROM tallies" in statement for statement in statements))

    def test_chat_tables_include_every_durable_chat_workflow(self):
        tables = {table.name: table for table in CHAT_TABLES}
        self.assertEqual(
            set(tables),
            {
                "durable_chats",
                "settings",
                "default_rights_state",
                "counters",
                "notes",
                "tallies",
                "pending_deletes",
                "pending_captchas",
                "pending_warn_actions",
                "pending_strict_actions",
                "pending_rank_awards",
                "moderation_cases",
                "moderation_case_events",
                "image_filters",
            },
        )
        self.assertEqual(
            tables["pending_captchas"].columns,
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
        )
        self.assertEqual(
            tables["default_rights_state"].columns,
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
        )
        self.assertEqual(
            tables["pending_warn_actions"].columns,
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
        )
        self.assertEqual(
            tables["pending_strict_actions"].columns,
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
        )
        self.assertEqual(
            tables["pending_rank_awards"].columns,
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
        )
        self.assertIn("workflow_key", tables["moderation_cases"].columns)
        self.assertLess(
            [table.name for table in CHAT_TABLES].index("moderation_cases"),
            [table.name for table in CHAT_TABLES].index("moderation_case_events"),
        )
        self.assertLess(
            [table.name for table in reversed(CHAT_TABLES)].index("moderation_case_events"),
            [table.name for table in reversed(CHAT_TABLES)].index("moderation_cases"),
        )

    def test_stats_spools_must_exist_and_be_drained(self):
        with tempfile.TemporaryDirectory() as directory:
            spool = Path(directory)
            self.assertEqual(require_empty_stats_spool(spool, "source"), spool.resolve())
            (spool / "pending.json").write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(MigrationError, "not drained"):
                require_empty_stats_spool(spool, "source")
        with self.assertRaisesRegex(MigrationError, "not configured"):
            require_empty_stats_spool(None, "source")
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "missing"
            with self.assertRaisesRegex(MigrationError, "absent"):
                require_empty_stats_spool(missing, "target")

    def test_command_refuses_before_database_access_without_spool_proof(self):
        output = io.StringIO()
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "sys.argv", ["shard_migrate.py", "--chat-id", "-100"]
        ), contextlib.redirect_stdout(output):
            self.assertEqual(main(), 2)
        self.assertIn("statistics spool", output.getvalue())

    def test_composite_moderation_ids_preserve_each_chat_relation(self):
        # The source and target independently allocated case 7 and event 9. Combining them is
        # collision-free because chat identity is part of both keys and relations.
        source = {
            "moderation_cases": [(7, -100, None, "source case")],
            "moderation_case_events": [(9, -100, 7, "source event")],
        }
        target = {
            "moderation_cases": [(7, -200, None, "target case")],
            "moderation_case_events": [(9, -200, 7, "target event")],
        }
        validate_moderation_relations(source)
        validate_moderation_relations(target)
        migrated = {
            name: target[name] + source[name]
            for name in ("moderation_cases", "moderation_case_events")
        }
        validate_moderation_relations(migrated)
        self.assertEqual(
            {(row[1], row[0]) for row in migrated["moderation_cases"]},
            {(-100, 7), (-200, 7)},
        )
        self.assertEqual(
            {(row[1], row[0], row[2]) for row in migrated["moderation_case_events"]},
            {(-100, 9, 7), (-200, 9, 7)},
        )
        broken = dict(migrated)
        broken["moderation_case_events"] = [(9, -200, 8, "opened")]
        with self.assertRaisesRegex(MigrationError, "has no case"):
            validate_moderation_relations(broken)

    def test_explicit_moderation_ids_advance_target_sequences(self):
        statements = []

        class Cursor:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def execute(self, statement, parameters):
                statements.append((statement, parameters))

        class Connection:
            @staticmethod
            def cursor():
                return Cursor()

        tables = {table.name: table for table in CHAT_TABLES}
        _advance_serial_sequence(Connection(), tables["moderation_cases"])
        _advance_serial_sequence(Connection(), tables["moderation_case_events"])
        self.assertEqual(len(statements), 2)
        self.assertIn("SELECT max(id) FROM moderation_cases", statements[0][0])
        self.assertIn("SELECT max(id) FROM moderation_case_events", statements[1][0])

    def test_imported_fencing_ids_advance_the_shared_durable_sequence(self):
        statements = []

        class Cursor:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def execute(self, statement):
                statements.append(statement)

        class Connection:
            @staticmethod
            def cursor():
                return Cursor()

        _advance_durable_work_sequence(Connection())
        self.assertEqual(len(statements), 1)
        statement = statements[0]
        self.assertIn("last_value FROM durable_work_token_seq", statement)
        for table in ("pending_captchas", "pending_warn_actions", "pending_strict_actions"):
            self.assertIn(f"max(generation) FROM {table}", statement)
            self.assertIn(f"max(lease_token) FROM {table}", statement)
        self.assertIn("max(lease_token) FROM default_rights_state", statement)
        self.assertIn("max(notice_token) FROM default_rights_state", statement)

    def test_apply_requires_database_owner_locks(self):
        class Cursor:
            def __init__(self, acquired):
                self.acquired = acquired

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def execute(self, statement):
                self.statement = statement

            def fetchone(self):
                return (self.acquired,)

        class Connection:
            def __init__(self, acquired):
                self.acquired = acquired

            def cursor(self):
                return Cursor(self.acquired)

        require_stopped_owner(Connection(True), "source")
        with self.assertRaisesRegex(MigrationError, "still running"):
            require_stopped_owner(Connection(False), "target")

    def test_apply_locks_both_owners_before_capacity_preflight(self):
        events = []

        class Connection:
            def __init__(self, name):
                self.name = name

            def commit(self):
                events.append(f"commit:{self.name}")

            def close(self):
                events.append(f"close:{self.name}")

        source = Connection("source")
        target = Connection("target")
        args = SimpleNamespace(
            max_chats=10,
            max_settings_rows=10,
            max_settings_bytes=1_000,
            max_counter_rows=10,
            max_tally_rows=10_000,
            max_note_rows=10,
            max_pending_captchas=10,
            batch_chats=10,
            max_rows_per_batch=100,
            source_stats_dir=Path("source-spool"),
            target_stats_dir=Path("target-spool"),
            input=None,
            chat_ids=[-100, -200],
            shard=None,
            source_dsn_env="SOURCE_DATABASE_URL",
            target_dsn_env="TARGET_DATABASE_URL",
            apply=True,
            json=True,
        )

        def lock(connection, label):
            self.assertIs(connection, source if label == "source" else target)
            events.append(f"lock:{label}")

        def check_spool(_path, label):
            events.append(f"spool:{label}")
            return Path(f"{label}-spool")

        def locked_preflight(*_args, **_kwargs):
            self.assertEqual(
                events,
                ["lock:source", "lock:target", "spool:source", "spool:target"],
            )
            events.append("preflight")
            return {
                "chat_ids": 2,
                "pending_chat_ids": [-200],
                "completed_chat_ids": [-100],
                "source_rows": {},
                "capacity": {},
            }

        def migrate(_source, _target, batch, _max_rows):
            self.assertEqual(batch, [-200])
            events.append("migrate")
            return {}

        with mock.patch("shard_migrate.parse_args", return_value=args), mock.patch(
            "shard_migrate.require_empty_stats_spool",
            side_effect=check_spool,
        ), mock.patch("shard_migrate._read_input", return_value=[-100, -200]), mock.patch(
            "shard_migrate._open_connection", side_effect=[source, target]
        ), mock.patch("shard_migrate.require_stopped_owner", side_effect=lock), mock.patch(
            "shard_migrate.preflight", side_effect=locked_preflight
        ), mock.patch("shard_migrate.migrate_batch", side_effect=migrate), contextlib.redirect_stdout(
            io.StringIO()
        ):
            self.assertEqual(main(), 0)

        self.assertEqual(
            events,
            [
                "lock:source",
                "lock:target",
                "spool:source",
                "spool:target",
                "preflight",
                "commit:source",
                "commit:target",
                "migrate",
                "close:source",
                "close:target",
            ],
        )

    def test_reads_route_tsv_and_filters_a_shard(self):
        self.assertEqual(
            parse_chat_ids(["-3\talpha", "-4\tbeta", "# comment"], shard="alpha"),
            [-3],
        )

    def test_rejects_global_state_and_duplicates(self):
        with self.assertRaises(MigrationError):
            parse_chat_ids(["0"])
        with self.assertRaises(MigrationError):
            parse_chat_ids(["-3", "-3"])

    def test_rejects_invalid_direct_chat_ids(self):
        for chat in (0, 7, -1_000_000_000_000, -2_000_000_000_000):
            with self.subTest(chat=chat), self.assertRaises(MigrationError):
                _read_input(None, [chat], None)

    def test_capacity_allows_an_exact_resume_without_double_counting(self):
        result = validate_limits(
            target_chat_count=10,
            target_settings_rows=100,
            target_settings_bytes=1000,
            moving_chat_count=5,
            moving_settings_rows=50,
            moving_settings_bytes=500,
            target_existing_moving_chats=5,
            target_existing_moving_settings_rows=50,
            target_existing_moving_settings_bytes=500,
            max_chats=10,
            max_settings_rows=100,
            max_settings_bytes=1000,
        )
        self.assertEqual(result["projected_chats"], 10)
        self.assertEqual(result["projected_settings_rows"], 100)
        self.assertEqual(result["projected_settings_bytes"], 1000)

    def test_capacity_rejects_overflow(self):
        with self.assertRaisesRegex(MigrationError, "MAX_SHARD_CHATS"):
            validate_limits(
                target_chat_count=10,
                target_settings_rows=100,
                target_settings_bytes=1000,
                moving_chat_count=1,
                moving_settings_rows=1,
                moving_settings_bytes=10,
                target_existing_moving_chats=0,
                target_existing_moving_settings_rows=0,
                target_existing_moving_settings_bytes=0,
                max_chats=10,
                max_settings_rows=101,
                max_settings_bytes=2000,
            )

    def test_capacity_rejects_setting_byte_overflow(self):
        with self.assertRaisesRegex(MigrationError, "MAX_SHARD_SETTINGS_BYTES"):
            validate_limits(
                target_chat_count=1,
                target_settings_rows=1,
                target_settings_bytes=100,
                moving_chat_count=1,
                moving_settings_rows=1,
                moving_settings_bytes=50,
                target_existing_moving_chats=0,
                target_existing_moving_settings_rows=0,
                target_existing_moving_settings_bytes=0,
                max_chats=10,
                max_settings_rows=10,
                max_settings_bytes=120,
            )

    def test_capacity_rejects_counter_row_overflow(self):
        with self.assertRaisesRegex(MigrationError, "MAX_SHARD_COUNTER_ROWS"):
            validate_limits(
                target_chat_count=1,
                target_settings_rows=1,
                target_settings_bytes=100,
                moving_chat_count=1,
                moving_settings_rows=1,
                moving_settings_bytes=50,
                target_existing_moving_chats=0,
                target_existing_moving_settings_rows=0,
                target_existing_moving_settings_bytes=0,
                target_counter_rows=90,
                moving_counter_rows=20,
                max_chats=10,
                max_settings_rows=10,
                max_settings_bytes=2000,
                max_counter_rows=100,
            )

    def test_capacity_rejects_tally_row_overflow_and_reports_projection(self):
        with self.assertRaisesRegex(MigrationError, "MAX_SHARD_TALLY_ROWS"):
            validate_limits(
                target_chat_count=1,
                target_settings_rows=1,
                target_settings_bytes=100,
                moving_chat_count=1,
                moving_settings_rows=1,
                moving_settings_bytes=50,
                target_existing_moving_chats=0,
                target_existing_moving_settings_rows=0,
                target_existing_moving_settings_bytes=0,
                target_tally_rows=90,
                moving_tally_rows=20,
                max_chats=10,
                max_settings_rows=10,
                max_settings_bytes=2_000,
                max_tally_rows=100,
            )
        capacity = validate_limits(
            target_chat_count=1,
            target_settings_rows=1,
            target_settings_bytes=100,
            moving_chat_count=1,
            moving_settings_rows=1,
            moving_settings_bytes=50,
            target_existing_moving_chats=0,
            target_existing_moving_settings_rows=0,
            target_existing_moving_settings_bytes=0,
            target_tally_rows=90,
            moving_tally_rows=20,
            target_existing_moving_tally_rows=10,
            max_chats=10,
            max_settings_rows=10,
            max_settings_bytes=2_000,
            max_tally_rows=100,
        )
        self.assertEqual(capacity["new_tally_rows"], 10)
        self.assertEqual(capacity["projected_tally_rows"], 100)

    def test_refresh_durable_counts_includes_tally_metadata(self):
        statements = []

        class Cursor:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def execute(self, statement):
                statements.append(statement)

        class Connection:
            @staticmethod
            def cursor():
                return Cursor()

        _refresh_durable_counts(Connection())
        self.assertEqual(len(statements), 1)
        self.assertIn("tally_rows = (SELECT count(*) FROM tallies)", statements[0])
        preflight_source = inspect.getsource(preflight)
        for binding in (
            "target_tally_rows=target_tally_rows",
            "moving_tally_rows=moving_tally_rows",
            "target_existing_moving_tally_rows=target_existing_tally_rows",
            "max_tally_rows=max_tally_rows",
        ):
            with self.subTest(binding=binding):
                self.assertIn(binding, preflight_source)

    def test_tally_limit_uses_rust_default_env_and_cli_bounds(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "sys.argv", ["shard_migrate.py"]
        ):
            self.assertEqual(parse_args().max_tally_rows, DEFAULT_MAX_TALLY_ROWS)
        with mock.patch.dict(os.environ, {"MAX_SHARD_TALLY_ROWS": "20000"}, clear=True), mock.patch(
            "sys.argv", ["shard_migrate.py"]
        ):
            self.assertEqual(parse_args().max_tally_rows, 20_000)
        with mock.patch.dict(os.environ, {"MAX_SHARD_TALLY_ROWS": "20000"}, clear=True), mock.patch(
            "sys.argv", ["shard_migrate.py", "--max-tally-rows", "30000"]
        ):
            self.assertEqual(parse_args().max_tally_rows, 30_000)
        for value in ("9999", "10000001", "not-a-number"):
            with self.subTest(value=value), mock.patch.dict(
                os.environ, {"MAX_SHARD_TALLY_ROWS": value}, clear=True
            ), mock.patch("sys.argv", ["shard_migrate.py"]), contextlib.redirect_stderr(
                io.StringIO()
            ), self.assertRaises(SystemExit):
                parse_args()

    def test_capacity_rejects_note_row_overflow(self):
        with self.assertRaisesRegex(MigrationError, "MAX_SHARD_NOTE_ROWS"):
            validate_limits(
                target_chat_count=1,
                target_settings_rows=1,
                target_settings_bytes=100,
                moving_chat_count=1,
                moving_settings_rows=1,
                moving_settings_bytes=50,
                target_existing_moving_chats=0,
                target_existing_moving_settings_rows=0,
                target_existing_moving_settings_bytes=0,
                target_note_rows=90,
                moving_note_rows=20,
                max_chats=10,
                max_settings_rows=10,
                max_settings_bytes=2000,
                max_note_rows=100,
            )


if __name__ == "__main__":
    unittest.main()
