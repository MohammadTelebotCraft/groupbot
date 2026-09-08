"""Read-only post-deployment checks; does not read credentials or send Telegram actions."""
from datetime import datetime, timezone
import json
from pathlib import Path
import shlex
import subprocess
import time

OUT = Path(__file__).resolve().parent
deployment = json.loads((OUT / "deployment.json").read_text())
expected_pid = str(deployment["verified_pid"])

def ssh(script):
    return subprocess.check_output(["ssh", "-o", "BatchMode=yes", "groupbot-prod", "bash -se"],
        input=script, text=True, timeout=30, stderr=subprocess.STDOUT).strip()

samples = []
for index in range(4):
    if index:
        time.sleep(30)
    raw = ssh("systemctl show groupbot.service -p ActiveState -p MainPID -p NRestarts -p MemoryCurrent -p CPUUsageNSec -p TasksCurrent")
    fields = dict(line.split("=", 1) for line in raw.splitlines() if "=" in line)
    assert fields["ActiveState"] == "active" and fields["MainPID"] == expected_pid, raw
    fields["at_utc"] = datetime.now(timezone.utc).isoformat()
    samples.append(fields)
    print(json.dumps(fields), flush=True)
    (OUT / "production-observation.json").write_text(json.dumps(samples, indent=2) + "\n")

invocation = ssh("systemctl show groupbot.service -p InvocationID --value")
logs = ssh(f"journalctl _SYSTEMD_INVOCATION_ID={shlex.quote(invocation)} -o cat --no-pager | tail -500")
capacity = [line for line in logs.splitlines() if "capacity:" in line or "dispatch:" in line]
(OUT / "production-capacity.log").write_text("\n".join(capacity) + "\n")
print("\n".join(capacity[-6:]), flush=True)
errors = [line[:300] for line in logs.splitlines() if any(term in line.lower() for term in
    ["ownership lost", "retaining durable batch", "stale groupbot ownership", "panicked", "syntax error", "receipt cleanup deferred"])]
assert not errors, errors

# Find the application's database using only the known, non-sensitive heartbeat SQL.
query = "SELECT DISTINCT datname FROM pg_stat_activity WHERE query='SELECT epoch FROM runtime_owner WHERE id=0'"
database = ssh("sudo -n -u postgres psql -Atqc " + shlex.quote(query))
assert database and "\n" not in database, "could not uniquely identify the active database heartbeat"
verification = "SELECT json_build_object('server_version',current_setting('server_version'),'epoch',(SELECT epoch FROM runtime_owner WHERE id=0),'fencing_triggers',(SELECT count(*) FROM pg_trigger WHERE tgname='groupbot_owner_fence'),'stats_receipts',(SELECT count(*) FROM applied_stats_batches))"
fencing = json.loads(ssh("sudo -n -u postgres psql -d " + shlex.quote(database) + " -Atqc " + shlex.quote(verification)))
assert fencing["epoch"] > 0 and fencing["fencing_triggers"] > 0
(OUT / "production-fencing.json").write_text(json.dumps(fencing, indent=2) + "\n")
print(json.dumps(fencing), flush=True)
