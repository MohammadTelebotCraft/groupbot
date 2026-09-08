"""Final tests/measurements and source fingerprints after bounded receipt retention."""
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "tools"))
import scalability_run as runner
runner.OUT = OUT
binary = ROOT / "target/release/deps/groupbot-e0fabec816ad445f"
baseline = ROOT / "target/audit-followup-baseline-tests"
records = []
for iteration in range(3):
    for name, candidate in [("before", baseline), ("final", binary)]:
        records += runner.run_case(candidate, f"{name}-million-{iteration}", "handlers::scalability_tests::replay_dispatch",
                                   PERF_UPDATES=1000000, PERF_DISPATCHER="fair")
records += runner.run_case(binary, "final-owned-stats", "handlers::scalability_tests::fleet_stats_flush",
                          PERF_OWNERSHIP=1, DURABLE_WORK_DIR="/tmp/groupbot-audit-durable-work")
records += runner.run_case(binary, "final-ownership", "state::ownership::tests::checked_out_connection_is_fenced_and_monitor_closes_gate")
(OUT / "final-checks.json").write_text(json.dumps(records, indent=2) + "\n")
paths = [ROOT / "Cargo.lock", ROOT / "Cargo.toml", binary, baseline, ROOT / "target/release/groupbot"]
paths += list((ROOT / "src").rglob("*.rs")) + list((ROOT / "src").rglob("*.sql"))
paths += list((ROOT.parent / "grammers").glob("grammers-*/src/**/*.rs"))
fingerprints = {str(path.relative_to(ROOT.parent)): hashlib.file_digest(path.open("rb"), "sha256").hexdigest() for path in paths}
(OUT / "fingerprints.json").write_text(json.dumps({"at_utc": datetime.now(timezone.utc).isoformat(), "sha256": fingerprints}, indent=2) + "\n")
print("Final source/artifact fingerprints recorded", flush=True)
