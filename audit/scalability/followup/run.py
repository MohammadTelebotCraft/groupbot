"""Follow-up measurements; only the isolated audit database is used by these tests."""
import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools"))
import scalability_run as runner

OUT = Path(__file__).resolve().parent
runner.OUT = OUT
baseline = ROOT / "target/audit-followup-baseline-tests"
after = ROOT / "target/release/deps/groupbot-e0fabec816ad445f"
records = []

def run(binary, label, test, **env):
    records.extend(runner.run_case(binary, label, test, **env))
    (OUT / "measurements.json").write_text(json.dumps(records, indent=2) + "\n")

prefix = "handlers::scalability_tests::"
for repeat in range(3):
    for label, binary in [("before", baseline), ("after", after)]:
        run(binary, f"{label}-replay-{repeat}", prefix + "replay_dispatch", PERF_DISPATCHER="fair")
for label, binary in [("before", baseline), ("after", after)]:
    run(binary, label + "-allocations", prefix + "replay_dispatch", PERF_DISPATCHER="fair", PERF_ALLOC=1)
    run(binary, label + "-stats", prefix + "fleet_stats_flush")
for cap in ["legacy", "bounded"]:
    run(after, "rings-" + cap, prefix + "event_ring_memory", PERF_EVENT_CAP=cap)
run(after, "after-owned-stats", prefix + "fleet_stats_flush", PERF_OWNERSHIP=1,
    DURABLE_WORK_DIR="/tmp/groupbot-audit-durable-work")
for name in ["database_delays", "database_recovery"]:
    run(after, "after-" + name, "state::scalability_tests::" + name,
        DB_STATEMENT_TIMEOUT_MS=100, DB_LOCK_TIMEOUT_MS=50)
run(after, "after-owner-gate", "state::ownership::tests::checked_out_connection_is_fenced_and_monitor_closes_gate")
run(after, "after-durable", "state::durable::tests::failed_and_uncertain_statistics_commits_replay_once",
    DB_STATEMENT_TIMEOUT_MS=100, DB_LOCK_TIMEOUT_MS=50)
run(after, "after-queues", "dispatcher::load_tests::queue_workloads")
run(after, "after-counter-contention", prefix + "counter_lock_contention")
run(after, "after-soak", prefix + "replay_dispatch", PERF_DISPATCHER="fair", PERF_UPDATES=60000000)
