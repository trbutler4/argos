#!/usr/bin/env python3
"""Read-only VM state listing checks."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def run(*args, env=None):
    return subprocess.run(args, env=env, text=True, capture_output=True, timeout=10)


def record(vm_id, **overrides):
    value = {
        "schema_version": 1,
        "id": vm_id,
        "name": vm_id.replace("-", " ").title(),
        "status": "stopped",
        "host": "test-host",
        "project": None,
        "repo_path": None,
        "workdir": None,
        "created_at_unix_ms": None,
        "updated_at_unix_ms": None,
    }
    value.update(overrides)
    return value


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    with tempfile.TemporaryDirectory(prefix="argos-vm-list-") as temp:
        root = Path(temp)
        missing = run(str(BINARY), "vm", "list", "--state-dir", str(root / "missing"), "--json")
        assert missing.returncode == 0, missing.stderr
        empty = json.loads(missing.stdout)
        assert empty["schema_version"] == 1
        assert empty["state_root"] == str(root / "missing")
        assert empty["vms"] == []
        assert not (root / "missing").exists(), "read-only list created a missing state directory"

        state = root / "state"
        vms = state / "vms"
        vms.mkdir(parents=True)
        (vms / "ignore.txt").write_text("not state")
        (vms / "b.json").write_text(json.dumps(record("beta", status="running", project="trade", repo_path="/repo/trade", workdir="/work/trade")))
        (vms / "a.json").write_text(json.dumps(record("alpha", name="Alpha VM")))
        result = run(str(BINARY), "vm", "list", "--state-dir", str(state), "--json")
        assert result.returncode == 0, result.stderr
        snapshot = json.loads(result.stdout)
        assert [vm["id"] for vm in snapshot["vms"]] == ["alpha", "beta"]
        assert snapshot["vms"][1]["status"] == "running"
        human = run(str(BINARY), "vm", "list", "--state-dir", str(state))
        assert human.returncode == 0, human.stderr
        assert '"Alpha VM" (stopped) on test-host' in human.stdout
        assert '"trade"' in human.stdout

        relative = run(str(BINARY), "vm", "list", "--state-dir", "relative", "--json")
        assert relative.returncode == 2
        (vms / "bad.json").write_text(json.dumps(record("bad/slash")))
        bad = run(str(BINARY), "vm", "list", "--state-dir", str(state), "--json")
        assert bad.returncode == 1 and "invalid_state" in bad.stderr
    print("PASS: VM list reads deterministic local state, handles empty/malformed state, and never creates VMs.")


if __name__ == "__main__":
    main()
