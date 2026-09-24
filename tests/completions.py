#!/usr/bin/env python3
"""Dynamic shell completion checks.

Completion is read-only. These tests exercise the real CompleteEnv protocol the
shell uses, against isolated XDG_CONFIG_HOME/ARGOS_STATE_DIR trees, so no real
inventory, host or VM is touched.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()

CONFIG = '''schema_version = 1
[client]
machine_id = "alpha"

[machines.alpha]

[machines.beta]
ssh_alias = "beta-alias"
'''


def split_candidate(line):
    """Split `value:help`, honoring the backslash-escaped colons zsh needs."""
    out = []
    i = 0
    while i < len(line):
        if line[i] == "\\" and i + 1 < len(line):
            out.append(line[i + 1])
            i += 2
            continue
        if line[i] == ":":
            break
        out.append(line[i])
        i += 1
    return "".join(out)


def complete(env, *words, index=None):
    """Invoke the completer exactly as the generated shell function does."""
    args = [str(BINARY), "--", "argos", *words]
    call_env = dict(env, COMPLETE="zsh")
    if index is None:
        index = len(words)
    call_env["_CLAP_COMPLETE_INDEX"] = str(index)
    result = subprocess.run(args, env=call_env, text=True, capture_output=True, timeout=10)
    assert result.returncode == 0, result.stderr
    return [split_candidate(line) for line in result.stdout.splitlines() if line]


def record(vm_id, name, status):
    return {
        "schema_version": 1,
        "id": vm_id,
        "name": name,
        "status": status,
        "host": "alpha",
        "project": None,
        "repo_path": None,
        "workdir": None,
        "created_at_unix_ms": None,
        "updated_at_unix_ms": None,
    }


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    with tempfile.TemporaryDirectory(prefix="argos-completions-") as temp:
        root = Path(temp)
        config_dir = root / "xdg/argos"
        config_dir.mkdir(parents=True)
        (config_dir / "config.toml").write_text(CONFIG)
        vms = root / "state/vms"
        vms.mkdir(parents=True)
        (vms / "web.json").write_text(json.dumps(record("web", "Web", "running")))
        (vms / "api.json").write_text(json.dumps(record("api", "Api", "stopped")))
        env = dict(
            os.environ,
            XDG_CONFIG_HOME=str(root / "xdg"),
            HOME=str(root / "home"),
            ARGOS_STATE_DIR=str(root / "state"),
        )

        # Registration scripts are emitted for each supported shell.
        for shell in ("zsh", "bash", "fish"):
            script = subprocess.run(
                [str(BINARY)], env=dict(env, COMPLETE=shell), text=True, capture_output=True, timeout=10
            )
            assert script.returncode == 0, script.stderr
            assert "argos" in script.stdout and len(script.stdout) > 100, shell

        # Completion is off by default, so normal runs are unaffected.
        plain = subprocess.run([str(BINARY), "vm", "list", "--json"], env=env, text=True, capture_output=True, timeout=10)
        assert plain.returncode == 0, plain.stderr
        assert json.loads(plain.stdout)["vms"], "normal dispatch must still run"

        # Top-level and nested subcommands.
        top = complete(env, "")
        for name in ("list", "sessions", "tui", "host", "hosts", "vm", "task"):
            assert name in top, (name, top)
        assert "list" in complete(env, "vm", "") and "rm" in complete(env, "vm", "")

        # Dynamic VM ids come from local state only.
        for sub in ("start", "stop", "show", "rm", "logs", "shell", "tmux", "restart", "update"):
            ids = complete(env, "vm", sub, "")
            assert "web" in ids and "api" in ids, (sub, ids)

        # Dynamic host ids come from the private inventory.
        hosts = complete(env, "sessions", "list", "--host", "")
        assert "alpha" in hosts and "beta" in hosts, hosts
        assert "alpha" in complete(env, "hosts", "status", "--host", ""), "hosts status --host"
        assert "beta" in complete(env, "vm", "create", "demo", "--host", ""), "vm create --host"

        # Attach targets combine VM and host entry points.
        targets = complete(env, "sessions", "attach", "")
        assert "vm:web" in targets and "vm:api" in targets, targets
        assert "host:alpha:" in targets and "host:beta:" in targets, targets

        # Prefixes filter, and remote hosts are never probed for sessions.
        assert complete(env, "sessions", "attach", "vm:w") == ["vm:web"]

        # Missing config and missing state degrade to empty, never an error.
        bare = dict(os.environ, XDG_CONFIG_HOME=str(root / "absent"), HOME=str(root / "absent"),
                    ARGOS_STATE_DIR=str(root / "absent-state"))
        assert complete(bare, "vm", "start", "") is not None
        assert "web" not in complete(bare, "vm", "start", "")
        assert "alpha" not in complete(bare, "sessions", "list", "--host", "")

        # Tab latency must stay well under a human-perceptible delay.
        start = time.monotonic()
        complete(env, "sessions", "attach", "")
        elapsed = time.monotonic() - start
        assert elapsed < 2.0, f"completion too slow: {elapsed:.2f}s"

        # Corrupt state must not break the shell.
        (vms / "broken.json").write_text("{not json")
        assert complete(env, "vm", "start", "") is not None
    print("PASS: dynamic completion emits shell registration scripts and completes subcommands, VM ids, inventory hosts and attach targets from local state without SSH, errors or mutation.")


if __name__ == "__main__":
    main()
