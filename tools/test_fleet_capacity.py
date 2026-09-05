import unittest

from fleet_capacity import plan


class FleetCapacityTests(unittest.TestCase):
    def test_groups_drive_ten_shards_for_500k(self):
        result = plan(groups=500_000, groups_per_shard=50_000)
        self.assertEqual(result.shards_for_groups, 10)
        self.assertEqual(result.shards_required, 10)
        self.assertEqual(result.db_connections_used, 80)
        self.assertEqual(result.db_connections_with_reserve, 100)
        self.assertTrue(result.fits_database)

    def test_action_rate_can_require_more_shards_than_groups(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            actions_per_second=301,
            telegram_actions_per_second=30,
            db_pool=4,
            db_max_connections=100,
        )
        self.assertEqual(result.shards_for_actions, 11)
        self.assertEqual(result.shards_required, 11)

    def test_incoming_update_rate_is_a_separate_shard_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            updates_per_second=5_001,
            updates_per_shard_per_second=500,
            db_pool=4,
        )
        self.assertEqual(result.shards_for_updates, 11)
        self.assertEqual(result.shards_required, 11)

    def test_counter_write_rate_is_a_separate_shard_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            counter_rows_per_second=5_001,
            counter_rows_per_shard_per_second=500,
            db_pool=4,
        )
        self.assertEqual(result.shards_for_counters, 11)
        self.assertEqual(result.shards_required, 11)

    def test_counter_storage_rows_are_a_separate_shard_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            counter_rows_per_group=101,
            max_counter_rows_per_shard=5_000_000,
        )
        self.assertEqual(result.counter_rows_total, 50_500_000)
        self.assertEqual(result.shards_for_counter_storage, 11)
        self.assertEqual(result.shards_required, 11)

    def test_note_storage_rows_are_a_separate_shard_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            note_rows_per_group=101,
            max_note_rows_per_shard=5_000_000,
        )
        self.assertEqual(result.note_rows_total, 50_500_000)
        self.assertEqual(result.shards_for_note_storage, 11)
        self.assertEqual(result.shards_required, 11)

    def test_settings_rows_are_a_separate_shard_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            settings_rows_per_group=101,
            max_settings_rows_per_shard=5_000_000,
        )
        self.assertEqual(result.settings_rows_total, 50_500_000)
        self.assertEqual(result.shards_for_settings, 11)
        self.assertEqual(result.shards_required, 11)

    def test_setting_bytes_are_a_separate_shard_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            settings_bytes_per_group=11_000,
            max_settings_bytes_per_shard=512 * 1024 * 1024,
        )
        self.assertEqual(result.settings_bytes_total, 5_500_000_000)
        self.assertEqual(result.shards_for_settings_bytes, 11)
        self.assertEqual(result.shards_required, 11)

    def test_full_pool_is_rejected_when_reserve_would_be_breached(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            db_pool=10,
            db_max_connections=100,
        )
        self.assertEqual(result.db_connections_used, 100)
        self.assertEqual(result.db_connections_with_reserve, 125)
        self.assertFalse(result.fits_database)

    def test_memory_is_a_separate_ceiling(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            db_pool=8,
            memory_per_shard_mb=768,
            host_memory_mb=7_679,
        )
        self.assertEqual(result.memory_used_mb, 7_680)
        self.assertFalse(result.fits_memory)

    def test_cpu_placement_can_require_a_second_host(self):
        result = plan(
            groups=500_000,
            groups_per_shard=50_000,
            host_cpu_cores=8,
            cpu_cores_per_shard=1,
            hosts_available=1,
        )
        self.assertEqual(result.hosts_for_cpu, 2)
        self.assertEqual(result.hosts_required, 2)
        self.assertFalse(result.fits_cpu)
        self.assertFalse(result.fits_fleet)


if __name__ == "__main__":
    unittest.main()
