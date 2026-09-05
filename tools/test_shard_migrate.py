import unittest

from shard_migrate import MigrationError, _read_input, parse_chat_ids, validate_limits


class ShardMigrateTests(unittest.TestCase):
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
        with self.assertRaises(MigrationError):
            _read_input(None, [0], None)

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
