#!/usr/bin/env python3
"""Exercise host Git/SSH sharing policy through create and update without booting VMs."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def run(*args, env):
    return subprocess.run(args, env=env, text=True, capture_output=True, timeout=15)


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    with tempfile.TemporaryDirectory(prefix="argos-credential-sharing-") as temp:
        root = Path(temp)
        home = root / "home"
        (home / ".ssh").mkdir(parents=True)
        (home / ".ssh" / "id_ed25519.pub").write_text("ssh-ed25519 AAAA disposable\n")
        (home / ".gitconfig").write_text("[user]\n\tname = Disposable\n")
        env = os.environ.copy()
        env["HOME"] = str(home)
        env["XDG_CONFIG_HOME"] = str(root / "xdg")
        state = root / "state"
        work = root / "work"
        config = root / "argos.toml"
        base = """schema_version = 1
[client]
machine_id = "local"
[machines.local]
"""
        config.write_text(base)

        created = run(
            str(BINARY), "vm", "create", "credential-test", "--state-dir", str(state),
            "--work-root", str(work), "--config", str(config), "--json", env=env,
        )
        assert created.returncode == 0, created.stderr
        value = json.loads(created.stdout)
        assert value["record"]["share_host_git"] is True
        microvm = Path(value["microvm_config"]).read_text()
        assert 'tag = "host-ssh";' in microvm
        assert "Disposable" in microvm

        config.write_text(base + "\n[vm.defaults]\nshare_host_git = false\n")
        updated = run(
            str(BINARY), "vm", "update", "credential-test", "--state-dir", str(state),
            "--config", str(config), "--json", env=env,
        )
        assert updated.returncode == 0, updated.stderr
        result = json.loads(updated.stdout)
        assert result["previous_record"]["share_host_git"] is True
        assert result["record"]["share_host_git"] is False
        persisted = json.loads((state / "vms" / "credential-test.json").read_text())
        assert persisted["share_host_git"] is False
        microvm = Path(result["record"]["microvm_config"]).read_text()
        assert 'tag = "host-ssh";' not in microvm
        assert "Disposable" not in microvm

        # An update without an explicit policy preserves the persisted opt-out.
        config.write_text(base)
        preserved = run(
            str(BINARY), "vm", "update", "credential-test", "--state-dir", str(state),
            "--config", str(config), "--json", env=env,
        )
        assert preserved.returncode == 0, preserved.stderr
        assert json.loads(preserved.stdout)["record"]["share_host_git"] is False

    print("credential sharing checks passed")


if __name__ == "__main__":
    main()
