#!/usr/bin/env python3
"""VM state create/list checks. Create writes state only and never starts guests."""
import json
import os
from pathlib import Path
import stat
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

        create_state = root / "create-state"
        work_root = root / "work-root"
        source = root / "source.git"
        init = run("git", "init", "--bare", str(source))
        assert init.returncode == 0, init.stderr
        dry = run(str(BINARY), "vm", "create", "Trade Feature", "--repo", str(source), "--project", "trade", "--state-dir", str(create_state), "--work-root", str(work_root), "--dry-run", "--json")
        assert dry.returncode == 0, dry.stderr
        dry_value = json.loads(dry.stdout)
        assert dry_value["dry_run"] is True and dry_value["record"]["id"] == "trade-feature"
        assert not create_state.exists(), "dry-run wrote state"
        created = run(str(BINARY), "vm", "create", "Trade Feature", "--repo", str(source), "--project", "trade", "--state-dir", str(create_state), "--work-root", str(work_root), "--json")
        assert created.returncode == 0, created.stderr
        value = json.loads(created.stdout)
        assert value["dry_run"] is False
        assert Path(value["state_file"]).is_file()
        assert Path(value["microvm_config"]).read_text().count("ARGOS_VM_READY") == 1
        assert (Path(value["microvm_config"]).parent / "flake.nix").is_file()
        assert (work_root / "trade-feature" / "repo" / ".git").is_dir()
        listed = json.loads(run(str(BINARY), "vm", "list", "--state-dir", str(create_state), "--json").stdout)
        assert [vm["id"] for vm in listed["vms"]] == ["trade-feature"]
        assert listed["vms"][0]["status"] == "created"
        duplicate = run(str(BINARY), "vm", "create", "Trade Feature", "--state-dir", str(create_state), "--json")
        assert duplicate.returncode == 1 and "already exists" in duplicate.stderr
        bad_args = run(str(BINARY), "vm", "create", "bad", "--id", "bad/slash", "--state-dir", str(create_state), "--json")
        assert bad_args.returncode == 2

        fake_bin = root / "fake-bin"
        fake_store = root / "fake-store"
        fake_bin.mkdir()
        fake_nix = fake_bin / "nix"
        fake_nix.write_text(f"""#!/usr/bin/env python3
import os
import pathlib
import stat
import sys

args = sys.argv[1:]
if len(args) < 4 or args[:2] != ["build", ".#runner"] or "--out-link" not in args:
    print("unexpected fake nix args", args, file=sys.stderr)
    sys.exit(9)
out = pathlib.Path(args[args.index("--out-link") + 1])
runner = pathlib.Path({str(fake_store)!r}) / out.parent.name
(runner / "bin").mkdir(parents=True, exist_ok=True)
run = runner / "bin" / "microvm-run"
run.write_text('''#!/bin/sh\nid=$(basename "$PWD")\necho "ARGOS_VM_READY id=$id host=fake"\nexec sleep 60\n''')
run.chmod(run.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
if out.exists() or out.is_symlink():
    out.unlink()
out.symlink_to(runner)
""")
        fake_nix.chmod(fake_nix.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        fake_env = os.environ.copy()
        fake_env["PATH"] = f"{fake_bin}:{fake_env['PATH']}"
        started = run(str(BINARY), "vm", "start", "trade-feature", "--state-dir", str(create_state), "--json", env=fake_env)
        assert started.returncode == 0, started.stderr
        start_value = json.loads(started.stdout)
        assert start_value["already_running"] is False
        assert start_value["record"]["status"] == "running"
        assert start_value["record"]["pid"] == start_value["pid"]
        assert Path(start_value["log_path"]).read_text().count("ARGOS_VM_READY id=trade-feature") == 1
        again = run(str(BINARY), "vm", "start", "trade-feature", "--state-dir", str(create_state), "--json", env=fake_env)
        assert again.returncode == 0, again.stderr
        assert json.loads(again.stdout)["already_running"] is True
        stopped = run(str(BINARY), "vm", "stop", "trade-feature", "--state-dir", str(create_state), "--json")
        assert stopped.returncode == 0, stopped.stderr
        stop_value = json.loads(stopped.stdout)
        assert stop_value["already_stopped"] is False
        assert stop_value["record"]["status"] == "stopped"
        assert stop_value["record"]["pid"] is None
        stopped_again = run(str(BINARY), "vm", "stop", "trade-feature", "--state-dir", str(create_state), "--json")
        assert stopped_again.returncode == 0, stopped_again.stderr
        assert json.loads(stopped_again.stdout)["already_stopped"] is True

        (vms / "bad.json").write_text(json.dumps(record("bad/slash")))
        bad = run(str(BINARY), "vm", "list", "--state-dir", str(state), "--json")
        assert bad.returncode == 1 and "invalid_state" in bad.stderr
    print("PASS: VM create/start/stop/list manages deterministic local state, clone/config scaffolds, malformed state, fake local runner lifecycle, and no remote hosts.")


if __name__ == "__main__":
    main()
