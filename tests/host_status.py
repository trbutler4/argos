#!/usr/bin/env python3
"""Host-installed helper interface checks. Read-only and local only."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def run(*args, env=None):
    return subprocess.run(args, env=env, text=True, capture_output=True, timeout=10)


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    env = dict(os.environ)
    with tempfile.TemporaryDirectory(prefix="argos-host-") as temp:
        root = Path(temp)
        env["ARGOS_STATE_DIR"] = str(root / "state")
        env["ARGOS_RUNTIME_DIR"] = str(root / "runtime")
        help_result = run(str(BINARY), "host", "status", "--help", env=env)
        assert help_result.returncode == 0 and "--json" in help_result.stdout
        human = run(str(BINARY), "host", "status", env=env)
        assert human.returncode == 0, human.stderr
        assert "state: " + str(root / "state") in human.stdout
        assert "runtime: " + str(root / "runtime") in human.stdout
        result = run(str(BINARY), "host", "status", "--json", env=env)
        assert result.returncode == 0, result.stderr
        status = json.loads(result.stdout)
        assert status["schema_version"] == 1
        assert status["argos_version"]
        assert status["state_root"] == str(root / "state")
        assert status["runtime_root"] == str(root / "runtime")
        caps = status["capabilities"]
        for key in ("kvm_device_exists", "kvm_accessible", "nix_available", "tmux_available", "ssh_available", "can_run_microvms"):
            assert isinstance(caps[key], bool), (key, caps)
        assert caps["can_run_microvms"] == all(caps[key] for key in ("kvm_accessible", "nix_available", "tmux_available", "ssh_available"))

        bad_env = dict(env, ARGOS_STATE_DIR="relative", ARGOS_RUNTIME_DIR="relative")
        fallback = json.loads(run(str(BINARY), "host", "status", "--json", env=bad_env).stdout)
        assert fallback["state_root"].endswith("/.local/state/argos"), fallback
        assert fallback["runtime_root"].endswith("/argos"), fallback
    print("PASS: host status helper reports stable JSON, paths, tools and microvm capability.")


if __name__ == "__main__":
    main()
