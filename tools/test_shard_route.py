import json
import unittest
from pathlib import Path

from shard_route import assign, owner


class ShardRouteTests(unittest.TestCase):
    shards = tuple(
        json.loads(Path(__file__).with_name("fleet.example.json").read_text())["shards"][i]["name"]
        for i in range(10)
    )

    def test_owner_is_stable_when_input_order_changes(self):
        for chat_id in (-1001, -1002, -1003, 7, 42):
            self.assertEqual(
                owner(chat_id, self.shards, "groupbot-v1"),
                owner(chat_id, reversed(self.shards), "groupbot-v1"),
            )

    def test_owner_is_one_of_the_declared_shards(self):
        assigned = {owner(-100000 - number, self.shards, "groupbot-v1") for number in range(10_000)}
        self.assertEqual(assigned, set(self.shards))

    def test_draining_one_shard_only_moves_its_owners(self):
        reduced = self.shards[:-1]
        for number in range(2_000):
            chat_id = -200000 - number
            before = owner(chat_id, self.shards, "groupbot-v1")
            after = owner(chat_id, reduced, "groupbot-v1")
            self.assertEqual(before == self.shards[-1], before != after)

    def test_real_group_plan_never_overflows_a_shard(self):
        chat_ids = range(-1_000_000, -999_000)
        planned = assign(chat_ids, self.shards, "groupbot-v1", capacity=100)
        counts = {shard: 0 for shard in self.shards}
        for shard in planned.values():
            counts[shard] += 1
        self.assertEqual(len(planned), 1_000)
        self.assertEqual(set(counts.values()), {100})

    def test_capacity_assignment_can_be_filtered_to_one_shard(self):
        planned = assign(range(-1_000, -900), self.shards, "groupbot-v1", capacity=10)
        self.assertEqual(sum(shard == self.shards[0] for shard in planned.values()), 10)

    def test_weighted_assignment_respects_row_and_byte_caps(self):
        chat_ids = [1, 2, 3, 4]
        rows = {1: 3, 2: 3, 3: 2, 4: 2}
        bytes_ = {1: 7, 2: 7, 3: 4, 4: 4}
        planned = assign(
            chat_ids,
            ("alpha", "bravo"),
            "groupbot-v1",
            capacity=4,
            row_weights=rows,
            byte_weights=bytes_,
            max_rows=5,
            max_bytes=11,
        )

        self.assertEqual(set(planned), set(chat_ids))
        for shard in ("alpha", "bravo"):
            owned = [chat for chat, target in planned.items() if target == shard]
            self.assertLessEqual(sum(rows[chat] for chat in owned), 5)
            self.assertLessEqual(sum(bytes_[chat] for chat in owned), 11)

    def test_assignment_respects_resident_counter_rows(self):
        planned = assign(
            [1, 2, 3, 4],
            ("alpha", "bravo"),
            "groupbot-v1",
            capacity=4,
            counter_rows_per_group=2,
            max_counter_rows=4,
        )
        self.assertEqual(set(planned), {1, 2, 3, 4})
        for shard in ("alpha", "bravo"):
            self.assertLessEqual(
                sum(target == shard for target in planned.values()) * 2,
                4,
            )

    def test_assignment_respects_resident_note_rows(self):
        planned = assign(
            [1, 2, 3, 4],
            ("alpha", "bravo"),
            "groupbot-v1",
            capacity=4,
            note_rows_per_group=2,
            max_note_rows=4,
        )
        self.assertEqual(set(planned), {1, 2, 3, 4})
        for shard in ("alpha", "bravo"):
            self.assertLessEqual(
                sum(target == shard for target in planned.values()) * 2,
                4,
            )

    def test_assignment_can_balance_uneven_counter_inventories(self):
        weights = {1: 3, 2: 1, 3: 3, 4: 1}
        planned = assign(
            weights,
            ("alpha", "bravo"),
            "groupbot-v1",
            capacity=4,
            max_counter_rows=4,
            counter_row_weights=weights,
        )
        self.assertEqual(set(planned), set(weights))
        for shard in ("alpha", "bravo"):
            self.assertLessEqual(
                sum(weights[chat] for chat, target in planned.items() if target == shard),
                4,
            )


if __name__ == "__main__":
    unittest.main()
