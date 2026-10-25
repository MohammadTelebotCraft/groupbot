from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

from tools import media_eval
from tools import media_benchmark


def row(
    sample_id: str,
    label: str | None,
    split: str,
    *,
    digest: str,
    hard_negative: bool = False,
    group_id: str | None = None,
) -> dict:
    value = {
        "schema_version": 1,
        "id": sample_id,
        "media_path": f"media/{sample_id}.jpg",
        "media_type": "image",
        "split": split,
        "label": label,
        "label_status": "adjudicated" if label is not None else "unlabeled",
        "hard_negative": hard_negative,
        "tags": ["test"],
        "content_sha256": digest,
    }
    if label is not None:
        value["label_source"] = "two-reviewer-test-consensus"
    if group_id:
        value["group_id"] = group_id
    return value


class ManifestTests(unittest.TestCase):
    def test_rejects_cross_split_hash_and_group_leakage(self):
        rows = [
            row("a", "SAFE", "development", digest="a" * 64, group_id="near-duplicate"),
            row("b", "SAFE", "holdout", digest="a" * 64, group_id="near-duplicate"),
        ]
        errors = media_eval.validate_manifest(rows)
        self.assertTrue(any("content_sha256" in error and "leaks" in error for error in errors))
        self.assertTrue(any("group_id" in error and "leaks" in error for error in errors))

    def test_requires_label_provenance_and_consistent_unlabelled_state(self):
        labelled = row("a", "AD", "development", digest="a" * 64)
        del labelled["label_source"]
        unlabelled = row("b", None, "unassigned", digest="b" * 64)
        unlabelled["label_status"] = "reviewed"
        errors = media_eval.validate_manifest([labelled, unlabelled])
        self.assertTrue(any("label_source" in error for error in errors))
        self.assertTrue(any("null label" in error for error in errors))

    def test_hard_negative_cannot_be_action_label(self):
        errors = media_eval.validate_manifest(
            [row("a", "AD", "development", digest="a" * 64, hard_negative=True)],
            action_labels={"AD"},
        )
        self.assertTrue(any("hard_negative" in error for error in errors))

    def test_split_is_deterministic_and_keeps_groups_together(self):
        rows = [
            row("a", "SAFE", "unassigned", digest="a" * 64, group_id="same"),
            row("b", "SAFE", "unassigned", digest="b" * 64, group_id="same"),
            row("c", "AD", "unassigned", digest="c" * 64),
        ]
        first = media_eval.assign_splits(
            rows, development=0.7, calibration=0.15, holdout=0.15, salt="v1"
        )
        second = media_eval.assign_splits(
            rows, development=0.7, calibration=0.15, holdout=0.15, salt="v1"
        )
        self.assertEqual(first, second)
        self.assertEqual(first[0]["split"], first[1]["split"])
        self.assertTrue(all(item["split"] != "unassigned" for item in first))

    def test_holdout_lock_detects_relabel(self):
        rows = [row("a", "SAFE", "holdout", digest="a" * 64)]
        lock = media_eval.holdout_lock(rows)
        self.assertEqual(media_eval.check_holdout_lock(rows, lock), [])
        changed = [dict(rows[0], label="AD")]
        self.assertTrue(media_eval.check_holdout_lock(changed, lock))

    def test_holdout_lock_binds_evaluation_metadata(self):
        rows = [row("a", "SAFE", "holdout", digest="a" * 64, group_id="family")]
        rows[0]["taxonomy_version"] = "policy-v1"
        lock = media_eval.holdout_lock(rows)
        for field, value in [
            ("media_path", "different.jpg"),
            ("group_id", "different-family"),
            ("hard_negative", True),
            ("tags", ["different-tag"]),
            ("taxonomy_version", "policy-v2"),
        ]:
            changed = [dict(rows[0], **{field: value})]
            self.assertTrue(media_eval.check_holdout_lock(changed, lock), field)

    def test_require_hashes_recomputes_media_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            media = root / "sample.jpg"
            media.write_bytes(b"first")
            digest = __import__("hashlib").sha256(media.read_bytes()).hexdigest()
            item = row("a", "SAFE", "development", digest=digest)
            item["media_path"] = media.name
            self.assertEqual(
                media_eval.validate_manifest(
                    [item], manifest_path=root / "manifest.jsonl", require_hashes=True
                ),
                [],
            )
            media.write_bytes(b"changed")
            errors = media_eval.validate_manifest(
                [item], manifest_path=root / "manifest.jsonl", require_hashes=True
            )
            self.assertTrue(any("does not match media bytes" in error for error in errors))


class MetricTests(unittest.TestCase):
    def setUp(self):
        self.manifest = [
            row("tp", "AD", "calibration", digest="1" * 64),
            row("fn", "AD", "calibration", digest="2" * 64),
            row("tn", "SAFE", "calibration", digest="3" * 64),
            row("fp", "SAFE", "calibration", digest="4" * 64, hard_negative=True),
        ]
        self.predictions = [
            {"id": "tp", "predicted_label": "AD", "decision": "ACTION", "score": 0.9, "latency_ms": 10},
            {"id": "fn", "predicted_label": "SAFE", "decision": "ALLOW", "score": 0.4, "latency_ms": 20},
            {"id": "tn", "predicted_label": "SAFE", "decision": "ALLOW", "score": 0.1, "latency_ms": 30},
            {"id": "fp", "predicted_label": "AD", "decision": "ACTION", "score": 0.8, "latency_ms": 40},
        ]

    def test_reports_required_quality_and_latency_metrics(self):
        report = media_eval.evaluate(
            self.manifest,
            self.predictions,
            split="calibration",
            action_labels={"AD"},
            wall_seconds=2.0,
        )
        action = report["action"]
        self.assertEqual((action["tp"], action["fp"], action["tn"], action["fn"]), (1, 1, 1, 1))
        self.assertEqual(action["precision"], 0.5)
        self.assertEqual(action["recall"], 0.5)
        self.assertEqual(action["false_positive_rate"], 0.5)
        self.assertEqual(action["false_negative_rate"], 0.5)
        self.assertEqual(action["f1"], 0.5)
        self.assertEqual(report["hard_negative_action"]["false_positive_rate"], 1.0)
        self.assertEqual(report["false_positive_ids"], ["fp"])
        self.assertEqual(report["performance"]["latency_ms"]["p50"], 25.0)
        self.assertEqual(report["performance"]["latency_ms"]["p95"], 38.5)
        self.assertEqual(report["performance"]["media_per_second"], 2.0)
        self.assertEqual(report["classification"]["confusion_matrix"]["SAFE"]["AD"], 1)

    def test_missing_predictions_are_failures_not_dropped(self):
        report = media_eval.evaluate(
            self.manifest, self.predictions[:3], split="calibration", action_labels={"AD"}
        )
        self.assertEqual(report["classification"]["samples"], 4)
        self.assertEqual(report["coverage"]["received"], 3)
        self.assertEqual(report["classification"]["confusion_matrix"]["SAFE"][media_eval.PREDICTION_ERROR], 1)
        self.assertEqual(report["performance"]["latency_coverage"]["expected"], 4)
        self.assertEqual(report["performance"]["latency_coverage"]["measured"], 3)
        self.assertEqual(report["performance"]["latency_coverage"]["missing_ids"], ["fp"])

    def test_threshold_curve_uses_scores(self):
        curve = media_eval.threshold_curve(
            self.manifest,
            self.predictions,
            split="calibration",
            action_labels={"AD"},
            thresholds=[0.5, 0.85],
        )
        self.assertEqual(curve[0]["tp"], 1)
        self.assertEqual(curve[0]["fp"], 1)
        self.assertEqual(curve[1]["tp"], 1)
        self.assertEqual(curve[1]["fp"], 0)

    def test_threshold_curve_refuses_incomplete_score_coverage(self):
        without_score = [dict(item) for item in self.predictions]
        del without_score[-1]["score"]
        with self.assertRaisesRegex(media_eval.ValidationError, "every selected ID"):
            media_eval.threshold_curve(
                self.manifest,
                without_score,
                split="calibration",
                action_labels={"AD"},
                thresholds=[0.5],
            )
        with self.assertRaisesRegex(media_eval.ValidationError, "every selected ID"):
            media_eval.threshold_curve(
                self.manifest,
                self.predictions[:-1],
                split="calibration",
                action_labels={"AD"},
                thresholds=[0.5],
            )

    def test_error_prediction_is_not_received_or_classified_as_its_label(self):
        predictions = [dict(item) for item in self.predictions]
        predictions[2] = {
            "id": "tn",
            "predicted_label": None,
            "decision": "ERROR",
            "error": "sanitized failure",
        }
        report = media_eval.evaluate(
            self.manifest,
            predictions,
            split="calibration",
            action_labels={"AD"},
        )
        self.assertEqual(report["coverage"]["received"], 3)
        self.assertIn("tn", report["coverage"]["error_ids"])
        self.assertEqual(
            report["classification"]["confusion_matrix"]["SAFE"][media_eval.PREDICTION_ERROR],
            1,
        )

    def test_error_or_timeout_cannot_be_actionable(self):
        for failed in [
            {"id": "a", "predicted_label": "AD", "decision": "ACTION", "error": "failed"},
            {"id": "a", "predicted_label": "AD", "decision": "ACTION", "timed_out": True},
        ]:
            errors = media_eval.validate_predictions([failed], {"AD"})
            self.assertTrue(any("cannot be actionable" in error for error in errors))

    def test_prediction_contract_rejects_unknown_raw_fields(self):
        errors = media_eval.validate_predictions(
            [{"id": "a", "predicted_label": "SAFE", "ocr_text": "private user text"}],
            {"AD"},
        )
        self.assertTrue(any("unsupported fields" in error for error in errors))

    def test_rates_without_denominator_are_null(self):
        result = media_eval.binary_metrics([True], [False])
        self.assertIsNone(result["precision"])
        self.assertIsNone(result["false_positive_rate"])

    def test_evaluation_refuses_missing_labels(self):
        manifest = [row("unknown", None, "calibration", digest="a" * 64)]
        with self.assertRaisesRegex(media_eval.ValidationError, "missing labels"):
            media_eval.evaluate(
                manifest,
                [{"id": "unknown", "predicted_label": "SAFE"}],
                split="calibration",
                action_labels={"AD"},
            )

    def test_holdout_requires_adjudicated_labels(self):
        manifest = [row("provisional", "SAFE", "holdout", digest="a" * 64)]
        manifest[0]["label_status"] = "provisional"
        with self.assertRaisesRegex(media_eval.ValidationError, "requires adjudicated labels"):
            media_eval.evaluate(
                manifest,
                [{"id": "provisional", "predicted_label": "SAFE", "latency_ms": 1}],
                split="holdout",
                action_labels={"AD"},
            )

    def test_named_pipeline_comparison_is_paired(self):
        left = self.predictions
        right = [dict(item) for item in self.predictions]
        right[-1] = dict(right[-1], predicted_label="SAFE", decision="ALLOW")
        report = media_eval.compare_pipelines(
            self.manifest,
            {"baseline": left, "candidate": right},
            split="calibration",
            action_labels={"AD"},
            threshold_sweep_enabled=True,
        )
        self.assertIn("manifest_sha256", report)
        self.assertEqual(report["paired_disagreements"][0]["label_disagreement_count"], 1)
        self.assertEqual(report["paired_disagreements"][0]["action_disagreement_count"], 1)
        self.assertEqual(report["paired_disagreements"][0]["left_only_action_count"], 1)
        self.assertIn("threshold_curve", report["pipelines"]["candidate"])
        markdown = media_eval.comparison_markdown(report)
        self.assertIn("| baseline |", markdown)
        self.assertIn("Paired disagreements", markdown)
        self.assertIn("Prediction coverage", markdown)
        self.assertIn("Latency coverage", markdown)
        self.assertIn("Timeout", markdown)
        self.assertIn("Uncertain", markdown)
        self.assertIn("Cache hit", markdown)


class CommandTests(unittest.TestCase):
    def test_cli_blocks_holdout_threshold_tuning(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            media_file = root / "holdout.jpg"
            media_file.write_bytes(b"holdout")
            manifest = root / "manifest.jsonl"
            predictions = root / "predictions.jsonl"
            item = row(
                "a",
                "AD",
                "holdout",
                digest=__import__("hashlib").sha256(media_file.read_bytes()).hexdigest(),
            )
            item["media_path"] = media_file.name
            media_eval.write_jsonl(manifest, [item])
            media_eval.write_jsonl(predictions, [{"id": "a", "predicted_label": "AD", "score": 0.9}])
            result = subprocess.run(
                [
                    sys.executable,
                    str(pathlib.Path(media_eval.__file__)),
                    "evaluate",
                    str(manifest),
                    str(predictions),
                    "--split",
                    "holdout",
                    "--action-label",
                    "AD",
                    "--threshold-sweep",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("only on the calibration split", result.stderr)

    def test_real_command_benchmark_emits_predictions_and_resources(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            media_file = root / "input.bin"
            media_file.write_bytes(b"real bytes")
            manifest = root / "manifest.jsonl"
            sample = row("one", "SAFE", "calibration", digest="0" * 64)
            sample["media_path"] = media_file.name
            sample["content_sha256"] = __import__("hashlib").sha256(media_file.read_bytes()).hexdigest()
            media_eval.write_jsonl(manifest, [sample])
            adapter = root / "adapter.py"
            adapter.write_text(
                "import json,sys\n"
                "print(json.dumps({'predicted_label':'SAFE','decision':'ALLOW','stage_ms':{'decode':1.0}}))\n",
                encoding="utf-8",
            )
            output = root / "results"
            command = json.dumps([sys.executable, str(adapter), "{media_path}"])
            benchmark = pathlib.Path(media_eval.__file__).with_name("media_benchmark.py")
            result = subprocess.run(
                [
                    sys.executable,
                    str(benchmark),
                    str(manifest),
                    "--split",
                    "calibration",
                    "--command-json",
                    command,
                    "--output-dir",
                    str(output),
                    "--concurrency",
                    "1",
                    "--warmup",
                    "0",
                ],
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads((output / "benchmark.json").read_text(encoding="utf-8"))
            self.assertEqual(report["levels"][0]["samples"], 1)
            self.assertGreater(report["levels"][0]["media_per_second"], 0)
            self.assertEqual(report["cache"]["policy"], "cold")
            self.assertTrue(report["cache"]["independent_order_per_level"])
            predictions = media_eval.read_jsonl(output / "predictions-c1.jsonl")
            self.assertEqual(predictions[0]["predicted_label"], "SAFE")
            self.assertTrue((output / "resources-c1.jsonl").is_file())

    def test_compare_cli_emits_json_and_markdown(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            media_dir = root / "media"
            media_dir.mkdir()
            (media_dir / "a.jpg").write_bytes(b"a")
            (media_dir / "b.jpg").write_bytes(b"b")
            manifest = root / "manifest.jsonl"
            left = root / "left.jsonl"
            right = root / "right.jsonl"
            media_eval.write_jsonl(
                manifest,
                [
                    row(
                        "a",
                        "AD",
                        "calibration",
                        digest=__import__("hashlib").sha256((media_dir / "a.jpg").read_bytes()).hexdigest(),
                    ),
                    row(
                        "b",
                        "SAFE",
                        "calibration",
                        digest=__import__("hashlib").sha256((media_dir / "b.jpg").read_bytes()).hexdigest(),
                        hard_negative=True,
                    ),
                ],
            )
            media_eval.write_jsonl(
                left,
                [
                    {"id": "a", "predicted_label": "AD", "decision": "ACTION", "score": 0.9},
                    {"id": "b", "predicted_label": "AD", "decision": "ACTION", "score": 0.8},
                ],
            )
            media_eval.write_jsonl(
                right,
                [
                    {"id": "a", "predicted_label": "AD", "decision": "ACTION", "score": 0.9},
                    {"id": "b", "predicted_label": "SAFE", "decision": "ALLOW", "score": 0.1},
                ],
            )
            output_json = root / "comparison.json"
            output_markdown = root / "comparison.md"
            result = subprocess.run(
                [
                    sys.executable,
                    str(pathlib.Path(media_eval.__file__)),
                    "compare",
                    str(manifest),
                    "--split",
                    "calibration",
                    "--action-label",
                    "AD",
                    "--pipeline",
                    f"baseline={left}",
                    "--pipeline",
                    f"candidate={right}",
                    "--threshold-sweep",
                    "--output-json",
                    str(output_json),
                    "--output-markdown",
                    str(output_markdown),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            comparison = json.loads(output_json.read_text(encoding="utf-8"))
            self.assertEqual(comparison["paired_disagreements"][0]["action_disagreement_count"], 1)
            self.assertIn("candidate", output_markdown.read_text(encoding="utf-8"))

    def test_adapter_output_allowlist_rejects_private_text(self):
        with self.assertRaisesRegex(ValueError, "unsupported adapter output fields"):
            media_benchmark.validate_adapter_payload(
                {"predicted_label": "SAFE", "decision": "ALLOW", "evidence": "raw user text"}
            )
        with self.assertRaisesRegex(ValueError, "outside the manifest taxonomy"):
            media_benchmark.validate_adapter_payload(
                {"predicted_label": "raw OCR user text", "decision": "ALLOW"},
                {"SAFE", "AD"},
            )

    def test_each_concurrency_level_can_use_an_independent_order(self):
        rows = [{"id": str(index)} for index in range(20)]
        first = media_benchmark.independently_ordered(rows, 1)
        second = media_benchmark.independently_ordered(rows, 2)
        self.assertNotEqual(
            [item["id"] for item in first],
            [item["id"] for item in second],
        )


if __name__ == "__main__":
    unittest.main()
