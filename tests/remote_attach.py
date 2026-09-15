#!/usr/bin/env python3
"""Real PTY remote attachment checks using disposable loopback sshd and tmux."""
import fcntl
import json
import os
from pathlib import Path
import pwd
import pty
import select
import shlex
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def run(*args, env=None, timeout=15):
    return subprocess.run(args, env=env, text=True, capture_output=True, timeout=timeout)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    for tool in ("sshd", "ssh", "ssh-keygen", "tmux"):
        assert shutil.which(tool), f"missing {tool}; run inside nix develop"
    assert BINARY.is_file(), "Run cargo build --locked first"
    clients = []
    with tempfile.TemporaryDirectory(prefix="argos-remote-attach-", dir=os.environ.get("XDG_RUNTIME_DIR")) as directory:
        root = Path(directory)
        wrapper_dir = root / "ssh-wrapper"
        wrapper_dir.mkdir()
        actual_ssh = Path(shutil.which("ssh")).resolve()
        ssh_config = root / "ssh_config"
        ssh_marker = root / "ssh-wrapper-used"
        wrapper = wrapper_dir / "ssh"
        wrapper.write_text(f"#!/bin/sh\nprintf . >> {ssh_marker}\nexec {actual_ssh} -F {ssh_config} \"$@\"\n")
        wrapper.chmod(0o755)
        env = dict(os.environ, TERM="xterm-256color", PATH=str(wrapper_dir) + os.pathsep + os.environ["PATH"])
        env.pop("TMUX", None)
        env.pop("TMUX_PANE", None)

        server_key = root / "server_ed25519"
        client_key = root / "client_ed25519"
        authorized = root / "authorized_keys"
        known_hosts = root / "known_hosts"
        sshd_config = root / "sshd_config"
        port = free_port()
        user = pwd.getpwuid(os.getuid()).pw_name
        for key in (server_key, client_key):
            result = run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key), env=env)
            assert result.returncode == 0, result.stderr
        authorized.write_text(Path(f"{client_key}.pub").read_text())
        host_type, host_blob, *_ = Path(f"{server_key}.pub").read_text().split()
        known_hosts.write_text(f"[127.0.0.1]:{port} {host_type} {host_blob}\n")
        ssh_config.write_text(
            f"Host test-peer\n HostName 127.0.0.1\n Port {port}\n User {user}\n"
            f" IdentityFile {client_key}\n IdentitiesOnly yes\n UserKnownHostsFile {known_hosts}\n"
            " StrictHostKeyChecking yes\n BatchMode yes\n"
        )
        sshd_config.write_text(
            f"HostKey {server_key}\nAuthorizedKeysFile {authorized}\nUsePAM no\nStrictModes no\n"
            f"PasswordAuthentication no\nAuthenticationMethods publickey\nListenAddress 127.0.0.1\nPort {port}\n"
            "LogLevel ERROR\nPidFile none\nPermitTTY yes\n"
        )
        remote_socket = root / "remote tmux.sock"
        config = root / "config.toml"
        config.write_text(
            "schema_version = 1\n[client]\nmachine_id = 'local'\nconnect_timeout_seconds = 2\n"
            "max_parallel_probes = 2\n[machines.local]\n[machines.remote]\nssh_alias = 'test-peer'\n"
            f"socket = {json.dumps(str(remote_socket))}\n"
        )
        remote_tmux = ["tmux", "-u", "-S", str(remote_socket), "-f", "/dev/null"]
        local_socket = root / "local,tmux.sock"
        local_tmux = ["tmux", "-u", "-S", str(local_socket), "-f", "/dev/null"]

        def tmux(args, *extra):
            result = run(*args, *extra, env=env)
            assert result.returncode == 0, (args, extra, result.stdout, result.stderr)
            return result.stdout.removesuffix("\n")

        def remote_clients():
            result = run(*remote_tmux, "list-clients", "-F", "#{client_tty}|#{session_name}", env=env)
            if result.returncode != 0:
                return []
            return result.stdout.splitlines()

        def wait(check, description, timeout=8):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                drain()
                if check():
                    return
                time.sleep(0.05)
            raise AssertionError(description)

        def drain():
            for process, master, slave, _ in clients:
                while select.select([master], [], [], 0)[0]:
                    try:
                        if not os.read(master, 65536):
                            break
                    except OSError:
                        break

        def terminal(args):
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
            original = termios.tcgetattr(slave)
            process = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True,
                                       preexec_fn=lambda: fcntl.ioctl(slave, termios.TIOCSCTTY, 0))
            item = (process, master, slave, original)
            clients.append(item)
            return item

        sshd_path = shutil.which("sshd")
        assert sshd_path is not None
        sshd = subprocess.Popen([sshd_path, "-D", "-e", "-f", str(sshd_config)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(20):
                if sshd.poll() is not None:
                    raise AssertionError(f"sshd exited: {sshd.stderr.read() if sshd.stderr else ''}")
                probe = run(str(actual_ssh), "-F", str(ssh_config), "-o", "ConnectTimeout=1", "test-peer", "true", env=env, timeout=3)
                if probe.returncode == 0:
                    break
                time.sleep(0.1)
            else:
                raise AssertionError("sshd did not become reachable")

            target_name = "remote α ' ; $(literal) | trailing "
            target_id = tmux(remote_tmux, "new-session", "-d", "-P", "-F", "#{session_id}", "-s", target_name, "/bin/sh")
            before = tmux(remote_tmux, "list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")

            # Plain terminal remote attach execs SSH, attaches to remote tmux, then detaches cleanly.
            plain = terminal([str(BINARY), "attach", target_name, "--config", str(config), "--host", "remote"])
            wait(lambda: any(target_name in line for line in remote_clients()), "plain terminal did not attach remotely")
            assert ssh_marker.exists(), "Argos did not invoke the configured SSH wrapper"
            os.write(plain[1], b"\x02d")
            wait(lambda: plain[0].poll() is not None, "plain remote attach did not detach")
            assert plain[0].returncode == 0
            assert termios.tcgetattr(plain[2]) == plain[3]
            wait(lambda: not remote_clients(), "plain remote client did not detach")
            assert before == tmux(remote_tmux, "list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")

            # TUI Enter exits the alternate screen process and then hands the terminal to remote tmux.
            tui_plain = terminal([str(BINARY), "tui", "--config", str(config), "--host", "remote"])
            time.sleep(0.2)
            os.write(tui_plain[1], b"\r")
            wait(lambda: any(target_name in line for line in remote_clients()), "TUI did not attach remotely")
            os.write(tui_plain[1], b"\x02d")
            wait(lambda: tui_plain[0].poll() is not None, "TUI remote attach did not detach")
            assert tui_plain[0].returncode == 0
            assert termios.tcgetattr(tui_plain[2]) == tui_plain[3]
            wait(lambda: not remote_clients(), "TUI remote client did not detach")
            assert before == tmux(remote_tmux, "list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")

            # Inside local tmux opens a connection window instead of replacing the current pane.
            source = tmux(local_tmux, "new-session", "-d", "-P", "-F", "#{session_id}", "-s", "local", "/bin/sh")
            pane = tmux(local_tmux, "display-message", "-p", "-t", source, "#{pane_id}")
            actor = terminal([*local_tmux, "attach-session", "-t", source])
            wait(lambda: run(*local_tmux, "list-clients", env=env).returncode == 0, "local client did not attach")
            fcntl.ioctl(actor[2], termios.TIOCSWINSZ, struct.pack("HHHH", 33, 111, 0, 0))
            os.kill(actor[0].pid, signal.SIGWINCH)
            command = shlex.join([str(BINARY), "attach", target_id, "--config", str(config), "--host", "remote"])
            marker = root / "inside-status"
            tmux(local_tmux, "send-keys", "-t", pane, "-l", command + "; printf '%s' $? > " + shlex.quote(str(marker)))
            tmux(local_tmux, "send-keys", "-t", pane, "Enter")
            wait(lambda: marker.exists() and marker.read_text() == "0", "inside tmux command failed")
            wait(lambda: "argos:remote:" in tmux(local_tmux, "list-windows", "-F", "#{window_name}"), "remote connection window was not created")
            wait(lambda: any(target_name in line for line in remote_clients()), "local connection window did not attach remotely")
            windows = [line.split("|", 1) for line in tmux(local_tmux, "list-windows", "-F", "#{window_id}|#{window_name}").splitlines()]
            remote_window = next(window_id for window_id, name in windows if name.startswith("argos:remote:"))
            tmux(local_tmux, "send-keys", "-t", remote_window, "C-b", "d")
            wait(lambda: not remote_clients(), "remote client from connection window did not detach")
            assert actor[0].poll() is None, "local tmux client was detached instead of the remote tmux client"
            assert before == tmux(remote_tmux, "list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")
        finally:
            run(*remote_tmux, "kill-server", env=env)
            run(*local_tmux, "kill-server", env=env)
            for process, master, slave, _ in clients:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
                os.close(master)
                os.close(slave)
            if sshd.poll() is None:
                sshd.terminate()
                try:
                    sshd.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    sshd.kill(); sshd.wait()
    print("PASS: real SSH remote attach from plain terminal, TUI, and local tmux connection window,")
    print("      exact target IDs/names, detach behavior, terminal restoration and unchanged panes.")


if __name__ == "__main__":
    main()
