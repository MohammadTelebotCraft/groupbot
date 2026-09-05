import copy
import json
import tempfile
import unittest
from pathlib import Path

from fleet_prepare import PrepareError, prepare


ROOT = Path(__file__).with_name("fleet.example.json")


class FleetPrepareTests(unittest.TestCase):
    def small_manifest(self):
        document = json.loads(ROOT.read_text(encoding="utf-8"))
        document["fleet"]["groups"] = 4
        document["fleet"]["groups_per_shard"] = 2
        document["fleet"]["actions_per_second"] = 0
        document["fleet"]["counter_rows_per_second"] = 0
        document["shards"] = copy.deepcopy(document["shards"][:2])
        for shard in document["shards"]:
            shard["groups"] = 2
        return document

    def test_prepares_routes_and_secret_free_templates(self):
        with tempfile.TemporaryDirectory() as directory:
            report = prepare(
                self.small_manifest(),
                [-1, -2, -3, -4],
                Path(directory),
            )
            self.assertEqual(report["prepared_shards"], 2)
            for shard in ("alpha", "bravo"):
                route = Path(directory, shard, "chat_ids.txt")
                env = Path(directory, shard, ".env.example")
                self.assertEqual(len(route.read_text().splitlines()), 2)
                self.assertIn("TG_BOT_TOKEN=", env.read_text())
                self.assertNotIn("XXXXXXXXXXXXXXXX", env.read_text())
            self.assertTrue(Path(directory, "fleet_prepare.json").exists())

    def test_host_filter_only_writes_that_host(self):
        document = self.small_manifest()
        document["shards"][0]["host"] = "host-a"
        document["shards"][1]["host"] = "host-b"
        with tempfile.TemporaryDirectory() as directory:
            report = prepare(document, [-1, -2, -3, -4], Path(directory), host="host-b")
            self.assertEqual(report["prepared_shards"], 1)
            self.assertFalse(Path(directory, "alpha").exists())
            self.assertTrue(Path(directory, "bravo", "chat_ids.txt").exists())

    def test_existing_generated_file_requires_force(self):
        with tempfile.TemporaryDirectory() as directory:
            prepare(self.small_manifest(), [-1, -2, -3, -4], Path(directory))
            with self.assertRaisesRegex(PrepareError, "output already exists"):
                prepare(self.small_manifest(), [-1, -2, -3, -4], Path(directory))


if __name__ == "__main__":
    unittest.main()
