#!/usr/bin/env python3
"""Run a real media pipeline command at bounded concurrency levels.

The adapter command is an argv JSON array, never a shell string.  Placeholders available
to every argument are ``{id}``, ``{media_path}``, and ``{media_type}``.  The command must
write exactly one JSON object to stdout.  Its useful fields are ``predicted_label``,
``decision``, ``score``, and an optional ``stage_ms`` object.

Example:

    python tools/media_benchmark.py corpus.jsonl --split calibration \
      --command-json '["python", "adapter.py", "{media_path}"]' \
      --output-dir target/media-benchmark --concurrency 1 10 50 100

Every concurrency level emits prediction JSONL, host resource samples, and a summary.  No
label is sent to the adapter, so the harness cannot accidentally leak ground truth.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import ctypes
import hashlib
import json
import pathlib
import platform
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from typing import Any, Sequence

try:
    import media_eval
except ModuleNotFoundError:  # Supports `python -m tools.media_benchmark` in tests and CI.
    from tools import media_eval


ADAPTER_OUTPUT_FIELDS = {
    "predicted_label",
    "decision",
    "score",
    "confidence",
    "stage_ms",
    "cache_hit",
    "frames_processed",
    "gpu_time_ms",
    "pipeline_version",
    "model_version",
}


def _windows_host_sample(previous: tuple[int, int] | None) -> tuple[dict[str, float | int | None], tuple[int, int]]:
    class FileTime(ctypes.Structure):
        _fields_ = [("low", ctypes.c_uint32), ("high", ctypes.c_uint32)]

        def integer(self) -> int:
            return (self.high << 32) | self.low

    class MemoryStatus(ctypes.Structure):
        _fields_ = [
            ("length", ctypes.c_uint32),
            ("memory_load", ctypes.c_uint32),
            ("total_phys", ctypes.c_uint64),
            ("avail_phys", ctypes.c_uint64),
            ("total_page", ctypes.c_uint64),
            ("avail_page", ctypes.c_uint64),
            ("total_virtual", ctypes.c_uint64),
            ("avail_virtual", ctypes.c_uint64),
            ("avail_extended_virtual", ctypes.c_uint64),
        ]

    idle, kernel, user = FileTime(), FileTime(), FileTime()
    ctypes.windll.kernel32.GetSystemTimes(ctypes.byref(idle), ctypes.byref(kernel), ctypes.byref(user))
    idle_ticks = idle.integer()
    total_ticks = kernel.integer() + user.integer()
    cpu = None
    if previous:
        total_delta = total_ticks - previous[1]
        idle_delta = idle_ticks - previous[0]
        if total_delta > 0:
            cpu = 100.0 * (1.0 - idle_delta / total_delta)
    memory = MemoryStatus()
    memory.length = ctypes.sizeof(memory)
    ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(memory))
    return (
        {"cpu_percent": cpu, "ram_bytes": int(memory.total_phys - memory.avail_phys)},
        (idle_ticks, total_ticks),
    )


def _linux_host_sample(previous: tuple[int, int] | None) -> tuple[dict[str, float | int | None], tuple[int, int]]:
    cpu_fields = pathlib.Path("/proc/stat").read_text(encoding="ascii").splitlines()[0].split()[1:]
    values = [int(value) for value in cpu_fields]
    idle_ticks = values[3] + (values[4] if len(values) > 4 else 0)
    total_ticks = sum(values)
    cpu = None
    if previous:
        total_delta = total_ticks - previous[1]
        idle_delta = idle_ticks - previous[0]
        if total_delta > 0:
            cpu = 100.0 * (1.0 - idle_delta / total_delta)
    meminfo = {}
    for line in pathlib.Path("/proc/meminfo").read_text(encoding="ascii").splitlines():
        key, value = line.split(":", 1)
        meminfo[key] = int(value.strip().split()[0]) * 1024
    ram = meminfo.get("MemTotal", 0) - meminfo.get("MemAvailable", 0)
    return ({"cpu_percent": cpu, "ram_bytes": ram}, (idle_ticks, total_ticks))


def _nvidia_sample() -> dict[str, float | int | None]:
    if not shutil.which("nvidia-smi"):
        return {"gpu_percent": None, "vram_bytes": None}
    try:
        result = subprocess.run(
            [
                "nvidia-smi",
                "--query-gpu=utilization.gpu,memory.used",
                "--format=csv,noheader,nounits",
            ],
            capture_output=True,
            text=True,
            timeout=2,
            check=True,
        )
        rows = [line.split(",") for line in result.stdout.splitlines() if line.strip()]
        gpu = [float(row[0].strip()) for row in rows]
        vram = [float(row[1].strip()) * 1024 * 1024 for row in rows]
        return {
            "gpu_percent": sum(gpu) / len(gpu) if gpu else None,
            "vram_bytes": int(sum(vram)) if vram else None,
        }
    except (OSError, ValueError, subprocess.SubprocessError):
        return {"gpu_percent": None, "vram_bytes": None}


class ResourceSampler:
    """Samples host-wide CPU/RAM/GPU while the workload runs.

    Host-wide scope is intentional and is recorded in every sample.  Dedicated benchmark
    hosts give pipeline-attributable numbers; shared hosts do not.
    """

    def __init__(self, active, interval: float = 0.2):
        self.active = active
        self.interval = interval
        self.rows: list[dict[str, Any]] = []
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, name="media-resource-sampler", daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=max(2.0, self.interval * 3))

    def _run(self) -> None:
        previous = None
        while not self._stop.is_set():
            row: dict[str, Any] = {
                "timestamp_ms": time.time() * 1000.0,
                "scope": "host",
                "queue_depth": self.active.queue_depth(),
            }
            try:
                if platform.system() == "Windows":
                    system, previous = _windows_host_sample(previous)
                elif platform.system() == "Linux":
                    system, previous = _linux_host_sample(previous)
                else:
                    system = {"cpu_percent": None, "ram_bytes": None}
                row.update(system)
            except (OSError, ValueError):
                row.update(cpu_percent=None, ram_bytes=None)
            row.update(_nvidia_sample())
            self.rows.append(row)
            self._stop.wait(self.interval)


class ActiveWork:
    def __init__(self, submitted: int):
        self.submitted = submitted
        self.started = 0
        self.completed = 0
        self.lock = threading.Lock()

    def begin(self) -> None:
        with self.lock:
            self.started += 1

    def finish(self) -> None:
        with self.lock:
            self.completed += 1

    def queue_depth(self) -> int:
        with self.lock:
            return max(0, self.submitted - self.started)


@dataclass(frozen=True)
class WorkItem:
    row: dict[str, Any]
    submitted_at: float


def render_command(template: Sequence[str], row: dict[str, Any], manifest: pathlib.Path) -> list[str]:
    source = pathlib.Path(str(row["media_path"]))
    if not source.is_absolute():
        source = (manifest.parent / source).resolve()
    values = {
        "id": str(row["id"]),
        "media_path": str(source),
        "media_type": str(row["media_type"]),
    }
    try:
        return [part.format_map(values) for part in template]
    except KeyError as error:
        raise media_eval.ValidationError(f"unsupported command placeholder: {error}") from error


def validate_adapter_payload(
    payload: Any, allowed_labels: set[str] | None = None
) -> dict[str, Any]:
    if not isinstance(payload, dict):
        raise ValueError("stdout JSON must be an object")
    unsupported = sorted(set(payload) - ADAPTER_OUTPUT_FIELDS)
    if unsupported:
        names_digest = hashlib.sha256("\0".join(unsupported).encode("utf-8")).hexdigest()
        raise ValueError(
            f"unsupported adapter output fields count={len(unsupported)} "
            f"names_sha256={names_digest}"
        )
    candidate = {"id": "adapter-contract", **payload}
    errors = media_eval.validate_predictions([candidate])
    if (
        allowed_labels is not None
        and payload.get("predicted_label") is not None
        and payload.get("predicted_label") not in allowed_labels
    ):
        errors.append("predicted_label is outside the manifest taxonomy")
    if errors:
        raise ValueError("; ".join(errors))
    return payload


def run_hook(template: Sequence[str] | None, timeout: float) -> None:
    if not template:
        return
    try:
        process = subprocess.run(template, capture_output=True, timeout=timeout, shell=False)
    except subprocess.TimeoutExpired as error:
        raise media_eval.ValidationError(f"before-level hook exceeded {timeout:.3f}s") from error
    except OSError as error:
        raise media_eval.ValidationError(f"could not execute before-level hook: {error}") from error
    if process.returncode != 0:
        # Do not persist stderr: adapters and reset hooks may accidentally print user content.
        stderr_hash = hashlib.sha256(process.stderr).hexdigest()
        raise media_eval.ValidationError(
            f"before-level hook exited {process.returncode}; stderr_sha256={stderr_hash}"
        )


def independently_ordered(rows: Sequence[dict[str, Any]], seed: int) -> list[dict[str, Any]]:
    import random

    ordered = list(rows)
    random.Random(seed).shuffle(ordered)
    return ordered


def run_one(
    item: WorkItem,
    *,
    template: Sequence[str],
    manifest: pathlib.Path,
    timeout: float,
    active: ActiveWork,
    concurrency: int,
    allowed_labels: set[str],
) -> dict[str, Any]:
    started = time.time()
    active.begin()
    queue_wait_ms = (started - item.submitted_at) * 1000.0
    command = render_command(template, item.row, manifest)
    result: dict[str, Any] = {"id": str(item.row["id"]), "concurrency": concurrency}
    try:
        process = subprocess.run(command, capture_output=True, text=True, timeout=timeout, shell=False)
        completed = time.time()
        if process.returncode != 0:
            stderr_bytes = process.stderr.encode("utf-8", errors="replace")
            result["error"] = (
                f"adapter exited {process.returncode}; stderr_bytes={len(stderr_bytes)}; "
                f"stderr_sha256={hashlib.sha256(stderr_bytes).hexdigest()}"
            )
        else:
            try:
                payload = json.loads(process.stdout)
                result.update(validate_adapter_payload(payload, allowed_labels))
            except (json.JSONDecodeError, ValueError) as error:
                result["error"] = f"invalid adapter output: {error}"
    except subprocess.TimeoutExpired:
        completed = time.time()
        result.update(timed_out=True, error=f"adapter exceeded {timeout:.3f}s timeout")
    except OSError as error:
        completed = time.time()
        result["error"] = f"could not execute adapter: {error}"
    finally:
        active.finish()
    result.update(
        started_at_ms=started * 1000.0,
        completed_at_ms=completed * 1000.0,
        latency_ms=(completed - started) * 1000.0,
        queue_wait_ms=queue_wait_ms,
    )
    return result


def run_level(
    rows: Sequence[dict[str, Any]],
    *,
    concurrency: int,
    template: Sequence[str],
    manifest: pathlib.Path,
    timeout: float,
    output_dir: pathlib.Path,
    order_seed: int,
    cache_policy: str,
    before_level: Sequence[str] | None,
    allowed_labels: set[str],
) -> dict[str, Any]:
    if concurrency < 1:
        raise media_eval.ValidationError("concurrency must be at least 1")
    run_hook(before_level, timeout)
    submitted_at = time.time()
    active = ActiveWork(len(rows))
    sampler = ResourceSampler(active)
    sampler.start()
    work = [WorkItem(row=row, submitted_at=submitted_at) for row in rows]
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
            predictions = list(
                pool.map(
                    lambda item: run_one(
                        item,
                        template=template,
                        manifest=manifest,
                        timeout=timeout,
                        active=active,
                        concurrency=concurrency,
                        allowed_labels=allowed_labels,
                    ),
                    work,
                )
            )
    finally:
        sampler.stop()
    completed_at = time.time()
    wall_seconds = completed_at - submitted_at
    prediction_path = output_dir / f"predictions-c{concurrency}.jsonl"
    resource_path = output_dir / f"resources-c{concurrency}.jsonl"
    media_eval.write_jsonl(prediction_path, predictions)
    media_eval.write_jsonl(resource_path, sampler.rows)
    latencies = [float(row["latency_ms"]) for row in predictions]
    queue_wait = [float(row["queue_wait_ms"]) for row in predictions]
    summary = {
        "concurrency": concurrency,
        "order_seed": order_seed,
        "input_order_sha256": hashlib.sha256(
            "\0".join(str(row["id"]) for row in rows).encode("utf-8")
        ).hexdigest(),
        "cache_policy": cache_policy,
        "samples": len(rows),
        "wall_seconds": wall_seconds,
        "media_per_second": len(rows) / wall_seconds if wall_seconds else None,
        "latency_ms": media_eval.distribution(latencies),
        "queue_wait_ms": media_eval.distribution(queue_wait),
        "timeout_rate": sum(row.get("timed_out") is True for row in predictions) / len(rows) if rows else None,
        "error_rate": sum(bool(row.get("error")) for row in predictions) / len(rows) if rows else None,
        "resources": media_eval.load_resource_samples(resource_path),
        "predictions": str(prediction_path),
        "resource_samples": str(resource_path),
    }
    (output_dir / f"summary-c{concurrency}.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return summary


def warm_up(rows, count, template, manifest, timeout, allowed_labels) -> None:
    if count <= 0 or not rows:
        return
    active = ActiveWork(min(count, len(rows)))
    for row in rows[:count]:
        result = run_one(
            WorkItem(row=row, submitted_at=time.time()),
            template=template,
            manifest=manifest,
            timeout=timeout,
            active=active,
            concurrency=1,
            allowed_labels=allowed_labels,
        )
        if result.get("error"):
            raise media_eval.ValidationError(f"warm-up failed for {row['id']}: {result['error']}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("--split", choices=media_eval.SPLITS[:-1], required=True)
    parser.add_argument("--command-json", required=True, help="JSON argv array with safe placeholders")
    parser.add_argument("--output-dir", type=pathlib.Path, required=True)
    parser.add_argument("--concurrency", nargs="+", type=int, default=[1, 10, 50, 100])
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--warmup", type=int, default=0)
    parser.add_argument("--limit", type=int)
    parser.add_argument("--shuffle-seed", type=int, default=20260919)
    parser.add_argument(
        "--allowed-label",
        action="append",
        default=[],
        help="declared taxonomy label; useful for wholly unlabelled performance manifests",
    )
    parser.add_argument(
        "--cache-policy",
        choices=("cold", "warm", "realistic"),
        default="cold",
        help="cold is process-isolated and performs no warm-up; external caches need a reset hook",
    )
    parser.add_argument(
        "--before-level-command-json",
        help="optional JSON argv array that resets external/persistent cache before every level",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        rows = media_eval.read_jsonl(args.manifest)
        errors = media_eval.validate_manifest(
            rows, manifest_path=args.manifest, require_files=True, require_hashes=True
        )
        if errors:
            raise media_eval.ValidationError("\n".join(errors))
        allowed_labels = {
            str(row["label"]) for row in rows if isinstance(row.get("label"), str)
        }
        allowed_labels.update(label for label in args.allowed_label if label)
        if not allowed_labels:
            raise media_eval.ValidationError(
                "benchmark manifest needs at least one labelled taxonomy class"
            )
        rows = [row for row in rows if row.get("split") == args.split]
        if not rows:
            raise media_eval.ValidationError(f"manifest has no rows in split {args.split!r}")
        try:
            template = json.loads(args.command_json)
        except json.JSONDecodeError as error:
            raise media_eval.ValidationError(f"--command-json is invalid JSON: {error.msg}") from error
        if not isinstance(template, list) or not template or any(not isinstance(part, str) for part in template):
            raise media_eval.ValidationError("--command-json must be a non-empty JSON array of strings")
        if args.timeout <= 0:
            raise media_eval.ValidationError("--timeout must be positive")
        if args.warmup < 0:
            raise media_eval.ValidationError("--warmup cannot be negative")
        if args.cache_policy == "cold" and args.warmup:
            raise media_eval.ValidationError("cold cache policy forbids warm-up")
        before_level = None
        if args.before_level_command_json:
            try:
                before_level = json.loads(args.before_level_command_json)
            except json.JSONDecodeError as error:
                raise media_eval.ValidationError(
                    f"--before-level-command-json is invalid JSON: {error.msg}"
                ) from error
            if (
                not isinstance(before_level, list)
                or not before_level
                or any(not isinstance(part, str) for part in before_level)
            ):
                raise media_eval.ValidationError(
                    "--before-level-command-json must be a non-empty JSON array of strings"
                )
        if args.limit is not None:
            if args.limit < 1:
                raise media_eval.ValidationError("--limit must be positive")
            rows = independently_ordered(rows, args.shuffle_seed)[: args.limit]
        if len(set(args.concurrency)) != len(args.concurrency):
            raise media_eval.ValidationError("--concurrency levels must be unique")
        args.output_dir.mkdir(parents=True, exist_ok=True)
        if args.cache_policy in {"warm", "realistic"}:
            warm_up(rows, args.warmup, template, args.manifest, args.timeout, allowed_labels)
        summaries = []
        for index, level in enumerate(args.concurrency):
            order_seed = args.shuffle_seed + index * 1_000_003 + level * 10_007
            level_rows = independently_ordered(rows, order_seed)
            summaries.append(
                run_level(
                    level_rows,
                    concurrency=level,
                    template=template,
                    manifest=args.manifest,
                    timeout=args.timeout,
                    output_dir=args.output_dir,
                    order_seed=order_seed,
                    cache_policy=args.cache_policy,
                    before_level=before_level,
                    allowed_labels=allowed_labels,
                )
            )
        external_cache_reset = before_level is not None
        limitation = None
        if args.cache_policy == "cold" and not external_cache_reset:
            limitation = (
                "Each sample uses a new adapter process, but caches in a remote or persistent "
                "service are not reset. Supply --before-level-command-json on a dedicated host."
            )
        report = {
            "schema_version": media_eval.SCHEMA_VERSION,
            "manifest": str(args.manifest),
            "split": args.split,
            "adapter_executable": pathlib.Path(template[0]).name,
            "command_argv_sha256": hashlib.sha256(
                json.dumps(template, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
            ).hexdigest(),
            "cache": {
                "policy": args.cache_policy,
                "adapter_process": "new process per sample",
                "warmup_requests": args.warmup,
                "external_cache_reset_before_each_level": external_cache_reset,
                "limitation": limitation,
                "independent_order_per_level": True,
            },
            "host_resource_scope": "host-wide; use a dedicated benchmark host for attribution",
            "levels": summaries,
        }
        report_path = args.output_dir / "benchmark.json"
        report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(report_path)
        return 0
    except (OSError, media_eval.ValidationError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
