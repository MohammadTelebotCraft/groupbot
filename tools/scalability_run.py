"""Run isolated release diagnostics and retain machine-readable output.

Run under WSL/Linux from groupbot. No .env is read. Each case gets a new process
so allocator reuse between tests cannot masquerade as low per-group memory.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "audit/scalability/results"


def run_case(binary, label, test, **overrides):
    env = os.environ.copy()
    env.update(PERF_CLK_TCK=str(os.sysconf("SC_CLK_TCK")), VOICE_PYTHON="python3",
               VOICE_WORKERS="4", VOICE_MONITOR_SCRIPT="tools/scalability_voice_stub.py")
    env.update({key: str(value) for key, value in overrides.items()})
    path = OUT / (label + ".log")
    samples = []
    with path.open("w") as log:
        process = subprocess.Popen([str(binary), test, "--exact", "--ignored", "--nocapture", "--test-threads=1"],
                                   cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        started = time.monotonic()
        while process.poll() is None:
            try:
                status = Path(f"/proc/{process.pid}/status").read_text()
                fields = dict(line.split(":", 1) for line in status.splitlines() if ":" in line)
                samples.append({"seconds": time.monotonic()-started,
                                "rss_kib": int(fields["VmRSS"].split()[0]),
                                "threads": int(fields["Threads"].strip())})
            except (OSError, KeyError):
                pass
            time.sleep(0.1)
        if process.returncode:
            raise RuntimeError(f"{label} failed: {path.read_text()[-4000:]}")
    records = []
    for line in path.read_text().splitlines():
        if "PERF {" in line:
            record = json.loads(line.split("PERF ", 1)[1])
            record["label"] = label
            record["sampled_peak_rss_kib"] = max((s["rss_kib"] for s in samples), default=0)
            records.append(record)
            print(json.dumps(record), flush=True)
    (OUT / (label + ".samples.json")).write_text(json.dumps(samples))
    return records


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--soak", action="store_true")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    subprocess.run(["sh", "tools/scalability_pg.sh"], cwd=ROOT, check=True)
    records = []
    prefix = "handlers::scalability_tests::"
    for label, binary, dispatcher in [("baseline", args.baseline, "legacy"), ("after", args.binary, "fair")]:
        if binary is None:
            continue
        binary = binary.resolve()
        for repeat in range(3):
            records += run_case(binary, f"{label}-replay-{repeat}", prefix+"replay_dispatch", PERF_DISPATCHER=dispatcher)
        records += run_case(binary, label+"-allocations", prefix+"replay_dispatch", PERF_ALLOC=1, PERF_DISPATCHER=dispatcher)
        for active in [1, 10000]:
            records += run_case(binary, f"{label}-active-{active}", prefix+"replay_dispatch", PERF_ACTIVE=active, PERF_DISPATCHER=dispatcher)
        for name in ["flood_window_cost", "scan_allocations", "voice_worker_parallelism"]:
            records += run_case(binary, label+"-"+name, prefix+name)
    records += run_case(args.binary.resolve(), "after-database", "state::scalability_tests::database_delays",
                        DB_STATEMENT_TIMEOUT_MS=100, DB_LOCK_TIMEOUT_MS=50)
    records += run_case(args.binary.resolve(), "database-recovery", "state::scalability_tests::database_recovery",
                        DB_STATEMENT_TIMEOUT_MS=100, DB_LOCK_TIMEOUT_MS=50)
    records += run_case(args.binary.resolve(), "counter-contention", prefix+"counter_lock_contention")
    records += run_case(args.binary.resolve(), "after-stats", prefix+"fleet_stats_flush")
    records += run_case(args.binary.resolve(), "after-queues", "dispatcher::load_tests::queue_workloads")
    if args.soak:
        records += run_case(args.binary.resolve(), "after-soak", prefix+"replay_dispatch",
                            PERF_DISPATCHER="fair", PERF_UPDATES=15000000)
    (OUT / "measurements.json").write_text(json.dumps(records, indent=2)+"\n")


if __name__ == "__main__":
    main()
