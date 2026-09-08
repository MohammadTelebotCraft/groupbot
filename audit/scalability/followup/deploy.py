"""Deploy the locked release artifact with a binary backup and automatic rollback.

Run under WSL from the repository. No application credentials are read or printed.
"""
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent
HOST = "groupbot-prod"
REMOTE = "/home/ubuntu/GroupManagement/groupbot/target/release/groupbot"
UNIT = "groupbot.service"
stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
backup = REMOTE + ".rollback-" + stamp
staged = REMOTE + ".next-" + stamp
binary = ROOT / "target/release/groupbot"
digest = hashlib.file_digest(binary.open("rb"), "sha256").hexdigest()
record = {"started_utc": stamp, "sha256": digest, "backup": backup, "target": REMOTE, "host": HOST}

def save():
    (OUT / "deployment.json").write_text(json.dumps(record, indent=2) + "\n")

def ssh(script, timeout=120):
    return subprocess.check_output(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", HOST, "bash -se"],
        input=script, text=True, timeout=timeout, stderr=subprocess.STDOUT).strip()

save()
print("Checking remote permissions and backing up the running binary", flush=True)
record["old_binary"] = ssh(f"sudo -n true\ntest -x {shlex.quote(REMOTE)}\ncp -p -- {shlex.quote(REMOTE)} {shlex.quote(backup)}\nsha256sum {shlex.quote(backup)}\n")
save()
subprocess.run(["scp", "-o", "BatchMode=yes", str(binary), HOST + ":" + staged], check=True)
remote_hash = ssh(f"sha256sum {shlex.quote(staged)}").split()[0]
if remote_hash != digest:
    raise RuntimeError("uploaded artifact checksum mismatch; running service was not changed")
record["dynamic_dependencies"] = ssh(f"ldd {shlex.quote(staged)}")
if "not found" in record["dynamic_dependencies"]:
    raise RuntimeError("missing dynamic dependency; running service was not changed")
save()
installed = False
try:
    print("Installing verified artifact and restarting groupbot.service", flush=True)
    installed = True
    ssh(f"chmod 755 {shlex.quote(staged)}\nmv -f -- {shlex.quote(staged)} {shlex.quote(REMOTE)}\nsudo -n systemctl restart {UNIT}\n", timeout=120)
    for attempt in range(24):
        status = ssh(f"systemctl show {UNIT} -p ActiveState -p SubState -p MainPID -p NRestarts -p MemoryCurrent -p ExecMainStartTimestamp\n")
        fields = dict(line.split("=", 1) for line in status.splitlines() if "=" in line)
        pid = int(fields.get("MainPID", "0"))
        ready = False
        if fields.get("ActiveState") == "active" and pid:
            actual = ssh(f"sha256sum /proc/{pid}/exe").split()[0]
            if actual != digest:
                raise RuntimeError("service executable does not match uploaded artifact")
            # Query only this invocation, avoiding unrelated historical logs and credentials.
            invocation = ssh(f"systemctl show {UNIT} -p InvocationID --value")
            lines = ssh(f"journalctl _SYSTEMD_INVOCATION_ID={shlex.quote(invocation)} --no-pager -o cat | tail -200")
            ready = "running" in lines.splitlines()
        record["latest_status"] = fields
        save()
        if ready:
            print(f"Application reached running state, PID {pid}; observing stability", flush=True)
            time.sleep(30)
            stable = ssh(f"systemctl show {UNIT} -p ActiveState -p MainPID -p NRestarts -p MemoryCurrent\n")
            stable_fields = dict(line.split("=", 1) for line in stable.splitlines() if "=" in line)
            if stable_fields.get("ActiveState") != "active" or stable_fields.get("MainPID") != str(pid):
                raise RuntimeError("service restarted or stopped during verification")
            record.update(status="deployed", verified_pid=pid, verified_utc=datetime.now(timezone.utc).isoformat(), final_status=stable_fields)
            save()
            print(json.dumps(record), flush=True)
            break
        print(f"Waiting for application readiness ({attempt + 1}/24), PID {pid}", flush=True)
        time.sleep(10)
    else:
        raise RuntimeError("application did not reach running state within readiness budget")
except Exception as error:
    record["error"] = str(error)
    if installed:
        print("Verification failed; restoring previous binary", flush=True)
        record["rollback_result"] = ssh(f"cp -p -- {shlex.quote(backup)} {shlex.quote(staged)}\nmv -f -- {shlex.quote(staged)} {shlex.quote(REMOTE)}\nsudo -n systemctl restart {UNIT}\nsystemctl is-active {UNIT}\n", timeout=120)
        record["status"] = "rolled_back"
    save()
    raise
