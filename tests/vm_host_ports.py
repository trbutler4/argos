#!/usr/bin/env python3
"""Real CLI create/update checks against an independently owned TCP listener.

No VM is started. All Argos records/workspaces are disposable fixtures.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def call(root, *args):
    result = subprocess.run(
        [str(BINARY), "vm", *args, "--state-dir", str(root / "state"),
         "--config", str(root / "inventory.toml"), "--json"],
        cwd=root, env={**os.environ, "XDG_CONFIG_HOME": str(root / "config")},
        capture_output=True, text=True, timeout=15)
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    return json.loads(result.stdout)


def main():
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", Path.home() / ".jcode/scratch"))
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="argos-host-ports-", dir=scratch) as directory:
        root = Path(directory)
        (root / "inventory.toml").write_text(
            'schema_version=1\n[client]\nmachine_id="fixture"\n'
            '[machines.fixture]\n[vm.defaults]\nshare_host_git=false\n')
        profile = root / "profile.toml"
        profile.write_text('[[ports]]\nname="metro"\nguest=8081\n'
                           '[[ports]]\nname="web"\nguest=8082\n')
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as outsider:
            for candidate in range(43000, 46000):
                try:
                    outsider.bind(("127.0.0.1", candidate))
                    break
                except OSError:
                    continue
            else:
                raise AssertionError("No disposable listener port available")
            outsider.listen(1)
            created = call(root, "create", "listener-check", "--id", "listener-check",
                           "--work-root", str(root / "work"), "--profile", str(profile))["record"]
            ports = {item["name"]: item["host"] for item in created["ports"]}
            assert len(set(ports.values())) == 2, ports
            assert outsider.getsockname()[1] not in ports.values(), ports
            state_file = root / "state/vms/listener-check.json"
            before = state_file.read_bytes()
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as newcomer:
                newcomer.bind(("127.0.0.1", ports["metro"]))
                newcomer.listen(1)
                planned = call(root, "update", "listener-check", "--profile", str(profile),
                               "--dry-run")["record"]
                assert state_file.read_bytes() == before, "dry-run changed VM record"
                updated = call(root, "update", "listener-check", "--profile", str(profile))["record"]
                for record in (planned, updated):
                    actual = {item["name"]: item["host"] for item in record["ports"]}
                    assert len(set(actual.values())) == 2, actual
                    assert not ({outsider.getsockname()[1], newcomer.getsockname()[1]} & set(actual.values())), actual
                    assert record["ssh_port"] == created["ssh_port"], "SSH identity endpoint changed"
                    assert record["repo_path"] == created["repo_path"]
                    assert record["started_at_unix_ms"] is None
                # Both independently owned listeners remain operational. The
                # allocator must neither stop nor take over another process.
                for listener in (outsider, newcomer):
                    with socket.create_connection(listener.getsockname(), timeout=2) as client:
                        connection, _ = listener.accept()
                        with connection:
                            client.sendall(b"retained")
                            assert connection.recv(8) == b"retained"
    print("PASS: real Argos CLI skips occupied ports, keeps mappings distinct, remaps before first boot, preserves SSH endpoint, leaves listeners unchanged and keeps dry-run read-only.")


if __name__ == "__main__":
    main()
