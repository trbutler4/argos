#!/usr/bin/env python3
"""Real loopback SSH integration for argos. Uses only disposable user-owned state."""
import json
import os
from pathlib import Path
import pwd
import shutil
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos"))


def run(*args, env=None, timeout=15):
    return subprocess.run(args, text=True, capture_output=True, env=env, timeout=timeout)


def require_tools():
    for tool in ("sshd", "ssh", "ssh-keygen", "tmux"):
        assert shutil.which(tool), f"missing {tool}; run inside nix develop"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def cli(*args, code=0, env=None, timeout=15):
    result = run(str(BINARY), *args, env=env, timeout=timeout)
    assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
    return result


def snapshot(config, *extra, code=0, env=None, timeout=15):
    result = cli("list", "--config", str(config), "--json", *extra, code=code, env=env, timeout=timeout)
    value = json.loads(result.stdout)
    assert value["schema_version"] == 1
    assert isinstance(value["hosts"], list)
    return value


def main():
    assert BINARY.is_file(), "Run cargo build --locked first (this test targets the unwrapped debug binary)"
    require_tools()
    base = os.environ.get("XDG_RUNTIME_DIR")
    with tempfile.TemporaryDirectory(prefix="argos-ssh-", dir=base) as directory:
        root = Path(directory)
        socket_path = root / "tmux socket | unicode α ' \" trailing "
        server_key = root / "server_ed25519"
        client_key = root / "client_ed25519"
        wrong_key = root / "wrong_ed25519"
        authorized = root / "authorized_keys"
        known_hosts = root / "known_hosts"
        bad_known_hosts = root / "empty_known_hosts"
        ssh_config = root / "ssh_config"
        sshd_config = root / "sshd_config"
        port = free_port()
        user = pwd.getpwuid(os.getuid()).pw_name
        env = dict(os.environ, PATH=str(root / "ssh-wrapper") + os.pathsep + os.environ["PATH"])
        (root / "ssh-wrapper").mkdir()
        actual_ssh = Path(shutil.which("ssh")).resolve()
        wrapper = root / "ssh-wrapper" / "ssh"
        wrapper.write_text(f"#!/bin/sh\nexec {actual_ssh} -F {ssh_config} \"$@\"\n")
        wrapper.chmod(0o755)

        for key in (server_key, client_key, wrong_key):
            result = run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
            assert result.returncode == 0, result.stderr
        authorized.write_text((Path(f"{client_key}.pub")).read_text())
        host_public = Path(f"{server_key}.pub").read_text().strip()
        known_hosts.write_text(f"[127.0.0.1]:{port} {host_public.split(None, 2)[0]} {host_public.split(None, 2)[1]}\n")
        bad_known_hosts.write_text("")
        ssh_config.write_text(
            f"Host test-peer\n HostName 127.0.0.1\n Port {port}\n User {user}\n"
            f" IdentityFile {client_key}\n IdentitiesOnly yes\n UserKnownHostsFile {known_hosts}\n"
            " StrictHostKeyChecking yes\n BatchMode yes\n"
        )
        sshd_config.write_text(
            f"HostKey {server_key}\nAuthorizedKeysFile {authorized}\nUsePAM no\nStrictModes no\n"
            f"PasswordAuthentication no\nAuthenticationMethods publickey\nListenAddress 127.0.0.1\nPort {port}\n"
            "LogLevel ERROR\nPidFile none\n"
        )
        (root / "config.toml").write_text(
            f"schema_version = 1\n[client]\nmachine_id = 'local'\nconnect_timeout_seconds = 2\n"
            "max_parallel_probes = 4\n[machines.local]\n"
            f"socket = {json.dumps(str(socket_path))}\n[machines.remote]\nssh_alias = 'test-peer'\n"
            f"socket = {json.dumps(str(socket_path))}\n"
        )
        tmux_args = ["tmux", "-u", "-S", str(socket_path), "-f", "/dev/null"]
        def tmux(*args):
            result = run(*tmux_args, *args)
            assert result.returncode == 0, (args, result.stdout, result.stderr)
            return result.stdout

        sshd_path = shutil.which("sshd")
        assert sshd_path is not None
        sshd = subprocess.Popen([sshd_path, "-D", "-e", "-f", str(sshd_config)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(20):
                if sshd.poll() is not None:
                    details = sshd.stderr.read() if sshd.stderr else ""
                    raise AssertionError(f"sshd exited during startup: {details}")
                probe = run(str(actual_ssh), "-F", str(ssh_config), "-o", "ConnectTimeout=1", "-o", "StrictHostKeyChecking=yes", "test-peer", "true", timeout=3)
                if probe.returncode == 0:
                    break
                time.sleep(0.1)
            else:
                if sshd.poll() is None:
                    sshd.terminate()
                    sshd.wait(timeout=3)
                details = sshd.stderr.read() if sshd.stderr else ""
                raise AssertionError(f"sshd failed to start: {details}")
            ids = [tmux("new-session", "-d", "-P", "-F", "#{session_id}", "-s", name, "sleep 300").strip() for name in ("quotes ' \" ; $(literal) trailing ", "unicode α | pipe")]
            before = tmux("list-panes", "-a", "-F", "#{pane_id}:#{pane_pid}")
            started = time.monotonic()
            healthy = snapshot(root / "config.toml", env=env)
            assert time.monotonic() - started <= 5
            assert {h["id"] for h in healthy["hosts"]} == {"local", "remote"}
            assert all(h["status"] == "ok" and h["error"] is None for h in healthy["hosts"])
            for host in healthy["hosts"]:
                assert [s["id"] for s in host["sessions"]] == ids
                assert [s["name"] for s in host["sessions"]] == ["quotes ' \" ; $(literal) trailing ", "unicode α | pipe"]
            assert before == tmux("list-panes", "-a", "-F", "#{pane_id}:#{pane_pid}")
            assert len(snapshot(root / "config.toml", "--host", "remote", env=env)["hosts"]) == 1

            # Remote missing/invalid sockets must not be confused with SSH failures.
            original_inventory = (root / "config.toml").read_text()
            (root / "config.toml").write_text(original_inventory.replace(json.dumps(str(socket_path)), json.dumps(str(root / "missing.sock"))))
            empty = snapshot(root / "config.toml", "--host", "remote", env=env)
            assert empty["hosts"][0]["status"] == "ok" and empty["hosts"][0]["sessions"] == []
            regular = root / "regular-file"
            regular.write_text("not a socket")
            (root / "config.toml").write_text(original_inventory.replace(json.dumps(str(socket_path)), json.dumps(str(regular))))
            invalid = snapshot(root / "config.toml", "--host", "remote", code=1, env=env)
            assert invalid["hosts"][0]["error"]["code"] == "invalid_socket"
            (root / "config.toml").write_text(original_inventory)

            # Host-key, authentication, and closed-port failures are real SSH failures.
            ssh_config.write_text(ssh_config.read_text().replace(str(known_hosts), str(bad_known_hosts)))
            partial = snapshot(root / "config.toml", code=1, env=env)
            assert any(h["id"] == "local" and h["status"] == "ok" and h["sessions"] for h in partial["hosts"])
            assert any(h["id"] == "remote" and h["status"] == "error" and h["error"]["code"] == "host_key_error" for h in partial["hosts"])
            ssh_config.write_text(ssh_config.read_text().replace(str(bad_known_hosts), str(known_hosts)).replace(str(client_key), str(wrong_key)))
            auth = snapshot(root / "config.toml", "--host", "remote", code=1, env=env)
            assert auth["hosts"][0]["error"]["code"] == "authentication_failed"
            ssh_config.write_text(ssh_config.read_text().replace(f"Port {port}", f"Port {free_port()}"))
            unreachable = snapshot(root / "config.toml", "--host", "remote", code=1, env=env)
            assert unreachable["hosts"][0]["error"]["code"] == "unreachable"
        finally:
            run(*tmux_args, "kill-server")
            sshd.terminate()
            try:
                sshd.wait(timeout=3)
            except subprocess.TimeoutExpired:
                sshd.kill(); sshd.wait()
    print("PASS: real loopback sshd, SSH host-key/auth errors, partial JSON, filtering, and isolated tmux")


if __name__ == "__main__":
    main()
