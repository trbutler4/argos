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


def run(*args, env=None, cwd=None):
    return subprocess.run(args, env=env, cwd=cwd, text=True, capture_output=True, timeout=10)


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


def config_text(tmux_section):
    return f'''schema_version = 1
[client]
machine_id = "local"

[machines.local]

[vm.guest_tmux]
{tmux_section}
'''


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
        assert human.stdout.splitlines() == ["alpha", "beta"]
        detailed = run(str(BINARY), "vm", "list", "--state-dir", str(state), "--detailed")
        assert detailed.returncode == 0, detailed.stderr
        assert '"Alpha VM" (stopped) on test-host' in detailed.stdout
        assert '"trade"' in detailed.stdout

        relative = run(str(BINARY), "vm", "list", "--state-dir", "relative", "--json")
        assert relative.returncode == 2

        create_state = root / "create-state"
        work_root = root / "work-root"
        tmux_conf = root / "guest.tmux.conf"
        tmux_conf.write_text("set -g status-left 'argos-vm-test'\n")
        argos_config = root / "argos.toml"
        argos_config.write_text(config_text(f'config_path = "{tmux_conf}"'))
        inline_config = root / "argos-inline.toml"
        inline_config.write_text(config_text('config_text = "set -g status-right inline"'))
        bad_tmux_config = root / "argos-bad-tmux.toml"
        bad_tmux_config.write_text(config_text(f'config_text = "a"\nconfig_path = "{tmux_conf}"'))
        profile = root / "repo.argos.toml"
        profile.write_text('''[workspace]
workdir = "/workspace/repo"

[vm]
packages = ["go", "just"]

[[ports]]
name = "api"
guest = 3001

[[ports]]
name = "web"
guest = 5173
''')
        bad_profile = root / "bad.argos.toml"
        bad_profile.write_text('[vm]\npackages = ["../nope"]\n')
        source = root / "source.git"
        init = run("git", "init", "--bare", str(source))
        assert init.returncode == 0, init.stderr
        local_source = root / "local-source"
        clone_source = run("git", "clone", str(source), str(local_source))
        assert clone_source.returncode == 0, clone_source.stderr
        fake_home = root / "home"
        (fake_home / ".ssh").mkdir(parents=True)
        (fake_home / ".ssh" / "known_hosts").write_text("example ssh-ed25519 AAAAhost\n")
        (fake_home / ".gitconfig").write_text('[user]\n\tname = Argos Tester\n\temail = tester@example.invalid\n')
        home_env = os.environ.copy()
        home_env["HOME"] = str(fake_home)
        dry = run(str(BINARY), "vm", "create", "Trade Feature", "--repo", str(local_source), "--project", "trade", "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--dry-run", "--json")
        assert dry.returncode == 0, dry.stderr
        dry_value = json.loads(dry.stdout)
        assert dry_value["dry_run"] is True and dry_value["record"]["id"] == "trade-feature"
        assert dry_value["record"]["packages"] == ["go", "just"]
        assert [port["guest"] for port in dry_value["record"]["ports"]] == [3001, 5173]
        assert not create_state.exists(), "dry-run wrote state"
        invalid_config = run(str(BINARY), "vm", "create", "bad config", "--state-dir", str(create_state), "--config", str(bad_tmux_config), "--json")
        assert invalid_config.returncode == 2 and "vm.guest_tmux" in invalid_config.stderr
        invalid_profile = run(str(BINARY), "vm", "create", "bad profile", "--state-dir", str(create_state), "--profile", str(bad_profile), "--json")
        assert invalid_profile.returncode == 1 and "invalid repo profile" in invalid_profile.stderr
        created = run(str(BINARY), "vm", "create", "Trade Feature", "--repo", str(local_source), "--project", "trade", "--state-dir", str(create_state), "--work-root", str(work_root), "--config", str(argos_config), "--profile", str(profile), "--json", env=home_env)
        assert created.returncode == 0, created.stderr
        value = json.loads(created.stdout)
        assert value["dry_run"] is False
        assert Path(value["state_file"]).is_file()
        microvm_text = Path(value["microvm_config"]).read_text()
        assert microvm_text.count("ARGOS_VM_READY") == 1
        assert "services.openssh" in microvm_text
        assert 'nix.settings.experimental-features = [ "nix-command" "flakes" ];' in microvm_text
        assert 'path = "/var/lib/argos/ssh/ssh_host_ed25519_key"' in microvm_text
        assert 'directory = /workspace/repo' in microvm_text
        assert 'tag = "host-ssh";' in microvm_text
        assert f'source = "{fake_home}/.ssh";' in microvm_text
        assert 'mountPoint = "/root/.ssh";' in microvm_text
        assert 'Argos Tester' in microvm_text
        assert "forwardPorts" in microvm_text
        assert "mem = 4096" in microvm_text
        assert 'writableStoreOverlay = "/nix/.rw-store"' in microvm_text
        assert 'image = "nix-store-overlay.img"' in microvm_text
        assert 'mountPoint = "/workspace"' in microvm_text
        assert 'proto = "virtiofs";' in microvm_text
        assert 'tag = "workspace";' in microvm_text
        assert 'socket = ' in microvm_text and 'workspace-virtiofs.sock' in microvm_text
        assert f'source = "{work_root}/trade-feature"' in microvm_text
        assert 'environment.systemPackages = with pkgs; [ git tmux openssh go just ];' in microvm_text
        assert 'networking.firewall.allowedTCPPorts = [ 22 3001 5173 ];' in microvm_text
        assert 'guest.port = 3001;' in microvm_text
        assert 'guest.port = 5173;' in microvm_text
        assert 'environment.etc."argos/tmux.conf".source = ./guest-tmux.conf' in microvm_text
        assert (Path(value["microvm_config"]).parent / "guest-tmux.conf").read_text() == tmux_conf.read_text()
        assert value["record"]["source_repo"] == str(source)
        assert value["record"]["profile_path"] == str(profile)
        assert value["record"]["guest_workdir"] == "/workspace/repo"
        assert value["record"]["packages"] == ["go", "just"]
        assert [port["name"] for port in value["record"]["ports"]] == ["api", "web"]
        assert [port["guest"] for port in value["record"]["ports"]] == [3001, 5173]
        assert len({port["host"] for port in value["record"]["ports"]}) == 2
        api_host = next(port["host"] for port in value["record"]["ports"] if port["name"] == "api")
        assert value["record"]["ssh_host"] == "127.0.0.1"
        assert value["record"]["ssh_user"] == "root"
        assert isinstance(value["record"]["ssh_port"], int)
        assert (Path(value["microvm_config"]).parent / "flake.nix").is_file()
        assert (work_root / "trade-feature" / "repo" / ".git").is_dir()
        origin = run("git", "-C", str(work_root / "trade-feature" / "repo"), "remote", "get-url", "origin")
        assert origin.returncode == 0 and origin.stdout.strip() == str(source)
        dry_existing_update = run(str(BINARY), "vm", "update", "trade-feature", "--state-dir", str(create_state), "--dry-run", "--json")
        assert dry_existing_update.returncode == 0, dry_existing_update.stderr
        assert json.loads(dry_existing_update.stdout)["profile_source"] == str(profile)
        repo_profile = work_root / "trade-feature" / "repo" / ".argos.toml"
        repo_profile.write_text('''[workspace]
workdir = "/workspace/repo"

[vm]
packages = ["jq"]

[[ports]]
name = "api"
guest = 3001

[[ports]]
name = "admin"
guest = 9090
''')
        dry_update = run(str(BINARY), "vm", "update", "trade-feature", "--state-dir", str(create_state), "--dry-run", "--json", env=home_env)
        assert dry_update.returncode == 0, dry_update.stderr
        dry_update_value = json.loads(dry_update.stdout)
        assert dry_update_value["dry_run"] is True
        assert dry_update_value["profile_source"] == str(repo_profile)
        assert dry_update_value["record"]["packages"] == ["jq"]
        assert json.loads((create_state / "vms" / "trade-feature.json").read_text())["packages"] == ["go", "just"]
        updated = run(str(BINARY), "vm", "update", "trade-feature", "--state-dir", str(create_state), "--json", env=home_env)
        assert updated.returncode == 0, updated.stderr
        updated_value = json.loads(updated.stdout)
        assert updated_value["dry_run"] is False
        assert updated_value["profile_source"] == str(repo_profile)
        assert updated_value["record"]["profile_path"] == str(repo_profile)
        assert updated_value["record"]["packages"] == ["jq"]
        assert [port["name"] for port in updated_value["record"]["ports"]] == ["api", "admin"]
        assert next(port["host"] for port in updated_value["record"]["ports"] if port["name"] == "api") == api_host
        assert next(port["host"] for port in updated_value["record"]["ports"] if port["name"] == "admin") != api_host
        microvm_text = Path(value["microvm_config"]).read_text()
        assert 'environment.systemPackages = with pkgs; [ git tmux openssh jq ];' in microvm_text
        assert 'guest.port = 9090;' in microvm_text and 'guest.port = 5173;' not in microvm_text
        listed = json.loads(run(str(BINARY), "vm", "list", "--state-dir", str(create_state), "--json").stdout)
        assert [vm["id"] for vm in listed["vms"]] == ["trade-feature"]
        assert listed["vms"][0]["status"] == "created" and listed["vms"][0]["packages"] == ["jq"]
        show_json = run(str(BINARY), "vm", "show", "trade-feature", "--state-dir", str(create_state), "--json")
        assert show_json.returncode == 0, show_json.stderr
        shown = json.loads(show_json.stdout)
        assert shown["record"]["ports"] == updated_value["record"]["ports"]
        show_human = run(str(BINARY), "vm", "show", "trade-feature", "--state-dir", str(create_state))
        assert show_human.returncode == 0, show_human.stderr
        assert "ports:" in show_human.stdout and "api: guest:3001 -> host:http://127.0.0.1:" in show_human.stdout
        second = run(str(BINARY), "vm", "create", "Trade Feature Two", "--repo", str(source), "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--json")
        assert second.returncode == 0, second.stderr
        second_value = json.loads(second.stdout)
        assert {p["guest"] for p in second_value["record"]["ports"]} == {3001, 5173}
        assert {p["host"] for p in second_value["record"]["ports"]}.isdisjoint({p["host"] for p in updated_value["record"]["ports"]})
        duplicate = run(str(BINARY), "vm", "create", "Trade Feature", "--state-dir", str(create_state), "--json")
        assert duplicate.returncode == 1 and "already exists" in duplicate.stderr
        bad_args = run(str(BINARY), "vm", "create", "bad", "--id", "bad/slash", "--state-dir", str(create_state), "--json")
        assert bad_args.returncode == 2
        non_tty_up = run(str(BINARY), "vm", "up", "Needs Terminal", "--state-dir", str(create_state))
        assert non_tty_up.returncode == 1 and "requires a terminal" in non_tty_up.stderr
        assert not (create_state / "vms" / "needs-terminal.json").exists()

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
run.write_text('''#!/bin/sh
id=$(basename "$PWD")
echo "ARGOS_VM_READY id=$id host=fake"
exec sleep 60
''')
run.chmod(run.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
if out.exists() or out.is_symlink():
    out.unlink()
out.symlink_to(runner)
""")
        fake_nix.chmod(fake_nix.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        fake_env = os.environ.copy()
        fake_env["PATH"] = f"{fake_bin}:{fake_env['PATH']}"
        run("tmux", "kill-session", "-t", "argos-vm-trade-feature")
        started = run(str(BINARY), "vm", "start", "trade-feature", "--state-dir", str(create_state), "--config", str(inline_config), "--json", env=fake_env)
        assert started.returncode == 0, started.stderr
        start_value = json.loads(started.stdout)
        assert start_value["already_running"] is False
        assert start_value["record"]["status"] == "running"
        assert start_value["record"]["pid"] == start_value["pid"]
        assert start_value["console_session"] == "argos-vm-trade-feature"
        assert start_value["record"]["console_session"] == "argos-vm-trade-feature"
        assert Path(start_value["log_path"]).read_text().count("ARGOS_VM_READY id=trade-feature") == 1
        logs = run(str(BINARY), "vm", "logs", "trade-feature", "--state-dir", str(create_state), "--lines", "5")
        assert logs.returncode == 0, logs.stderr
        assert "ARGOS_VM_READY id=trade-feature" in logs.stdout
        assert (Path(value["microvm_config"]).parent / "guest-tmux.conf").read_text() == "set -g status-right inline"
        assert run("tmux", "has-session", "-t", "argos-vm-trade-feature").returncode == 0
        shell = run(str(BINARY), "vm", "shell", "trade-feature", "--state-dir", str(create_state))
        assert shell.returncode == 1 and "requires a terminal" in shell.stderr
        guest_tmux = run(str(BINARY), "vm", "tmux", "trade-feature", "--state-dir", str(create_state))
        assert guest_tmux.returncode == 1 and "requires a terminal" in guest_tmux.stderr
        again = run(str(BINARY), "vm", "start", "trade-feature", "--state-dir", str(create_state), "--config", str(inline_config), "--json", env=fake_env)
        assert again.returncode == 0, again.stderr
        assert json.loads(again.stdout)["already_running"] is True
        up_existing = run(str(BINARY), "vm", "up", "Trade Feature", "--state-dir", str(create_state), "--config", str(inline_config), "--json", env=fake_env)
        assert up_existing.returncode == 0, up_existing.stderr
        up_existing_value = json.loads(up_existing.stdout)
        assert up_existing_value["created"] is False
        assert up_existing_value["start"]["already_running"] is True
        stopped = run(str(BINARY), "vm", "stop", "trade-feature", "--state-dir", str(create_state), "--json")
        assert stopped.returncode == 0, stopped.stderr
        stop_value = json.loads(stopped.stdout)
        assert stop_value["already_stopped"] is False
        assert stop_value["record"]["status"] == "stopped"
        assert stop_value["record"]["pid"] is None
        assert run("tmux", "has-session", "-t", "argos-vm-trade-feature").returncode != 0
        stopped_again = run(str(BINARY), "vm", "stop", "trade-feature", "--state-dir", str(create_state), "--json")
        assert stopped_again.returncode == 0, stopped_again.stderr
        assert json.loads(stopped_again.stdout)["already_stopped"] is True
        refused_rm = run(str(BINARY), "vm", "rm", "trade-feature", "--state-dir", str(create_state))
        assert refused_rm.returncode == 2 and "--force" in refused_rm.stderr
        dry_rm = run(str(BINARY), "vm", "rm", "trade-feature", "--state-dir", str(create_state), "--dry-run", "--json")
        assert dry_rm.returncode == 0, dry_rm.stderr
        dry_rm_value = json.loads(dry_rm.stdout)
        assert dry_rm_value["dry_run"] is True and dry_rm_value["removed"] is False
        assert (create_state / "vms" / "trade-feature.json").is_file()
        assert (work_root / "trade-feature").is_dir()
        removed = run(str(BINARY), "vm", "rm", "trade-feature", "--state-dir", str(create_state), "--force", "--json")
        assert removed.returncode == 0, removed.stderr
        removed_value = json.loads(removed.stdout)
        assert removed_value["removed"] is True and removed_value["stopped"]["already_stopped"] is True
        assert not (create_state / "vms" / "trade-feature.json").exists()
        assert not Path(value["instance_dir"]).exists()
        assert not (work_root / "trade-feature").exists()
        removed_show = run(str(BINARY), "vm", "show", "trade-feature", "--state-dir", str(create_state), "--json")
        assert removed_show.returncode == 1 and "VM state does not exist" in removed_show.stderr

        up_new = run(str(BINARY), "vm", "up", "Up Feature", "--repo", str(source), "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--config", str(inline_config), "--json", env=fake_env)
        assert up_new.returncode == 0, up_new.stderr
        up_new_value = json.loads(up_new.stdout)
        assert up_new_value["created"] is True
        assert up_new_value["start"]["record"]["id"] == "up-feature"
        assert up_new_value["start"]["record"]["status"] == "running"
        assert (work_root / "up-feature" / "repo" / ".git").is_dir()
        up_human = run(str(BINARY), "vm", "up", "Up Feature", "--state-dir", str(create_state), "--config", str(inline_config), "--no-attach", env=fake_env)
        assert up_human.returncode == 0, up_human.stderr
        assert "already running" in up_human.stdout and "tmux: argos vm tmux up-feature" in up_human.stdout
        stopped_up = run(str(BINARY), "vm", "stop", "up-feature", "--state-dir", str(create_state), "--json")
        assert stopped_up.returncode == 0, stopped_up.stderr

        task_created = run(str(BINARY), "task", "create", "Fix Indexer", "--repo", str(local_source), "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--json")
        assert task_created.returncode == 0, task_created.stderr
        task_value = json.loads(task_created.stdout)
        assert task_value["record"]["id"] == "local-source-fix-indexer"
        assert task_value["record"]["name"] == "local-source: Fix Indexer"
        assert task_value["record"]["project"] == "local-source"
        assert (work_root / "local-source-fix-indexer" / "repo" / ".git").is_dir()
        current_repo_task = run(str(BINARY), "task", "create", "Current Repo", "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--dry-run", "--json", cwd=local_source)
        assert current_repo_task.returncode == 0, current_repo_task.stderr
        current_repo_value = json.loads(current_repo_task.stdout)
        assert current_repo_value["record"]["id"] == "local-source-current-repo"
        assert current_repo_value["record"]["project"] == "local-source"
        assert current_repo_value["record"]["source_repo"] == str(source)
        missing_repo = run(str(BINARY), "task", "create", "No Repo", "--state-dir", str(create_state), "--dry-run", cwd=root)
        assert missing_repo.returncode == 2 and "--repo is required" in missing_repo.stderr
        task_human = run(str(BINARY), "task", "create", "Custom", "--repo", str(local_source), "--project", "qvattro", "--id", "qvattro-custom", "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--dry-run")
        assert task_human.returncode == 0, task_human.stderr
        assert 'would create "qvattro: Custom"' in task_human.stdout
        assert "ports:" in task_human.stdout and "guest:3001 -> host:http://127.0.0.1:" in task_human.stdout
        task_up = run(str(BINARY), "task", "up", "Ship UI", "--repo", str(local_source), "--project", "qvattro", "--state-dir", str(create_state), "--work-root", str(work_root), "--profile", str(profile), "--config", str(inline_config), "--json", env=fake_env)
        assert task_up.returncode == 0, task_up.stderr
        task_up_value = json.loads(task_up.stdout)
        assert task_up_value["created"] is True
        assert task_up_value["start"]["record"]["id"] == "qvattro-ship-ui"
        assert task_up_value["start"]["record"]["project"] == "qvattro"
        assert task_up_value["start"]["record"]["status"] == "running"
        stopped_task = run(str(BINARY), "vm", "stop", "qvattro-ship-ui", "--state-dir", str(create_state), "--json")
        assert stopped_task.returncode == 0, stopped_task.stderr

        (vms / "bad.json").write_text(json.dumps(record("bad/slash")))
        bad = run(str(BINARY), "vm", "list", "--state-dir", str(state), "--json")
        assert bad.returncode == 1 and "invalid_state" in bad.stderr
    print("PASS: VM create/update/start/logs/shell/tmux/stop/rm/list manages deterministic local state, guest SSH metadata, guest tmux config injection, tmux process lifecycle, clone/config scaffolds, malformed state, fake local runner lifecycle, and no remote hosts.")


if __name__ == "__main__":
    main()
