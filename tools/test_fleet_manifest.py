import copy
import json
import unittest
from pathlib import Path

from fleet_manifest import ManifestError, validate_manifest


ROOT = Path(__file__).with_name("fleet.example.json")


class FleetManifestTests(unittest.TestCase):
    def setUp(self):
        self.document = json.loads(ROOT.read_text(encoding="utf-8"))

    def test_reference_manifest_is_valid(self):
        result = validate_manifest(self.document)
        self.assertEqual(result["declared_shards"], 10)
        self.assertEqual(result["assigned_groups"], 500_000)
        self.assertEqual(result["db_connections_with_reserve"], 100)
        # 10 shards at the measured 2600 MB each — 1.62 GiB worst-case model pools
        # (--model-probe, patch16-224 set) plus runtime state observed at 2.4 GiB live.
        self.assertEqual(result["memory_used_mb"], 26_000)
        self.assertEqual(result["hosts"], {"host-a": 4, "host-b": 3, "host-c": 3})

    def test_shared_token_is_rejected(self):
        document = copy.deepcopy(self.document)
        document["shards"][1]["token_ref"] = document["shards"][0]["token_ref"]
        with self.assertRaisesRegex(ManifestError, "duplicate token reference"):
            validate_manifest(document)

    def test_underprovisioned_fleet_is_rejected(self):
        document = copy.deepcopy(self.document)
        document["shards"] = document["shards"][:9]
        document["fleet"]["groups"] = 450_000
        document["fleet"]["actions_per_second"] = 251
        with self.assertRaisesRegex(ManifestError, "required"):
            validate_manifest(document)

    def test_shared_database_is_rejected(self):
        document = copy.deepcopy(self.document)
        document["shards"][1]["database_ref"] = document["shards"][0]["database_ref"]
        with self.assertRaisesRegex(ManifestError, "duplicate database reference"):
            validate_manifest(document)

    def test_host_overcommit_is_rejected(self):
        document = copy.deepcopy(self.document)
        for shard in document["shards"]:
            shard["host"] = "one-host"
        document["fleet"]["hosts_available"] = 1
        # Memory would also overcommit at ten shards on one host; give the host enough of it
        # so the check this test pins — the CPU core budget — is the one that fires.
        document["fleet"]["host_memory_mb"] = 30_000
        with self.assertRaisesRegex(ManifestError, "needs 20.00 CPU cores"):
            validate_manifest(document)

    def test_uneven_settings_rows_are_rejected_per_shard(self):
        document = copy.deepcopy(self.document)
        document["fleet"]["groups_per_shard"] = 100_000
        document["shards"][0]["groups"] = 100_000
        for shard in document["shards"][1:9]:
            shard["groups"] = 44_444
        document["shards"][9]["groups"] = 44_448
        with self.assertRaisesRegex(ManifestError, "estimates 10000000 settings rows"):
            validate_manifest(document)

    def test_oversized_setting_bytes_are_rejected_per_shard(self):
        document = copy.deepcopy(self.document)
        document["fleet"]["settings_bytes_per_group"] = 11_000
        with self.assertRaisesRegex(ManifestError, "estimates 550000000 setting bytes"):
            validate_manifest(document)

    def test_oversized_counter_rows_are_rejected_per_shard(self):
        document = copy.deepcopy(self.document)
        document["fleet"]["counter_rows_per_group"] = 101
        with self.assertRaisesRegex(ManifestError, "estimates 5050000 counter rows"):
            validate_manifest(document)

    def test_oversized_note_rows_are_rejected_per_shard(self):
        document = copy.deepcopy(self.document)
        document["fleet"]["note_rows_per_group"] = 101
        with self.assertRaisesRegex(ManifestError, "estimates 5050000 note rows"):
            validate_manifest(document)


if __name__ == "__main__":
    unittest.main()
