"""Final offline checks after the paired measurements; run from groupbot under Linux."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from scalability_run import run_case

OUT = ROOT / "audit/scalability/results"
BINARY = (ROOT / sys.argv[1]).resolve()
records = []
records += run_case(BINARY, "final-database", "state::scalability_tests::database_delays",
                    DB_STATEMENT_TIMEOUT_MS=100, DB_LOCK_TIMEOUT_MS=50)
records += run_case(BINARY, "final-database-recovery", "state::scalability_tests::database_recovery",
                    DB_STATEMENT_TIMEOUT_MS=100, DB_LOCK_TIMEOUT_MS=50)
records += run_case(BINARY, "final-counter-contention", "handlers::scalability_tests::counter_lock_contention")
records += run_case(BINARY, "final-extended-replay", "handlers::scalability_tests::replay_dispatch",
                    PERF_DISPATCHER="fair", PERF_UPDATES=15000000)
(OUT / "final-validation.json").write_text(json.dumps(records, indent=2)+"\n")

def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

def command(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()

names = {"tokio", "sqlx", "sqlx-core", "sqlx-postgres", "reqwest", "axum", "ort", "libsql", "image"}
packages = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
source_paths = [ROOT / "Cargo.lock", ROOT / "Cargo.toml", BINARY, ROOT / "target/audit-baseline-tests"]
source_paths += list((OUT).glob("*.baseline.rs"))
for repository in [ROOT, ROOT.parent / "grammers"]:
    source_paths += list((repository / "src").rglob("*.rs")) if repository == ROOT else [
        path for path in repository.glob("grammers-*/src/**/*.rs")]
for package, suffix in [("tokio-1.53.1", "src/sync/notify.rs"), ("sqlx-core-0.8.6", "src/pool/options.rs")]:
    source_paths += list((Path.home()/".cargo/registry/src").glob(f"*/{package}/{suffix}"))
manifest = {
    "measured_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "system": platform.platform(), "logical_cpus": os.cpu_count(),
    "rustc": command(str(Path.home()/".cargo/bin/rustc"), "-Vv"),
    "postgres": command("/usr/lib/postgresql/16/bin/postgres", "--version"),
    "libc": command("ldd", "--version").splitlines()[0],
    "memory": Path("/proc/meminfo").read_text().splitlines()[:3],
    "packages": [{"name": p["name"], "version": p["version"]} for p in packages if p["name"] in names or p["name"].startswith("grammers-")],
    "repositories": {str(repo): {"head": command("git", "rev-parse", "HEAD", cwd=repo),
                                     "status": command("git", "status", "--short", cwd=repo)}
                     for repo in [ROOT, ROOT.parent/"grammers"]},
    "sha256": {str(path): digest(path) for path in source_paths if path.is_file()},
    "caveat": "Baseline was a dirty working state. Critical source snapshots and the retained test binary identify the measured baseline; these are not a full source archive."
}
(OUT / "environment.json").write_text(json.dumps(manifest, indent=2)+"\n")
