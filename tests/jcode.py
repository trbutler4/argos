#!/usr/bin/env python3
"""Passive Jcode probes: fake processes, real loopback SSH, and picker PTY."""
import fcntl
import json
import os
from pathlib import Path
import pty
import pwd
import select
import shlex
import shutil
import socket
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()
REMOTE = "command -v jcode >/dev/null 2>&1 || exit 127; exec jcode --no-update debug sessions"
SESSION = {"session_id": "fixture-session", "friendly_name": "Test Agent\u001b[2J\nlabel", "provider": "fixture", "model": "test-model", "is_processing": None, "working_dir": "/workspace/repo", "swarm_id": None, "status": "busy", "detail": None, "token_usage": None, "future_field": True}


def run(*args, env=None, timeout=12):
    return subprocess.run(args, env=env, text=True, capture_output=True, timeout=timeout)


def save_vm(state, vm_id, **extra):
    (state / "vms").mkdir(parents=True, exist_ok=True)
    value = dict(schema_version=1, id=vm_id, name="Jcode Test", status="running", host="fixture", ssh_host="127.0.0.1", ssh_user="root", ssh_port=2222)
    value.update(extra)
    (state / "vms" / f"{vm_id}.json").write_text(json.dumps(value))
    trust = state / "instances" / vm_id / "known_hosts"
    trust.parent.mkdir(parents=True, exist_ok=True)
    trust.write_text("fixture trust\n")
    return trust


def agents(state, env, code=0):
    result = run(str(BINARY), "agents", "list", "--state-dir", str(state), "--json", env=env)
    assert result.returncode == code, (result.returncode, result.stdout, result.stderr)
    data = json.loads(result.stdout)
    assert data["schema_version"] == 1
    assert data["scope"] == "client_connected_debug_sessions"
    return {g["vm_id"]: g for g in data["vms"]}


def fake_checks(root):
    tools = root / "fake-bin"
    tools.mkdir()
    mode = root / "mode"
    mode.write_text("ok")
    log = root / "calls"
    fake = tools / "ssh"
    fake.write_text(f'''#!{shutil.which("python3")}
import json, pathlib, sys, time
args = sys.argv[1:]
assert args[-1] == {REMOTE!r}, args
for option in ["StrictHostKeyChecking=yes", "GlobalKnownHostsFile=/dev/null", "ForwardAgent=no", "UpdateHostKeys=no", "ProxyCommand=none", "ProxyJump=none", "ControlPath=none"]:
    assert option in args, args
assert args[:2] == ["-F", "/dev/null"], args
with open({str(log)!r}, "a") as out: out.write(json.dumps(args) + "\\n")
mode = pathlib.Path({str(mode)!r}).read_text()
port = args[args.index("-p") + 1]
if port == "2223": mode = "disabled"
if mode == "slow": time.sleep(30)
if mode == "flood": print("x" * (5 * 1024 * 1024)); sys.exit(0)
if mode == "disabled": print("Error: Debug control is disabled.", file=sys.stderr); sys.exit(1)
if mode == "missing": sys.exit(127)
if mode == "hostkey": print("Host key verification failed.", file=sys.stderr); sys.exit(255)
if mode == "unreachable": print("Connection refused", file=sys.stderr); sys.exit(255)
if mode == "invalid": print("not JSON"); sys.exit(0)
print("[]" if mode == "empty" else {json.dumps([SESSION])!r})
''')
    fake.chmod(0o755)
    env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"], TERM="xterm-256color")
    state = root / "state"
    trust = save_vm(state, "healthy")
    save_vm(state, "disabled", ssh_port=2223)
    save_vm(state, "stopped", status="stopped")
    unknown = save_vm(state, "untrusted")
    unknown.unlink()
    before = trust.read_bytes()
    data = agents(state, env, code=1)
    assert len(data["healthy"]["sessions"]) == 1
    s = data["healthy"]["sessions"][0]
    assert s["is_processing"] is None and s["status"] == "busy"
    assert "\u001b" not in s["friendly_name"] and "\n" not in s["friendly_name"]
    assert data["disabled"]["error"]["code"] == "debug_disabled"
    assert data["stopped"]["status"] == "stopped"
    assert data["untrusted"]["error"]["code"] == "host_key_untrusted"
    assert len(log.read_text().splitlines()) == 2, "stopped/untrusted guests were probed"
    assert trust.read_bytes() == before and not unknown.exists(), "passive discovery changed trust"
    for name in ("disabled", "stopped", "untrusted"):
        (state / "vms" / f"{name}.json").unlink()
    for value, expected in [("empty", None), ("missing", "jcode_missing"), ("hostkey", "host_key_untrusted"), ("unreachable", "unreachable"), ("invalid", "invalid_output"), ("flood", "output_limit"), ("slow", "timeout")]:
        mode.write_text(value)
        started = time.monotonic()
        data = agents(state, env, code=int(expected is not None))["healthy"]
        assert time.monotonic() - started < 5
        assert (data["error"]["code"] if data["error"] else None) == expected, data
        if value == "empty": assert data["sessions"] == [] and data["status"] == "ok"
    # Four probes in flight, not an unbounded process per VM or serial timeout.
    for index in range(7): save_vm(state, f"parallel-{index}")
    started = time.monotonic()
    data = agents(state, env, code=1)
    assert len(data) == 8 and time.monotonic() - started < 8
    for path in (state / "vms").glob("parallel-*.json"): path.unlink()
    mode.write_text("ok")
    pty_checks(root, state, env, mode, log)


def pty_checks(root, state, env, mode, log):
    for key in ("TMUX", "TMUX_PANE", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"):
        env.pop(key, None)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 160, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen([str(BINARY), "tui", "--socket", str(root / "absent-tmux.sock"), "--state-dir", str(state)], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True, preexec_fn=lambda: fcntl.ioctl(slave, termios.TIOCSCTTY, 0))
    output = bytearray()
    def drain():
        while select.select([master], [], [], 0)[0]:
            output.extend(os.read(master, 65536))
    def wait_for(token):
        end = time.monotonic() + 7
        while time.monotonic() < end:
            drain()
            if token in output.decode(errors="replace"): return
            time.sleep(0.03)
        raise AssertionError((token, output.decode(errors="replace")))
    try:
        wait_for("test-model")
        wait_for("READY")
        mode.write_text("disabled")
        output.clear()
        os.write(master, b"r")
        wait_for("debug_disabled")
        wait_for("STALE")
        # Healthy session details are retained while the failed probe is visible.
        wait_for("test-model")
        mode.write_text("slow")
        output.clear()
        calls = len(log.read_text().splitlines())
        os.write(master, b"rrrrrrrr")
        wait_for("refreshing")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 60, 0, 0))
        time.sleep(0.2)
        drain()
        started = time.monotonic()
        os.write(master, b"q")
        process.wait(timeout=2)
        assert time.monotonic() - started < 1.5, "quit stalled on guest probe"
        assert len(log.read_text().splitlines()) <= calls + 1, "refreshes did not coalesce"
        assert process.returncode == 0
        assert termios.tcgetattr(slave) == original, "PTY modes not restored"
    finally:
        if process.poll() is None: process.kill(); process.wait()
        os.close(master)
        os.close(slave)


def real_ssh_checks(root):
    for tool in ("ssh", "sshd", "ssh-keygen"):
        assert shutil.which(tool), f"missing {tool}, use nix develop"
    root.mkdir()
    server_key, client_key, wrong_key = [root / name for name in ("server", "client", "wrong")]
    for key in (server_key, client_key, wrong_key):
        result = run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
        assert result.returncode == 0, result.stderr
    authorized = root / "authorized"
    authorized.write_text(client_key.with_suffix(".pub").read_text())
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    remote_bin = root / "remote-bin"
    remote_bin.mkdir()
    jcode = remote_bin / "jcode"
    jcode.write_text(f"#!/bin/sh\n[ \"$*\" = '--no-update debug sessions' ] || exit 91\nprintf '%s\\n' {shlex.quote(json.dumps([SESSION]))}\n")
    jcode.chmod(0o755)
    force = root / "force"
    force.write_text(f"#!/bin/sh\nexport PATH={shlex.quote(str(remote_bin))}\nexec /bin/sh -c \"$SSH_ORIGINAL_COMMAND\"\n")
    force.chmod(0o755)
    config = root / "sshd_config"
    config.write_text(f"HostKey {server_key}\nAuthorizedKeysFile {authorized}\nUsePAM no\nStrictModes no\nPasswordAuthentication no\nAuthenticationMethods publickey\nListenAddress 127.0.0.1\nPort {port}\nLogLevel ERROR\nPidFile none\nForceCommand {force}\n")
    wrappers = root / "bin"
    wrappers.mkdir()
    wrapper = wrappers / "ssh"
    wrapper.write_text(f"#!/bin/sh\nexec {shlex.quote(shutil.which('ssh'))} -i {shlex.quote(str(client_key))} -o IdentitiesOnly=yes \"$@\"\n")
    wrapper.chmod(0o755)
    env = dict(os.environ, PATH=str(wrappers) + os.pathsep + os.environ["PATH"])
    state = root / "state"
    trust = save_vm(state, "real", ssh_port=port, ssh_user=pwd.getpwuid(os.getuid()).pw_name)
    public = server_key.with_suffix(".pub").read_text().split()
    trust.write_text(f"[127.0.0.1]:{port} {public[0]} {public[1]}\n")
    before = trust.read_bytes()
    sshd = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(config)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        for _ in range(30):
            if sshd.poll() is not None: raise AssertionError(sshd.stderr.read().decode())
            result = run(str(BINARY), "agents", "list", "--state-dir", str(state), "--json", env=env)
            if result.returncode == 0: break
            time.sleep(0.1)
        else: raise AssertionError((result.stdout, result.stderr))
        data = json.loads(result.stdout)["vms"][0]
        assert data["status"] == "ok" and data["sessions"][0]["model"] == "test-model", data
        assert trust.read_bytes() == before
        public = wrong_key.with_suffix(".pub").read_text().split()
        trust.write_text(f"[127.0.0.1]:{port} {public[0]} {public[1]}\n")
        assert agents(state, env, code=1)["real"]["error"]["code"] == "host_key_untrusted"
        trust.write_bytes(before)
        jcode.unlink()
        assert agents(state, env, code=1)["real"]["error"]["code"] == "jcode_missing"
    finally:
        sshd.terminate()
        sshd.wait(timeout=4)
    assert agents(state, env, code=1)["real"]["error"]["code"] == "unreachable"


def main():
    with tempfile.TemporaryDirectory(prefix="argos-jcode-", dir=os.environ.get("XDG_RUNTIME_DIR")) as directory:
        root = Path(directory)
        missing = root / "missing"
        assert agents(missing, os.environ) == {} and not missing.exists()
        fake_checks(root)
        real_ssh_checks(root / "ssh-fixture")
    print("PASS: scoped Jcode JSON, partial errors, no trust mutation, bounded probes, real SSH and responsive/stale picker PTY")


if __name__ == "__main__":
    main()
