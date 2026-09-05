import unittest

from fleet_routes import RouteSetError, validate_route_sets


class FleetRouteTests(unittest.TestCase):
    def test_complete_route_set_is_accepted(self):
        report = validate_route_sets(
            [-1, -2, -3, -4],
            {"alpha": [-1, -2], "bravo": [-3, -4]},
            {"alpha": 2, "bravo": 2},
            2,
        )
        self.assertEqual(report["groups"], 4)
        self.assertEqual(report["min_groups_per_shard"], 2)

    def test_duplicate_ownership_is_rejected(self):
        with self.assertRaisesRegex(RouteSetError, "both"):
            validate_route_sets(
                [-1, -2],
                {"alpha": [-1, -2], "bravo": [-2]},
                {"alpha": 2, "bravo": 1},
                2,
            )

    def test_missing_coverage_is_rejected(self):
        with self.assertRaisesRegex(RouteSetError, "coverage"):
            validate_route_sets(
                [-1, -2, -3],
                {"alpha": [-1], "bravo": [-2]},
                {"alpha": 1, "bravo": 1},
                2,
            )

    def test_declared_count_and_capacity_are_enforced(self):
        with self.assertRaisesRegex(RouteSetError, "manifest declares"):
            validate_route_sets(
                [-1, -2],
                {"alpha": [-1, -2]},
                {"alpha": 1},
                2,
            )

    def test_weighted_settings_capacity_is_enforced(self):
        weights = {
            -1: (3, 7),
            -2: (2, 4),
            -3: (3, 7),
            -4: (2, 4),
        }
        report = validate_route_sets(
            list(weights),
            {"alpha": [-1, -2], "bravo": [-3, -4]},
            {"alpha": 2, "bravo": 2},
            2,
            weights=weights,
            max_settings_rows=5,
            max_settings_bytes=11,
        )
        self.assertEqual(report["settings_rows_per_shard"], {"alpha": 5, "bravo": 5})
        self.assertEqual(report["settings_bytes_per_shard"], {"alpha": 11, "bravo": 11})
        with self.assertRaisesRegex(RouteSetError, "setting bytes"):
            validate_route_sets(
                list(weights),
                {"alpha": [-1, -2], "bravo": [-3, -4]},
                {"alpha": 2, "bravo": 2},
                2,
                weights=weights,
                max_settings_rows=5,
                max_settings_bytes=10,
            )

    def test_resident_counter_capacity_is_enforced(self):
        report = validate_route_sets(
            [-1, -2, -3, -4],
            {"alpha": [-1, -2], "bravo": [-3, -4]},
            {"alpha": 2, "bravo": 2},
            2,
            counter_rows_per_group=2,
            max_counter_rows=4,
        )
        self.assertEqual(report["counter_rows_per_shard"], {"alpha": 4, "bravo": 4})
        with self.assertRaisesRegex(RouteSetError, "counter rows"):
            validate_route_sets(
                [-1, -2, -3, -4],
                {"alpha": [-1, -2], "bravo": [-3, -4]},
                {"alpha": 2, "bravo": 2},
                2,
                counter_rows_per_group=2,
                max_counter_rows=3,
            )

    def test_uneven_counter_inventory_is_reported_and_checked(self):
        counter_weights = {-1: 3, -2: 1, -3: 3, -4: 1}
        report = validate_route_sets(
            list(counter_weights),
            {"alpha": [-1, -2], "bravo": [-3, -4]},
            {"alpha": 2, "bravo": 2},
            2,
            counter_weights=counter_weights,
            max_counter_rows=4,
        )
        self.assertEqual(report["counter_rows_per_shard"], {"alpha": 4, "bravo": 4})
        with self.assertRaisesRegex(RouteSetError, "counter weights coverage"):
            validate_route_sets(
                list(counter_weights),
                {"alpha": [-1, -2], "bravo": [-3, -4]},
                {"alpha": 2, "bravo": 2},
                2,
                counter_weights={-1: 3},
                max_counter_rows=4,
            )

    def test_resident_note_capacity_is_enforced(self):
        report = validate_route_sets(
            [-1, -2, -3, -4],
            {"alpha": [-1, -2], "bravo": [-3, -4]},
            {"alpha": 2, "bravo": 2},
            2,
            note_rows_per_group=2,
            max_note_rows=4,
        )
        self.assertEqual(report["note_rows_per_shard"], {"alpha": 4, "bravo": 4})
        with self.assertRaisesRegex(RouteSetError, "note rows"):
            validate_route_sets(
                [-1, -2, -3, -4],
                {"alpha": [-1, -2], "bravo": [-3, -4]},
                {"alpha": 2, "bravo": 2},
                2,
                note_rows_per_group=2,
                max_note_rows=3,
            )


if __name__ == "__main__":
    unittest.main()
