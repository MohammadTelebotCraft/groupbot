#!/usr/bin/env python3
"""Run actual Rust still-image and sampled-animation inference in one resident process.

No threshold tuning. Public data is diagnostic, not an adjudicated holdout. Labels are
never sent to the model. Native bounded evidence is saved separately from predictions.
For --pipeline concepts, manifest labels are safe or one of the six c_* settings keys;
the diagnostic arms all six at the production default limit. Custom-filter pipelines
use the ten fixed public proxy names built into the Rust adapter.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import queue
import re
import subprocess
import threading
import time

import media_eval

CONCEPT_LABELS = {"c_cig", "c_alc", "c_gun", "c_bet", "c_drg", "c_bld"}
FILTER_NAMES = ("tench", "English springer", "cassette player", "chain saw",
                "church", "French horn", "garbage truck", "gas pump",
                "golf ball", "parachute")
FILTER_LABELS = set(FILTER_NAMES)
FILTER_PIPELINES = {"image_filter", "example_filter", "example_filter_three",
                    "example_filter_candidates"}


def digest(path: pathlib.Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def public_manifest(cache: pathlib.Path, destination: pathlib.Path) -> list[dict]:
    sources = {"people": ("safe", "nlphuji/flickr30k"),
               "faces": ("safe", "nielsr/CelebA-faces"),
               "nsfw": ("nsfw", "x1101/nsfw-full")}
    rows = []
    for group, (label, source) in sources.items():
        for path in sorted((cache / group).glob("*.jpg")):
            rows.append({"schema_version": 1, "id": f"public-{group}-{path.stem}",
                "media_path": str(path.resolve()), "media_type": "image",
                "split": "development", "label": label, "label_status": "provisional",
                "label_source": f"upstream membership: {source}; not policy-adjudicated",
                "hard_negative": False, "tags": ["public", group, "not-workload-holdout"],
                "content_sha256": digest(path), "group_id": f"public-{group}-{path.stem}"})
    if not rows:
        raise ValueError("no cached public JPEG samples found")
    media_eval.write_jsonl(destination, rows)
    return rows


def read_worker(stream, responses: queue.Queue) -> None:
    try:
        for line in stream:
            responses.put(line)
    finally:
        responses.put(None)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    data = parser.add_mutually_exclusive_group(required=True)
    data.add_argument("--manifest", type=pathlib.Path)
    data.add_argument("--public-cache", type=pathlib.Path)
    parser.add_argument("--split", choices=["development", "calibration"], default="development")
    parser.add_argument("--pipeline", choices=["nsfw", "advert", "concepts", *sorted(FILTER_PIPELINES)],
                        default="nsfw")
    parser.add_argument("--media-kind", choices=["animated", "tgs"],
                        help="override ambiguous container routing for every selected row")
    parser.add_argument("--vision-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output-dir", type=pathlib.Path, required=True)
    parser.add_argument("--timeout", type=float, default=60.0)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    if args.public_cache and (args.split != "development" or args.pipeline != "nsfw"):
        parser.error("public cache supports NSFW development diagnostics only")
    args.output_dir.mkdir(parents=True, exist_ok=False)
    manifest_path = args.manifest or args.output_dir / "manifest.jsonl"
    rows = (media_eval.read_jsonl(manifest_path) if args.manifest
            else public_manifest(args.public_cache, manifest_path))
    action_labels = ({"nsfw"} if args.pipeline == "nsfw" else
                     {"advert"} if args.pipeline == "advert" else
                     CONCEPT_LABELS if args.pipeline == "concepts" else FILTER_LABELS)
    errors = media_eval.validate_manifest(rows, manifest_path=manifest_path,
        require_files=True, require_hashes=True, action_labels=action_labels)
    if errors:
        raise ValueError("\n".join(errors))
    selected = [row for row in rows if row["split"] == args.split]
    if not selected:
        raise ValueError("selected split is empty")
    if any(not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", row["id"]) for row in selected):
        raise ValueError("Rust adapter IDs must be 1-128 ASCII letters, digits, underscores, dots or hyphens")
    root = pathlib.Path(__file__).resolve().parent.parent
    if args.pipeline == "concepts":
        artifact_paths = {"binary": args.binary, "vision": args.vision_dir / "vision.onnx",
            "concept_policy_source": root / "src/handlers/concepts.rs",
            "concept_vectors_source": root / "src/handlers/concept_vectors.rs"}
    elif args.pipeline == "advert":
        artifact_paths = {"binary": args.binary,
            "ocr_source": root / "src/handlers/ocr.rs",
            "ocr_signals_source": root / "src/handlers/ocr/signals.rs"}
        for name in ("libarchive.so.13.7.2", "libgif.so.7.2.0", "libicudata.so.74.2",
                     "libicuuc.so.74.2", "liblept.so.5.0.4", "libtesseract.so.5.0.3",
                     "libxml2.so.2.9.14", "tesseract", "eng.traineddata", "fas.traineddata"):
            artifact_paths[f"tesseract/{name}"] = root / "assets/tesseract" / name
    elif args.pipeline in FILTER_PIPELINES:
        artifact_paths = {"binary": args.binary, "vision": args.vision_dir / "vision.onnx",
            "filter_source": root / "src/handlers/imgfilter.rs",
            "text_source": root / "src/handlers/imgtext.rs",
            "text_model": args.vision_dir / "vision_text.onnx",
            "text_vocab": args.vision_dir / "vision_text_vocab.txt",
            "text_merges": args.vision_dir / "vision_text_merges.txt"}
        if args.pipeline != "image_filter":
            count = 1 if args.pipeline == "example_filter" else 3
            for index, name in enumerate(FILTER_NAMES):
                for sample in range(count):
                    filename = f"{index:03}_{name.replace(' ', '_')}/{sample:05}.jpg"
                    artifact_paths[f"example/{index:03}/{sample}"] = (
                        root / "target/imgfilter-eval/imagenette" / filename)
    else:
        artifact_paths = {"binary": args.binary, "vision": args.vision_dir / "vision.onnx",
            "classifier": root / "assets/nsfw.onnx", "grader": root / "assets/nsfw_grade.onnx",
            "fitted_head_source": root / "src/handlers/nsfw_head_vectors.rs"}
    artifact_hashes = {name: digest(path) if path.is_file() else None
                       for name, path in artifact_paths.items()}
    environment = dict(os.environ, VISION_FILES=str(args.vision_dir.resolve()))
    predictions = []
    responses: queue.Queue = queue.Queue()
    started = time.perf_counter()
    with (args.output_dir / "worker.stderr.txt").open("w", encoding="utf-8") as stderr, \
         (args.output_dir / "native-evidence.jsonl").open("w", encoding="utf-8") as evidence:
        worker = subprocess.Popen([str(args.binary.resolve()), "--media-eval"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr,
            text=True, encoding="utf-8", bufsize=1, env=environment)
        reader = threading.Thread(target=read_worker, args=(worker.stdout, responses), daemon=True)
        reader.start()
        try:
            for row in selected:
                path = pathlib.Path(row["media_path"])
                if not path.is_absolute():
                    path = manifest_path.parent / path
                request = {"id": row["id"], "media_path": str(path.resolve()),
                           "pipeline": args.pipeline}
                if args.media_kind:
                    request["media_kind"] = args.media_kind
                request_started = time.perf_counter()
                worker.stdin.write(json.dumps(request) + "\n")
                worker.stdin.flush()
                try:
                    line = responses.get(timeout=args.timeout)
                except queue.Empty as error:
                    raise RuntimeError("worker timed out; refusing desynchronized results") from error
                if line is None:
                    raise RuntimeError("worker exited early; inspect worker.stderr.txt")
                native = json.loads(line)
                if native.get("id") != row["id"] or native.get("prediction", {}).get("id") != row["id"]:
                    raise ValueError("worker returned a mismatched sample ID")
                prediction = native["prediction"]
                prediction["started_at_ms"] = (request_started - started) * 1000.0
                prediction["completed_at_ms"] = (time.perf_counter() - started) * 1000.0
                prediction["concurrency"] = 1
                predictions.append(prediction)
                evidence.write(json.dumps(native, ensure_ascii=False, sort_keys=True) + "\n")
                evidence.flush()
            worker.stdin.close()
            if worker.wait(timeout=args.timeout) != 0:
                raise RuntimeError("worker failed after completing input")
            reader.join(timeout=1)
        finally:
            if worker.poll() is None:
                worker.kill()
                worker.wait()
            media_eval.write_jsonl(args.output_dir / "predictions.jsonl", predictions)
    elapsed = time.perf_counter() - started
    report = media_eval.evaluate(rows, predictions, split=args.split,
        action_labels=action_labels, wall_seconds=elapsed)
    final_hashes = {name: digest(path) if path.is_file() else None
                    for name, path in artifact_paths.items()}
    if artifact_hashes != final_hashes:
        raise ValueError("binary/model artifacts changed during evaluation; report refused")
    report["protocol"] = {
        "scope": "Rust still and sampled-animation inference on supplied bytes; no Telegram fetch, poster/thumbnail selection, cache or enforcement",
        "concurrency": 1, "resident_worker": True, "warmup_requests": 0,
        "cold_first_request_included": True, "elapsed_seconds": elapsed,
        "ground_truth_warning": "selected labels are not policy-adjudicated; not workload accuracy"
            if any(row.get("label_status") != "adjudicated" for row in selected) else None,
        "manifest_sha256": digest(manifest_path),
        "artifact_sha256": artifact_hashes}
    (args.output_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"output_dir": str(args.output_dir), "samples": len(predictions),
                     "elapsed_seconds": elapsed}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
