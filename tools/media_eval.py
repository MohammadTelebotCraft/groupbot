#!/usr/bin/env python3
"""Validate and score human-labelled media moderation evaluation manifests.

This module is deliberately model-agnostic.  It never infers a ground-truth label:
labels and their provenance must already be present in the manifest.  It supports
group-stable dataset splitting, holdout locks, multiclass and action-level metrics,
latency/resource summaries, and calibration-set threshold curves.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import math
import pathlib
import re
import statistics
import sys
from dataclasses import dataclass
from typing import Any, Iterable, Sequence


SCHEMA_VERSION = 1
SPLITS = ("development", "calibration", "holdout", "unassigned")
MEDIA_TYPES = ("image", "video", "gif", "sticker", "thumbnail", "document", "other")
LABEL_STATUSES = ("adjudicated", "reviewed", "provisional", "unlabeled")
PREDICTION_ERROR = "__ERROR__"
HEX_256 = re.compile(r"^[0-9a-f]{64}$")
STAGE_NAME = re.compile(r"^[A-Za-z0-9_.-]{1,64}$")
VERSION_NAME = re.compile(r"^[A-Za-z0-9_.:/+@-]{1,128}$")
DECISIONS = {"ALLOW", "ACTION", "UNCERTAIN", "ERROR"}
PREDICTION_FIELDS = {
    "id",
    "predicted_label",
    "decision",
    "score",
    "confidence",
    "latency_ms",
    "queue_wait_ms",
    "started_at_ms",
    "completed_at_ms",
    "timed_out",
    "error",
    "stage_ms",
    "cache_hit",
    "frames_processed",
    "gpu_time_ms",
    "concurrency",
    "pipeline_version",
    "model_version",
}


class ValidationError(ValueError):
    """A manifest or prediction contract violation."""


def read_jsonl(path: pathlib.Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_number, raw in enumerate(handle, 1):
            if not raw.strip():
                continue
            try:
                value = json.loads(raw)
            except json.JSONDecodeError as error:
                raise ValidationError(f"{path}:{line_number}: invalid JSON: {error.msg}") from error
            if not isinstance(value, dict):
                raise ValidationError(f"{path}:{line_number}: each row must be an object")
            value["__line__"] = line_number
            rows.append(value)
    return rows


def write_jsonl(path: pathlib.Path, rows: Iterable[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        for row in rows:
            clean = {key: value for key, value in row.items() if not key.startswith("__")}
            handle.write(json.dumps(clean, ensure_ascii=False, sort_keys=True) + "\n")


def _nonempty_string(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def validate_manifest(
    rows: Sequence[dict[str, Any]],
    *,
    manifest_path: pathlib.Path | None = None,
    require_files: bool = False,
    require_hashes: bool = False,
    action_labels: set[str] | None = None,
) -> list[str]:
    errors: list[str] = []
    seen_ids: dict[str, int] = {}
    hash_splits: dict[str, set[str]] = collections.defaultdict(set)
    group_splits: dict[str, set[str]] = collections.defaultdict(set)
    verified_files: dict[pathlib.Path, str] = {}
    root = manifest_path.parent if manifest_path else pathlib.Path.cwd()

    for at, row in enumerate(rows, 1):
        line = row.get("__line__", at)
        where = f"line {line}"
        if row.get("schema_version") != SCHEMA_VERSION:
            errors.append(f"{where}: schema_version must be {SCHEMA_VERSION}")
        sample_id = row.get("id")
        if not _nonempty_string(sample_id):
            errors.append(f"{where}: id must be a non-empty string")
        elif sample_id in seen_ids:
            errors.append(f"{where}: duplicate id {sample_id!r} (first on line {seen_ids[sample_id]})")
        else:
            seen_ids[sample_id] = int(line)

        media_path = row.get("media_path")
        if not _nonempty_string(media_path):
            errors.append(f"{where}: media_path must be a non-empty string")
        elif require_files:
            candidate = pathlib.Path(media_path)
            if not candidate.is_absolute():
                candidate = root / candidate
            if not candidate.is_file():
                errors.append(f"{where}: media file does not exist: {candidate}")

        if row.get("media_type") not in MEDIA_TYPES:
            errors.append(f"{where}: media_type must be one of {', '.join(MEDIA_TYPES)}")
        split = row.get("split")
        if split not in SPLITS:
            errors.append(f"{where}: split must be one of {', '.join(SPLITS)}")

        label = row.get("label")
        status = row.get("label_status")
        if status not in LABEL_STATUSES:
            errors.append(f"{where}: label_status must be one of {', '.join(LABEL_STATUSES)}")
        if label is None:
            if status != "unlabeled":
                errors.append(f"{where}: a null label requires label_status='unlabeled'")
        else:
            if (
                not _nonempty_string(label)
                or len(label) > 128
                or "\n" in label
                or "\r" in label
            ):
                errors.append(f"{where}: label must be null or a short single-line taxonomy label")
            if status == "unlabeled":
                errors.append(f"{where}: a non-null label cannot have label_status='unlabeled'")
            if not _nonempty_string(row.get("label_source")):
                errors.append(f"{where}: labelled rows require a non-empty label_source")

        if not isinstance(row.get("hard_negative"), bool):
            errors.append(f"{where}: hard_negative must be boolean")
        elif row["hard_negative"] and action_labels and label in action_labels:
            errors.append(f"{where}: hard_negative row is labelled as an action class {label!r}")

        tags = row.get("tags", [])
        if not isinstance(tags, list) or any(not _nonempty_string(tag) for tag in tags):
            errors.append(f"{where}: tags must be a list of non-empty strings")

        digest = row.get("content_sha256")
        if require_hashes and not (isinstance(digest, str) and HEX_256.fullmatch(digest)):
            errors.append(f"{where}: content_sha256 is required and must be lowercase SHA-256")
        elif digest is not None and not (isinstance(digest, str) and HEX_256.fullmatch(digest)):
            errors.append(f"{where}: content_sha256 must be lowercase SHA-256 when present")
        elif isinstance(digest, str) and split in SPLITS:
            hash_splits[digest].add(split)
        if require_hashes and isinstance(digest, str) and HEX_256.fullmatch(digest) and _nonempty_string(media_path):
            candidate = pathlib.Path(media_path)
            if not candidate.is_absolute():
                candidate = root / candidate
            candidate = candidate.resolve()
            try:
                actual = verified_files.get(candidate)
                if actual is None:
                    hasher = hashlib.sha256()
                    with candidate.open("rb") as handle:
                        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                            hasher.update(chunk)
                    actual = hasher.hexdigest()
                    verified_files[candidate] = actual
                if actual != digest:
                    errors.append(
                        f"{where}: content_sha256 does not match media bytes for {candidate} "
                        f"(manifest={digest}, actual={actual})"
                    )
            except OSError as error:
                errors.append(f"{where}: cannot verify media hash for {candidate}: {error}")

        group_id = row.get("group_id")
        if group_id is not None and not _nonempty_string(group_id):
            errors.append(f"{where}: group_id must be a non-empty string when present")
        elif isinstance(group_id, str) and split in SPLITS:
            group_splits[group_id].add(split)

    for digest, splits in hash_splits.items():
        assigned = splits - {"unassigned"}
        if len(assigned) > 1:
            errors.append(f"content_sha256 {digest} leaks across splits: {sorted(assigned)}")
    for group_id, splits in group_splits.items():
        assigned = splits - {"unassigned"}
        if len(assigned) > 1:
            errors.append(f"group_id {group_id!r} leaks across splits: {sorted(assigned)}")
    return errors


def hash_manifest_files(rows: Sequence[dict[str, Any]], manifest_path: pathlib.Path) -> list[dict[str, Any]]:
    root = manifest_path.parent
    output: list[dict[str, Any]] = []
    for row in rows:
        clean = {key: value for key, value in row.items() if not key.startswith("__")}
        media_path = pathlib.Path(str(clean.get("media_path", "")))
        if not media_path.is_absolute():
            media_path = root / media_path
        if not media_path.is_file():
            raise ValidationError(f"{clean.get('id', '<unknown>')}: media file does not exist: {media_path}")
        digest = hashlib.sha256()
        with media_path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        clean["content_sha256"] = digest.hexdigest()
        output.append(clean)
    return output


def _split_bucket(key: str, salt: str) -> float:
    value = hashlib.sha256(f"{salt}\0{key}".encode()).digest()
    return int.from_bytes(value[:8], "big") / 2**64


def assign_splits(
    rows: Sequence[dict[str, Any]],
    *,
    development: float,
    calibration: float,
    holdout: float,
    salt: str,
) -> list[dict[str, Any]]:
    if any(value < 0 for value in (development, calibration, holdout)):
        raise ValidationError("split ratios cannot be negative")
    if not math.isclose(development + calibration + holdout, 1.0, abs_tol=1e-9):
        raise ValidationError("development + calibration + holdout ratios must equal 1")

    group_assignment: dict[str, str] = {}
    output: list[dict[str, Any]] = []
    for row in rows:
        clean = {key: value for key, value in row.items() if not key.startswith("__")}
        existing = clean.get("split")
        group_key = str(clean.get("group_id") or clean.get("content_sha256") or clean.get("id"))
        if existing != "unassigned":
            previous = group_assignment.setdefault(group_key, str(existing))
            if previous != existing:
                raise ValidationError(f"group {group_key!r} is already assigned to multiple splits")
        else:
            assigned = group_assignment.get(group_key)
            if assigned is None:
                bucket = _split_bucket(group_key, salt)
                assigned = "development"
                if bucket >= development:
                    assigned = "calibration"
                if bucket >= development + calibration:
                    assigned = "holdout"
                group_assignment[group_key] = assigned
            clean["split"] = assigned
        output.append(clean)
    return output


def _holdout_entries(rows: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    entries = []
    for row in rows:
        if row.get("split") != "holdout":
            continue
        entries.append(
            {
                "id": row.get("id"),
                "media_path": row.get("media_path"),
                "media_type": row.get("media_type"),
                "content_sha256": row.get("content_sha256"),
                "group_id": row.get("group_id"),
                "label": row.get("label"),
                "label_status": row.get("label_status"),
                "label_source": row.get("label_source"),
                "taxonomy_version": row.get("taxonomy_version"),
                "hard_negative": row.get("hard_negative"),
                "tags": sorted(row.get("tags", [])),
            }
        )
    return sorted(entries, key=lambda item: str(item["id"]))


def holdout_lock(rows: Sequence[dict[str, Any]]) -> dict[str, Any]:
    entries = _holdout_entries(rows)
    canonical = json.dumps(entries, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return {
        "schema_version": SCHEMA_VERSION,
        "purpose": "freeze final holdout membership, media hashes, labels, and review state",
        "holdout_sha256": hashlib.sha256(canonical.encode()).hexdigest(),
        "sample_count": len(entries),
        "samples": entries,
    }


def check_holdout_lock(rows: Sequence[dict[str, Any]], lock: dict[str, Any]) -> list[str]:
    current = holdout_lock(rows)
    errors: list[str] = []
    if lock.get("schema_version") != SCHEMA_VERSION:
        errors.append("holdout lock schema_version mismatch")
    if lock.get("holdout_sha256") != current["holdout_sha256"]:
        errors.append("holdout membership, hashes, labels, or review state changed")
    if lock.get("sample_count") != current["sample_count"]:
        errors.append("holdout sample_count changed")
    return errors


def dataset_summary(rows: Sequence[dict[str, Any]]) -> dict[str, Any]:
    def counts(key):
        return dict(sorted(collections.Counter(key(row) for row in rows).items(), key=lambda item: str(item[0])))

    hard_by_split = collections.Counter(str(row.get("split")) for row in rows if row.get("hard_negative"))
    return {
        "samples": len(rows),
        "labelled": sum(row.get("label") is not None for row in rows),
        "unlabelled": sum(row.get("label") is None for row in rows),
        "hard_negatives": sum(bool(row.get("hard_negative")) for row in rows),
        "by_split": counts(lambda row: str(row.get("split"))),
        "by_label": counts(lambda row: str(row.get("label"))),
        "by_media_type": counts(lambda row: str(row.get("media_type"))),
        "by_label_status": counts(lambda row: str(row.get("label_status"))),
        "hard_negatives_by_split": dict(sorted(hard_by_split.items())),
    }


def safe_rate(numerator: int | float, denominator: int | float) -> float | None:
    return numerator / denominator if denominator else None


def percentile(values: Sequence[float], percent: float) -> float | None:
    """Linear-interpolated percentile, equivalent to NumPy's default method."""
    clean = sorted(float(value) for value in values if math.isfinite(float(value)))
    if not clean:
        return None
    if len(clean) == 1:
        return clean[0]
    rank = (len(clean) - 1) * percent / 100.0
    low, high = math.floor(rank), math.ceil(rank)
    if low == high:
        return clean[low]
    return clean[low] + (clean[high] - clean[low]) * (rank - low)


def distribution(values: Sequence[float]) -> dict[str, float | int | None]:
    clean = [float(value) for value in values if math.isfinite(float(value))]
    return {
        "count": len(clean),
        "mean": statistics.fmean(clean) if clean else None,
        "p50": percentile(clean, 50),
        "p95": percentile(clean, 95),
        "p99": percentile(clean, 99),
        "max": max(clean) if clean else None,
    }


def classification_metrics(truth: Sequence[str], predicted: Sequence[str]) -> dict[str, Any]:
    if len(truth) != len(predicted):
        raise ValueError("truth and predicted must have equal length")
    labels = sorted(set(truth) | set(predicted))
    confusion = {actual: {guess: 0 for guess in labels} for actual in labels}
    for actual, guess in zip(truth, predicted):
        confusion[actual][guess] += 1

    per_class: dict[str, Any] = {}
    total = len(truth)
    sums = collections.Counter()
    for label in labels:
        tp = confusion[label][label]
        fp = sum(confusion[actual][label] for actual in labels if actual != label)
        fn = sum(confusion[label][guess] for guess in labels if guess != label)
        tn = total - tp - fp - fn
        precision = safe_rate(tp, tp + fp)
        recall = safe_rate(tp, tp + fn)
        fpr = safe_rate(fp, fp + tn)
        fnr = safe_rate(fn, fn + tp)
        f1 = safe_rate(2 * tp, 2 * tp + fp + fn)
        per_class[label] = {
            "support": sum(confusion[label].values()),
            "tp": tp,
            "fp": fp,
            "tn": tn,
            "fn": fn,
            "precision": precision,
            "recall": recall,
            "false_positive_rate": fpr,
            "false_negative_rate": fnr,
            "f1": f1,
        }
        sums.update(tp=tp, fp=fp, tn=tn, fn=fn)

    real_labels = [label for label in labels if label not in {PREDICTION_ERROR, "__UNCERTAIN__"}]
    macro_keys = ("precision", "recall", "false_positive_rate", "false_negative_rate", "f1")
    macro = {
        key: statistics.fmean(
            value for label in real_labels if (value := per_class[label][key]) is not None
        )
        if any(per_class[label][key] is not None for label in real_labels)
        else None
        for key in macro_keys
    }
    return {
        "samples": total,
        "accuracy": safe_rate(sum(actual == guess for actual, guess in zip(truth, predicted)), total),
        "macro": macro,
        "micro": {
            "precision": safe_rate(sums["tp"], sums["tp"] + sums["fp"]),
            "recall": safe_rate(sums["tp"], sums["tp"] + sums["fn"]),
            "false_positive_rate": safe_rate(sums["fp"], sums["fp"] + sums["tn"]),
            "false_negative_rate": safe_rate(sums["fn"], sums["fn"] + sums["tp"]),
            "f1": safe_rate(2 * sums["tp"], 2 * sums["tp"] + sums["fp"] + sums["fn"]),
        },
        "per_class": per_class,
        "confusion_matrix": {actual: dict(row) for actual, row in confusion.items()},
    }


def binary_metrics(actual: Sequence[bool], predicted: Sequence[bool]) -> dict[str, Any]:
    tp = sum(want and got for want, got in zip(actual, predicted))
    fp = sum(not want and got for want, got in zip(actual, predicted))
    tn = sum(not want and not got for want, got in zip(actual, predicted))
    fn = sum(want and not got for want, got in zip(actual, predicted))
    return {
        "samples": len(actual),
        "tp": tp,
        "fp": fp,
        "tn": tn,
        "fn": fn,
        "precision": safe_rate(tp, tp + fp),
        "recall": safe_rate(tp, tp + fn),
        "false_positive_rate": safe_rate(fp, fp + tn),
        "false_negative_rate": safe_rate(fn, fn + tp),
        "f1": safe_rate(2 * tp, 2 * tp + fp + fn),
    }


def load_resource_samples(path: pathlib.Path | None) -> dict[str, Any]:
    if path is None:
        return {
            "sample_count": 0,
            "cpu_percent": distribution([]),
            "ram_bytes": distribution([]),
            "gpu_percent": distribution([]),
            "vram_bytes": distribution([]),
            "queue_depth": distribution([]),
        }
    rows = read_jsonl(path)
    fields = ("cpu_percent", "ram_bytes", "gpu_percent", "vram_bytes", "queue_depth")
    result: dict[str, Any] = {"sample_count": len(rows)}
    for field in fields:
        result[field] = distribution(
            [row[field] for row in rows if isinstance(row.get(field), (int, float))]
        )
    return result


def _decision_is_action(prediction: dict[str, Any], action_labels: set[str]) -> bool:
    if prediction.get("timed_out") is True or _nonempty_string(prediction.get("error")):
        return False
    decision = prediction.get("decision")
    if isinstance(decision, str):
        return decision.upper() in {"ACTION", "BLOCK", "DELETE", "VIOLATION"}
    return prediction.get("predicted_label") in action_labels


def validate_predictions(
    rows: Sequence[dict[str, Any]], action_labels: set[str] | None = None
) -> list[str]:
    errors: list[str] = []
    seen: dict[str, int] = {}
    for at, row in enumerate(rows, 1):
        line = row.get("__line__", at)
        where = f"prediction line {line}"
        unknown_fields = sorted(set(row) - PREDICTION_FIELDS - {"__line__"})
        if unknown_fields:
            names_digest = hashlib.sha256("\0".join(unknown_fields).encode("utf-8")).hexdigest()
            errors.append(
                f"{where}: unsupported fields count={len(unknown_fields)} "
                f"names_sha256={names_digest}"
            )
        sample_id = row.get("id")
        if not _nonempty_string(sample_id):
            errors.append(f"{where}: id must be a non-empty string")
        elif sample_id in seen:
            errors.append(f"{where}: duplicate id {sample_id!r} (first on line {seen[sample_id]})")
        else:
            seen[str(sample_id)] = int(line)

        predicted = row.get("predicted_label")
        if predicted is not None and (
            not _nonempty_string(predicted) or len(predicted) > 128 or "\n" in predicted or "\r" in predicted
        ):
            errors.append(f"{where}: predicted_label must be null or a short single-line label")
        decision = row.get("decision")
        if decision is not None and decision not in DECISIONS:
            errors.append(f"{where}: decision must be one of {sorted(DECISIONS)}")
        timed_out = row.get("timed_out", False)
        if not isinstance(timed_out, bool):
            errors.append(f"{where}: timed_out must be boolean")
            timed_out = False
        error_value = row.get("error")
        has_error = error_value is not None
        if has_error and not _nonempty_string(error_value):
            errors.append(f"{where}: error must be a non-empty string when present")
        failed = timed_out or has_error or decision == "ERROR"
        if not failed and not _nonempty_string(predicted):
            errors.append(f"{where}: successful predictions require predicted_label")
        if failed:
            if decision == "ACTION" or (action_labels and predicted in action_labels):
                errors.append(f"{where}: timed-out/error predictions cannot be actionable")
            if predicted is not None:
                errors.append(f"{where}: timed-out/error predictions must set predicted_label to null")

        for field in ("score", "confidence", "latency_ms", "queue_wait_ms", "started_at_ms", "completed_at_ms", "gpu_time_ms"):
            value = row.get(field)
            if value is not None and (
                not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(float(value))
            ):
                errors.append(f"{where}: {field} must be a finite number when present")
        if isinstance(row.get("confidence"), (int, float)) and not 0 <= float(row["confidence"]) <= 1:
            errors.append(f"{where}: confidence must be between 0 and 1")
        for field in ("latency_ms", "queue_wait_ms", "gpu_time_ms"):
            if isinstance(row.get(field), (int, float)) and float(row[field]) < 0:
                errors.append(f"{where}: {field} cannot be negative")
        if (
            isinstance(row.get("started_at_ms"), (int, float))
            and isinstance(row.get("completed_at_ms"), (int, float))
            and float(row["completed_at_ms"]) < float(row["started_at_ms"])
        ):
            errors.append(f"{where}: completed_at_ms cannot precede started_at_ms")
        for field in ("cache_hit",):
            if field in row and not isinstance(row[field], bool):
                errors.append(f"{where}: {field} must be boolean")
        if "frames_processed" in row and (
            not isinstance(row["frames_processed"], int)
            or isinstance(row["frames_processed"], bool)
            or row["frames_processed"] < 0
        ):
            errors.append(f"{where}: frames_processed must be a non-negative integer")
        if "concurrency" in row and (
            not isinstance(row["concurrency"], int)
            or isinstance(row["concurrency"], bool)
            or row["concurrency"] < 1
        ):
            errors.append(f"{where}: concurrency must be a positive integer")
        for field in ("pipeline_version", "model_version"):
            if field in row and (
                not isinstance(row[field], str)
                or not VERSION_NAME.fullmatch(row[field])
            ):
                errors.append(f"{where}: {field} must match {VERSION_NAME.pattern}")
        stages = row.get("stage_ms")
        if stages is not None:
            if not isinstance(stages, dict):
                errors.append(f"{where}: stage_ms must be an object")
            else:
                for name, value in stages.items():
                    if not isinstance(name, str) or not STAGE_NAME.fullmatch(name):
                        errors.append(f"{where}: stage_ms names must match {STAGE_NAME.pattern}")
                    if (
                        not isinstance(value, (int, float))
                        or isinstance(value, bool)
                        or not math.isfinite(float(value))
                        or float(value) < 0
                    ):
                        errors.append(f"{where}: stage_ms values must be finite and non-negative")
    return errors


def evaluate(
    manifest_rows: Sequence[dict[str, Any]],
    prediction_rows: Sequence[dict[str, Any]],
    *,
    split: str,
    action_labels: set[str],
    wall_seconds: float | None = None,
    resource_path: pathlib.Path | None = None,
) -> dict[str, Any]:
    split_rows = [row for row in manifest_rows if row.get("split") == split]
    if not split_rows:
        raise ValidationError(f"manifest has no rows in split {split!r}")
    missing_labels = [str(row.get("id")) for row in split_rows if row.get("label") is None]
    if missing_labels:
        raise ValidationError(f"split {split!r} contains missing labels: {missing_labels[:10]}")
    if split == "holdout":
        non_adjudicated = [
            str(row.get("id")) for row in split_rows if row.get("label_status") != "adjudicated"
        ]
        if non_adjudicated:
            raise ValidationError(
                f"holdout evaluation requires adjudicated labels: {non_adjudicated[:10]}"
            )
    selected = split_rows
    prediction_errors = validate_predictions(prediction_rows, action_labels)
    if prediction_errors:
        raise ValidationError("\n".join(prediction_errors))
    predictions: dict[str, dict[str, Any]] = {}
    for row in prediction_rows:
        sample_id = row.get("id")
        if not _nonempty_string(sample_id):
            raise ValidationError("prediction rows require a non-empty id")
        predictions[sample_id] = row
    selected_ids = {str(row["id"]) for row in selected}
    unexpected = sorted(set(predictions) - selected_ids)

    truth: list[str] = []
    guessed: list[str] = []
    action_truth: list[bool] = []
    action_guessed: list[bool] = []
    hard_actual: list[bool] = []
    hard_guessed: list[bool] = []
    latencies: list[float] = []
    latency_missing_ids: list[str] = []
    latency_by_media: dict[str, list[float]] = collections.defaultdict(list)
    queue_wait: list[float] = []
    stage_values: dict[str, list[float]] = collections.defaultdict(list)
    frames_per_video: list[float] = []
    gpu_time: list[float] = []
    timeout_count = 0
    uncertain_count = 0
    cache_observations = 0
    cache_hits = 0
    error_ids: list[str] = []
    false_positive_ids: list[str] = []
    media_counts = collections.Counter()
    starts: list[float] = []
    ends: list[float] = []

    for item in selected:
        sample_id = str(item["id"])
        prediction = predictions.get(sample_id)
        if prediction is None:
            prediction = {"id": sample_id, "error": "missing prediction", "predicted_label": None}
        actual_label = str(item["label"])
        predicted_label = prediction.get("predicted_label")
        if (
            not _nonempty_string(predicted_label)
            or prediction.get("timed_out") is True
            or _nonempty_string(prediction.get("error"))
            or prediction.get("decision") == "ERROR"
        ):
            predicted_label = PREDICTION_ERROR
        truth.append(actual_label)
        guessed.append(str(predicted_label))
        want_action = actual_label in action_labels
        got_action = _decision_is_action(prediction, action_labels)
        action_truth.append(want_action)
        action_guessed.append(got_action)
        if not want_action and got_action:
            false_positive_ids.append(sample_id)
        if item.get("hard_negative"):
            hard_actual.append(want_action)
            hard_guessed.append(got_action)
        media_counts[str(item.get("media_type"))] += 1
        if str(prediction.get("decision", "")).upper() == "UNCERTAIN":
            uncertain_count += 1

        if isinstance(prediction.get("latency_ms"), (int, float)):
            latencies.append(float(prediction["latency_ms"]))
            latency_by_media[str(item.get("media_type"))].append(float(prediction["latency_ms"]))
        else:
            latency_missing_ids.append(sample_id)
        if isinstance(prediction.get("queue_wait_ms"), (int, float)):
            queue_wait.append(float(prediction["queue_wait_ms"]))
        stages = prediction.get("stage_ms")
        if isinstance(stages, dict):
            for name, value in stages.items():
                if isinstance(name, str) and isinstance(value, (int, float)):
                    stage_values[name].append(float(value))
        if prediction.get("timed_out") is True:
            timeout_count += 1
        if item.get("media_type") == "video" and isinstance(prediction.get("frames_processed"), (int, float)):
            frames_per_video.append(float(prediction["frames_processed"]))
        if isinstance(prediction.get("gpu_time_ms"), (int, float)):
            gpu_time.append(float(prediction["gpu_time_ms"]))
        if isinstance(prediction.get("cache_hit"), bool):
            cache_observations += 1
            cache_hits += int(prediction["cache_hit"])
        if _nonempty_string(prediction.get("error")) or prediction.get("timed_out") is True or prediction.get("decision") == "ERROR":
            error_ids.append(sample_id)
        if isinstance(prediction.get("started_at_ms"), (int, float)):
            starts.append(float(prediction["started_at_ms"]))
        if isinstance(prediction.get("completed_at_ms"), (int, float)):
            ends.append(float(prediction["completed_at_ms"]))

    measured_wall = wall_seconds
    if measured_wall is None and starts and ends:
        measured_wall = max(ends) / 1000.0 - min(starts) / 1000.0
    throughput = safe_rate(len(selected), measured_wall) if measured_wall and measured_wall > 0 else None
    media_per_second = {
        media_type: safe_rate(count, measured_wall) if measured_wall and measured_wall > 0 else None
        for media_type, count in sorted(media_counts.items())
    }
    hard_metrics = binary_metrics(hard_actual, hard_guessed) if hard_actual else None
    return {
        "schema_version": SCHEMA_VERSION,
        "split": split,
        "action_labels": sorted(action_labels),
        "coverage": {
            "expected": len(selected),
            "received": sum(
                sample_id in predictions
                and _nonempty_string(predictions[sample_id].get("predicted_label"))
                and not _nonempty_string(predictions[sample_id].get("error"))
                and predictions[sample_id].get("timed_out") is not True
                and predictions[sample_id].get("decision") != "ERROR"
                for sample_id in selected_ids
            ),
            "unexpected_prediction_ids": unexpected,
            "error_ids": error_ids,
        },
        "classification": classification_metrics(truth, guessed),
        "action": binary_metrics(action_truth, action_guessed),
        "hard_negative_action": hard_metrics,
        "false_positive_ids": false_positive_ids,
        "performance": {
            "latency_ms": distribution(latencies),
            "latency_coverage": {
                "expected": len(selected),
                "measured": len(latencies),
                "missing_ids": latency_missing_ids,
            },
            "latency_ms_by_media_type": {
                media_type: distribution(values) for media_type, values in sorted(latency_by_media.items())
            },
            "queue_wait_ms": distribution(queue_wait),
            "stage_ms": {name: distribution(values) for name, values in sorted(stage_values.items())},
            "frames_per_video": distribution(frames_per_video),
            "gpu_time_ms": distribution(gpu_time),
            "wall_seconds": measured_wall,
            "media_per_second": throughput,
            "by_media_type_per_second": media_per_second,
            "timeout_rate": safe_rate(timeout_count, len(selected)),
            "uncertain_rate": safe_rate(uncertain_count, len(selected)),
            "cache_hit_rate": safe_rate(cache_hits, cache_observations),
            "resource": load_resource_samples(resource_path),
        },
    }


def threshold_curve(
    manifest_rows: Sequence[dict[str, Any]],
    prediction_rows: Sequence[dict[str, Any]],
    *,
    split: str,
    action_labels: set[str],
    thresholds: Sequence[float] | None,
) -> list[dict[str, Any]]:
    if split != "calibration":
        raise ValidationError("threshold sweeps are allowed only on the calibration split")
    missing_labels = [
        str(row.get("id"))
        for row in manifest_rows
        if row.get("split") == split and row.get("label") is None
    ]
    if missing_labels:
        raise ValidationError(f"split {split!r} contains missing labels: {missing_labels[:10]}")
    prediction_errors = validate_predictions(prediction_rows, action_labels)
    if prediction_errors:
        raise ValidationError("\n".join(prediction_errors))
    labels = {
        str(row["id"]): str(row["label"])
        for row in manifest_rows
        if row.get("split") == split and row.get("label") is not None
    }
    if not labels:
        raise ValidationError(f"manifest has no labelled rows in split {split!r}")
    predictions = _prediction_map(prediction_rows)
    missing = [sample_id for sample_id in labels if sample_id not in predictions]
    invalid = [
        sample_id
        for sample_id in labels
        if sample_id in predictions
        and (
            not isinstance(predictions[sample_id].get("score"), (int, float))
            or isinstance(predictions[sample_id].get("score"), bool)
            or not math.isfinite(float(predictions[sample_id]["score"]))
            or predictions[sample_id].get("timed_out") is True
            or _nonempty_string(predictions[sample_id].get("error"))
            or predictions[sample_id].get("decision") == "ERROR"
        )
    ]
    if missing or invalid:
        raise ValidationError(
            "threshold sweep requires one valid numeric score for every selected ID; "
            f"missing={missing[:10]}, invalid_or_error={invalid[:10]}"
        )
    scored = [(label, float(predictions[sample_id]["score"])) for sample_id, label in labels.items()]
    if thresholds is None:
        unique = sorted({score for _, score in scored})
        if len(unique) > 201:
            thresholds = [percentile(unique, index / 2) for index in range(201)]
        else:
            thresholds = unique
    actual = [label in action_labels for label, _ in scored]
    output = []
    for threshold in thresholds:
        metrics = binary_metrics(actual, [score >= float(threshold) for _, score in scored])
        output.append({"threshold": float(threshold), **metrics})
    return output


def _prediction_map(rows: Sequence[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    output: dict[str, dict[str, Any]] = {}
    for row in rows:
        sample_id = row.get("id")
        if not _nonempty_string(sample_id):
            raise ValidationError("prediction rows require a non-empty id")
        if sample_id in output:
            raise ValidationError(f"duplicate prediction id: {sample_id!r}")
        output[str(sample_id)] = row
    return output


def compare_pipelines(
    manifest_rows: Sequence[dict[str, Any]],
    pipeline_rows: dict[str, Sequence[dict[str, Any]]],
    *,
    split: str,
    action_labels: set[str],
    threshold_sweep_enabled: bool = False,
    resource_paths: dict[str, pathlib.Path] | None = None,
) -> dict[str, Any]:
    if len(pipeline_rows) < 2:
        raise ValidationError("comparison requires at least two pipelines")
    split_rows = [row for row in manifest_rows if row.get("split") == split]
    if not split_rows:
        raise ValidationError(f"manifest has no rows in split {split!r}")
    missing_labels = [str(row.get("id")) for row in split_rows if row.get("label") is None]
    if missing_labels:
        raise ValidationError(f"split {split!r} contains missing labels: {missing_labels[:10]}")
    if split == "holdout":
        non_adjudicated = [
            str(row.get("id")) for row in split_rows if row.get("label_status") != "adjudicated"
        ]
        if non_adjudicated:
            raise ValidationError(
                f"holdout comparison requires adjudicated labels: {non_adjudicated[:10]}"
            )
    if threshold_sweep_enabled and split != "calibration":
        raise ValidationError("threshold sweeps are allowed only on the calibration split")

    maps = {name: _prediction_map(rows) for name, rows in pipeline_rows.items()}
    reports = {
        name: evaluate(
            manifest_rows,
            rows,
            split=split,
            action_labels=action_labels,
            resource_path=(resource_paths or {}).get(name),
        )
        for name, rows in pipeline_rows.items()
    }
    if threshold_sweep_enabled:
        for name, rows in pipeline_rows.items():
            reports[name]["threshold_curve"] = threshold_curve(
                manifest_rows,
                rows,
                split=split,
                action_labels=action_labels,
                thresholds=None,
            )

    paired = []
    names = list(pipeline_rows)
    for left_at, left in enumerate(names):
        for right in names[left_at + 1 :]:
            label_disagreements = []
            action_disagreements = []
            left_only_action = []
            right_only_action = []
            for item in split_rows:
                sample_id = str(item["id"])
                left_prediction = maps[left].get(sample_id, {"id": sample_id})
                right_prediction = maps[right].get(sample_id, {"id": sample_id})
                left_label = left_prediction.get("predicted_label") or PREDICTION_ERROR
                right_label = right_prediction.get("predicted_label") or PREDICTION_ERROR
                if left_label != right_label:
                    label_disagreements.append(sample_id)
                left_action = _decision_is_action(left_prediction, action_labels)
                right_action = _decision_is_action(right_prediction, action_labels)
                if left_action != right_action:
                    action_disagreements.append(sample_id)
                    (left_only_action if left_action else right_only_action).append(sample_id)
            paired.append(
                {
                    "left": left,
                    "right": right,
                    "samples": len(split_rows),
                    "label_disagreement_count": len(label_disagreements),
                    "label_disagreement_ids": label_disagreements,
                    "action_disagreement_count": len(action_disagreements),
                    "action_disagreement_ids": action_disagreements,
                    "left_only_action_count": len(left_only_action),
                    "left_only_action_ids": left_only_action,
                    "right_only_action_count": len(right_only_action),
                    "right_only_action_ids": right_only_action,
                }
            )
    canonical_manifest = json.dumps(
        [{key: value for key, value in row.items() if not key.startswith("__")} for row in manifest_rows],
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "manifest_sha256": hashlib.sha256(canonical_manifest.encode()).hexdigest(),
        "split": split,
        "action_labels": sorted(action_labels),
        "pipelines": reports,
        "paired_disagreements": paired,
    }


def _markdown_number(value: Any, *, percent: bool = False) -> str:
    if value is None:
        return "—"
    if percent:
        return f"{float(value) * 100:.2f}%"
    return f"{float(value):.3f}"


def comparison_markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Media pipeline comparison",
        "",
        f"Manifest SHA-256: `{report['manifest_sha256']}`  ",
        f"Split: `{report['split']}`  ",
        f"Action labels: `{', '.join(report['action_labels'])}`",
        "",
        "## Overall",
        "",
        "| Pipeline | Prediction coverage | Errors | Precision | Recall | FPR | FNR | F1 | Latency coverage | p50 ms | p95 ms | p99 ms | Timeout | Uncertain | Cache hit | media/s | CPU mean | RAM peak MiB | GPU mean | VRAM peak MiB |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for name, pipeline in report["pipelines"].items():
        action = pipeline["action"]
        latency = pipeline["performance"]["latency_ms"]
        latency_coverage = pipeline["performance"]["latency_coverage"]
        coverage = pipeline["coverage"]
        resource = pipeline["performance"]["resource"]
        ram_max = resource["ram_bytes"]["max"]
        vram_max = resource["vram_bytes"]["max"]
        lines.append(
            "| "
            + " | ".join(
                [
                    name.replace("|", "\\|"),
                    f"{coverage['received']}/{coverage['expected']}",
                    str(len(coverage["error_ids"])),
                    _markdown_number(action["precision"], percent=True),
                    _markdown_number(action["recall"], percent=True),
                    _markdown_number(action["false_positive_rate"], percent=True),
                    _markdown_number(action["false_negative_rate"], percent=True),
                    _markdown_number(action["f1"]),
                    f"{latency_coverage['measured']}/{latency_coverage['expected']}",
                    _markdown_number(latency["p50"]),
                    _markdown_number(latency["p95"]),
                    _markdown_number(latency["p99"]),
                    _markdown_number(pipeline["performance"]["timeout_rate"], percent=True),
                    _markdown_number(pipeline["performance"]["uncertain_rate"], percent=True),
                    _markdown_number(pipeline["performance"]["cache_hit_rate"], percent=True),
                    _markdown_number(pipeline["performance"]["media_per_second"]),
                    _markdown_number(resource["cpu_percent"]["mean"]),
                    _markdown_number(ram_max / 1024 / 1024 if ram_max is not None else None),
                    _markdown_number(resource["gpu_percent"]["mean"]),
                    _markdown_number(vram_max / 1024 / 1024 if vram_max is not None else None),
                ]
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "## Paired disagreements",
            "",
            "| Left | Right | Samples | Label disagreements | Action disagreements | Left-only actions | Right-only actions |",
            "|---|---|---:|---:|---:|---:|---:|",
        ]
    )
    for pair in report["paired_disagreements"]:
        lines.append(
            f"| {pair['left']} | {pair['right']} | {pair['samples']} | "
            f"{pair['label_disagreement_count']} | {pair['action_disagreement_count']} | "
            f"{pair['left_only_action_count']} | {pair['right_only_action_count']} |"
        )
    for name, pipeline in report["pipelines"].items():
        lines.extend(
            [
                "",
                f"## Per-class: {name}",
                "",
                "| Class | Support | Precision | Recall | FPR | FNR | F1 |",
                "|---|---:|---:|---:|---:|---:|---:|",
            ]
        )
        for label, metric in pipeline["classification"]["per_class"].items():
            lines.append(
                f"| {label} | {metric['support']} | "
                f"{_markdown_number(metric['precision'], percent=True)} | "
                f"{_markdown_number(metric['recall'], percent=True)} | "
                f"{_markdown_number(metric['false_positive_rate'], percent=True)} | "
                f"{_markdown_number(metric['false_negative_rate'], percent=True)} | "
                f"{_markdown_number(metric['f1'])} |"
            )
    lines.extend(["", "Full confusion matrices, error IDs, false-positive IDs, resource samples, and optional threshold curves are in the JSON report.", ""])
    return "\n".join(lines)


def _parse_labels(values: Sequence[str] | None) -> set[str]:
    labels = {item.strip() for value in values or [] for item in value.split(",") if item.strip()}
    if not labels:
        raise ValidationError("at least one --action-label is required")
    return labels


def _load_and_validate(args) -> list[dict[str, Any]]:
    rows = read_jsonl(args.manifest)
    action_labels = (
        {item.strip() for value in args.action_label or [] for item in value.split(",") if item.strip()}
        if hasattr(args, "action_label")
        else None
    )
    errors = validate_manifest(
        rows,
        manifest_path=args.manifest,
        require_files=getattr(args, "require_files", False),
        require_hashes=(
            getattr(args, "require_hashes", False)
            or bool(getattr(args, "holdout_lock", None))
        ),
        action_labels=action_labels,
    )
    if errors:
        raise ValidationError("\n".join(errors))
    return rows


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    validate = sub.add_parser("validate", help="validate a manifest and report its inventory")
    validate.add_argument("manifest", type=pathlib.Path)
    validate.add_argument("--require-files", action="store_true")
    validate.add_argument("--require-hashes", action="store_true")
    validate.add_argument("--action-label", action="append", default=[])
    validate.add_argument("--holdout-lock", type=pathlib.Path)

    hash_files = sub.add_parser("hash", help="compute content SHA-256 without changing labels")
    hash_files.add_argument("manifest", type=pathlib.Path)
    hash_files.add_argument("output", type=pathlib.Path)

    split = sub.add_parser("split", help="assign only unassigned rows using stable group hashing")
    split.add_argument("manifest", type=pathlib.Path)
    split.add_argument("output", type=pathlib.Path)
    split.add_argument("--development", type=float, default=0.70)
    split.add_argument("--calibration", type=float, default=0.15)
    split.add_argument("--holdout", type=float, default=0.15)
    split.add_argument("--salt", required=True, help="versioned project salt; keep it stable")

    freeze = sub.add_parser("freeze-holdout", help="write an immutable holdout membership lock")
    freeze.add_argument("manifest", type=pathlib.Path)
    freeze.add_argument("output", type=pathlib.Path)
    freeze.set_defaults(require_hashes=True)

    score = sub.add_parser("evaluate", help="join predictions to labels and emit quality/performance JSON")
    score.add_argument("manifest", type=pathlib.Path)
    score.add_argument("predictions", type=pathlib.Path)
    score.add_argument("--split", choices=SPLITS[:-1], required=True)
    score.add_argument("--action-label", action="append", required=True)
    score.add_argument("--wall-seconds", type=float)
    score.add_argument("--resources", type=pathlib.Path)
    score.add_argument("--holdout-lock", type=pathlib.Path)
    score.add_argument("--output", type=pathlib.Path)
    score.add_argument("--threshold-sweep", action="store_true")
    score.add_argument("--threshold", action="append", type=float)
    score.set_defaults(require_hashes=True)
    compare = sub.add_parser("compare", help="compare named pipelines on one immutable manifest")
    compare.add_argument("manifest", type=pathlib.Path)
    compare.add_argument("--split", choices=SPLITS[:-1], required=True)
    compare.add_argument("--action-label", action="append", required=True)
    compare.add_argument(
        "--pipeline",
        action="append",
        required=True,
        metavar="NAME=PREDICTIONS.jsonl",
        help="repeat for each candidate; names must be unique",
    )
    compare.add_argument("--threshold-sweep", action="store_true")
    compare.add_argument(
        "--resource",
        action="append",
        default=[],
        metavar="NAME=RESOURCES.jsonl",
        help="optional resource sample stream for a named pipeline",
    )
    compare.add_argument("--holdout-lock", type=pathlib.Path)
    compare.add_argument("--output-json", type=pathlib.Path, required=True)
    compare.add_argument("--output-markdown", type=pathlib.Path, required=True)
    compare.set_defaults(require_hashes=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.command == "hash":
            rows = read_jsonl(args.manifest)
            errors = validate_manifest(rows, manifest_path=args.manifest)
            if errors:
                raise ValidationError("\n".join(errors))
            write_jsonl(args.output, hash_manifest_files(rows, args.manifest))
            return 0

        rows = _load_and_validate(args)
        if args.command == "validate":
            if args.holdout_lock:
                lock = json.loads(args.holdout_lock.read_text(encoding="utf-8"))
                errors = check_holdout_lock(rows, lock)
                if errors:
                    raise ValidationError("\n".join(errors))
            print(json.dumps(dataset_summary(rows), ensure_ascii=False, indent=2, sort_keys=True))
            return 0
        if args.command == "split":
            output = assign_splits(
                rows,
                development=args.development,
                calibration=args.calibration,
                holdout=args.holdout,
                salt=args.salt,
            )
            errors = validate_manifest(output, manifest_path=args.output)
            if errors:
                raise ValidationError("\n".join(errors))
            write_jsonl(args.output, output)
            return 0
        if args.command == "freeze-holdout":
            missing = [str(row.get("id")) for row in rows if row.get("split") == "holdout" and not row.get("content_sha256")]
            if missing:
                raise ValidationError(f"holdout rows lack content_sha256: {missing[:10]}")
            lock = holdout_lock(rows)
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(lock, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            return 0
        if args.command == "evaluate":
            action_labels = _parse_labels(args.action_label)
            if (args.threshold_sweep or args.threshold) and args.split != "calibration":
                raise ValidationError("threshold sweeps are allowed only on the calibration split")
            if args.split == "holdout":
                if not args.holdout_lock:
                    raise ValidationError("holdout evaluation requires --holdout-lock")
                lock = json.loads(args.holdout_lock.read_text(encoding="utf-8"))
                errors = check_holdout_lock(rows, lock)
                if errors:
                    raise ValidationError("\n".join(errors))
            predictions = read_jsonl(args.predictions)
            report = evaluate(
                rows,
                predictions,
                split=args.split,
                action_labels=action_labels,
                wall_seconds=args.wall_seconds,
                resource_path=args.resources,
            )
            if args.threshold_sweep or args.threshold:
                report["threshold_curve"] = threshold_curve(
                    rows,
                    predictions,
                    split=args.split,
                    action_labels=action_labels,
                    thresholds=args.threshold,
                )
            rendered = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
            if args.output:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(rendered, encoding="utf-8")
            else:
                print(rendered, end="")
            return 0
        if args.command == "compare":
            action_labels = _parse_labels(args.action_label)
            if args.threshold_sweep and args.split != "calibration":
                raise ValidationError("threshold sweeps are allowed only on the calibration split")
            if args.split == "holdout":
                if not args.holdout_lock:
                    raise ValidationError("holdout comparison requires --holdout-lock")
                lock = json.loads(args.holdout_lock.read_text(encoding="utf-8"))
                errors = check_holdout_lock(rows, lock)
                if errors:
                    raise ValidationError("\n".join(errors))
            pipeline_rows: dict[str, list[dict[str, Any]]] = {}
            for specification in args.pipeline:
                if "=" not in specification:
                    raise ValidationError("--pipeline must use NAME=PREDICTIONS.jsonl")
                name, raw_path = specification.split("=", 1)
                name = name.strip()
                if not name:
                    raise ValidationError("pipeline name cannot be empty")
                if name in pipeline_rows:
                    raise ValidationError(f"duplicate pipeline name: {name!r}")
                pipeline_rows[name] = read_jsonl(pathlib.Path(raw_path))
            resource_paths: dict[str, pathlib.Path] = {}
            for specification in args.resource:
                if "=" not in specification:
                    raise ValidationError("--resource must use NAME=RESOURCES.jsonl")
                name, raw_path = specification.split("=", 1)
                if name not in pipeline_rows:
                    raise ValidationError(f"resource stream refers to unknown pipeline: {name!r}")
                if name in resource_paths:
                    raise ValidationError(f"duplicate resource stream for pipeline: {name!r}")
                resource_paths[name] = pathlib.Path(raw_path)
            report = compare_pipelines(
                rows,
                pipeline_rows,
                split=args.split,
                action_labels=action_labels,
                threshold_sweep_enabled=args.threshold_sweep,
                resource_paths=resource_paths,
            )
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            args.output_markdown.parent.mkdir(parents=True, exist_ok=True)
            args.output_json.write_text(
                json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            args.output_markdown.write_text(comparison_markdown(report), encoding="utf-8")
            return 0
        raise AssertionError(args.command)
    except (OSError, ValidationError, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
