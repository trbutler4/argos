#!/usr/bin/env python3
"""Managed connection lifecycle through real PTYs, tmux, and loopback SSH.

All bindings, keys, sockets, masters, and sessions belong to this fixture.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import pwd
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
BINDING = Path(os.environ.get("ARGOS_TMUX_BINDING", ROOT / "examples/connect.tmux.conf"))


class Terminal:
    def __init__(self, args, env):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 150, 0, 0))
        self.original = termios.tcgetattr(self.slave)
        self.original_flags = fcntl.fcntl(self.slave, fcntl.F_GETFL)
        self.output = b""
        self.process = subprocess.Popen(
            args, stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env,
            start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0),
        )

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            try:
                data = os.read(self.master, 65536)
                if not data:
                    break
                self.output = (self.output + data)[-200000:]
            except OSError:
                break

    def send(self, data):
        self.output = b""
        os.write(self.master, data)

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        os.close(self.master)
        os.close(self.slave)


def main():
    for tool in ("sshd", "ssh", "ssh-keygen", "tmux", "grep"):
        assert shutil.which(tool), f"missing {tool}; use nix develop"
    terminals = []
    with tempfile.TemporaryDirectory(prefix="argos-connect-", dir=os.environ.get("XDG_RUNTIME_DIR")) as directory:
        root = Path(directory)
        env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(root / "config-home"),
                   ARGOS_STATE_DIR=str(root / "empty-state"))
        for name in ("TMUX", "TMUX_PANE", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "ARGOS_CONNECT_CLIENT"):
            env.pop(name, None)
        actual_ssh = shutil.which("ssh")
        ssh_config = root / "ssh_config"
        wrapper_dir = root / "bin"
        wrapper_dir.mkdir()
        wrapper = wrapper_dir / "ssh"
        probe_log = root / "probe-pids"
        overlap = root / "probe-overlap"
        guard = root / "probe-guard"
        # Delay actual preflight/discovery invocations, never fabricate SSH
        # results. Children inherit pipes to exercise complete group cleanup.
        wrapper.write_text(f'''#!/bin/sh
root={shlex.quote(str(root))}
mode=
case " $* " in
  *" -G "*) mode=config ;;
  *list-sessions*) mode=probe ;;
esac
if [ -n "$mode" ] && [ -f "$root/slow-$mode" ]; then
  sleep 30 & helper=$!
  printf '%s %s\\n' "$$" "$helper" >> "$root/probe-pids"
  if [ -f "$root/exit-probe-parent" ]; then exit 0; fi
  wait "$helper"
fi
case " $* " in
  *attach-session*)
    if [ -f "$root/probe-guard" ]; then
      for pid in $(cat "$root/probe-guard"); do
        if kill -0 "$pid" 2>/dev/null; then printf '%s\\n' "$pid" >> "$root/probe-overlap"; fi
      done
    fi ;;
esac
exec {shlex.quote(actual_ssh)} -F {shlex.quote(str(ssh_config))} "$@"
''')
        wrapper.chmod(0o755)
        env["PATH"] = str(wrapper_dir) + os.pathsep + env["PATH"]

        def run(*args, check=True):
            result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=10)
            if check:
                assert result.returncode == 0, (args, result.stderr)
            return result

        def terminal(*args, override=None):
            term = Terminal(list(args), env if override is None else dict(env, **override))
            terminals.append(term)
            return term

        def wait(check, message, timeout=10):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                for term in terminals:
                    term.drain()
                if check():
                    return
                time.sleep(0.02)
            raise AssertionError((message, [(t.process.poll(), t.output[-3000:]) for t in terminals]))

        def exited(term, code=0):
            wait(lambda: term.process.poll() is not None, "process did not exit")
            assert term.process.returncode == code, (code, term.process.returncode, term.output)
            assert termios.tcgetattr(term.slave) == term.original, "terminal not restored"
            assert fcntl.fcntl(term.slave, fcntl.F_GETFL) == term.original_flags, "terminal descriptor flags not restored"

        keys = [root / "server_key", root / "client_key"]
        for key in keys:
            run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
        authorized = root / "authorized_keys"
        authorized.write_text(Path(str(keys[1]) + ".pub").read_text())
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        known = root / "known_hosts"
        kind, blob, *_ = Path(str(keys[0]) + ".pub").read_text().split()
        known.write_text(f"[127.0.0.1]:{port} {kind} {blob}\n")
        ssh_config.write_text(
            f"Host peer-a peer-b\n HostName 127.0.0.1\n Port {port}\n User {pwd.getpwuid(os.getuid()).pw_name}\n"
            f" IdentityFile {keys[1]}\n IdentitiesOnly yes\n UserKnownHostsFile {known}\n"
            f" ControlMaster auto\n ControlPath {root}/master-%n\n ControlPersist 600\n"
            " StrictHostKeyChecking yes\n BatchMode yes\n"
        )
        sshd_config = root / "sshd_config"
        sshd_config.write_text(
            f"HostKey {keys[0]}\nAuthorizedKeysFile {authorized}\nUsePAM no\nStrictModes no\n"
            f"PasswordAuthentication no\nAuthenticationMethods publickey\nListenAddress 127.0.0.1\nPort {port}\n"
            "LogLevel ERROR\nPidFile none\nPermitTTY yes\nPerSourcePenalties no\n"
        )
        sshd = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(sshd_config)],
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        sockets = {name: root / f"{name} ' α.sock" for name in ("local", "a", "b")}

        def tmux(host, *args, check=True):
            return run("tmux", "-u", "-N", "-S", str(sockets[host]), *args, check=check).stdout.strip()

        def clients(host):
            text = tmux(host, "list-clients", "-F", "#{client_pid}|#{session_id}")
            return dict(line.split("|", 1) for line in text.splitlines())

        def children(term):
            owned = set()
            for path in Path(f"/proc/{term.process.pid}/task").glob("*/children"):
                try:
                    owned.update(path.read_text().split())
                except FileNotFoundError:
                    pass
            return sorted(owned)

        def picker(term, error=False):
            needle = b"failed:" if error else b"Origin-side"
            wait(lambda: needle in term.output and b"READY" in term.output, "picker did not finish discovery")
            assert children(term) == [], "picker opened before owned child was reaped"
            assert os.tcgetpgrp(term.master) == term.process.pid, "foreground group not restored"

        def managed(host):
            result = {}
            for pid, session in clients(host).items():
                try:
                    entries = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
                except FileNotFoundError:
                    continue
                if b"ARGOS_CONNECT_CLIENT=1" in entries:
                    result[pid] = session
            return result

        config = root / "inventory.toml"
        config.write_text(
            "schema_version=1\n[client]\nmachine_id='local'\nconnect_timeout_seconds=2\nmax_parallel_probes=2\n"
            f"[machines.local]\nsocket={json.dumps(str(sockets['local']))}\n"
            f"[machines.a]\nssh_alias='peer-a'\nsocket={json.dumps(str(sockets['a']))}\n"
            f"[machines.b]\nssh_alias='peer-b'\nsocket={json.dumps(str(sockets['b']))}\n"
        )
        connect = [str(BINARY), "connect", "--config", str(config)]
        try:
            wait(lambda: run("ssh", "-o", "ControlPath=none", "peer-a", "true", check=False).returncode == 0,
                 "sshd not reachable")
            ids = {}
            for host in sockets:
                # -N attach/list must not create servers, but this explicit test
                # setup does. /dev/null prevents sourcing personal tmux config.
                result = run("tmux", "-u", "-S", str(sockets[host]), "-f", "/dev/null",
                             "new-session", "-d", "-P", "-F", "#{session_id}",
                             "-s", f"{host} α ' ; $(literal) trailing ", "/bin/sh")
                ids[host] = result.stdout.strip()
                tmux(host, "source-file", str(BINDING))
                # Even a user-configured update list must not leak the marker
                # from a managed client into shared session environment.
                tmux(host, "set-option", "-g", "update-environment", "ARGOS_CONNECT_CLIENT")
            before = {host: tmux(host, "list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}") for host in sockets}

            # Marker is client-specific, even on the same session. A missing
            # marker produces guidance without detaching an unmanaged client.
            peer = terminal("tmux", "-S", str(sockets["a"]), "attach", "-t", ids["a"])
            wait(lambda: len(clients("a")) == 1, "unmanaged peer did not attach")
            peer.send(b"\x1b[24~")
            wait(lambda: b"Start Argos" in peer.output, "guard did not show guidance")
            assert peer.process.poll() is None
            actor = terminal(str(BINARY), "sessions", "attach", f"host:a:{ids['a']}", "--config", str(config))
            wait(lambda: len(managed("a")) == 1, "managed SSH client did not attach")
            assert len(clients("a")) == 2
            assert "=1" not in tmux("a", "show-environment", "-t", ids["a"], "ARGOS_CONNECT_CLIENT", check=False)
            actor.send(b"\x1b[24~")
            picker(actor)
            assert clients("a") == {str(peer.process.pid): ids["a"]}
            actor.send(b"\x1b")
            wait(lambda: len(managed("a")) == 1, "Escape did not reconnect")
            actor.send(b"\x02d")
            exited(actor)
            assert peer.process.poll() is None

            # Initial cancellation, nested refusal, VM rejection, selectors.
            initial = terminal(*connect)
            picker(initial)
            initial.send(b"\x1b")
            exited(initial)
            for name in ("TMUX", "TMUX_PANE", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"):
                for args in ([*connect, f"host:a:{ids['a']}"],
                             [str(BINARY), "sessions", "attach", f"host:a:{ids['a']}", "--config", str(config)],
                             [str(BINARY), "tui", "--config", str(config)]):
                    rejected = terminal(*args, override={name: "nested"})
                    exited(rejected, 1)
                    assert b"original terminal" in rejected.output
            for args in (["vm:"], ["host:a:test"], ["host:a:test", "--socket", str(sockets["a"])]):
                rejected = terminal(str(BINARY), "connect", *args)
                exited(rejected, 2)

            # Deliberately establish an unrelated fixture-owned master. Managed
            # attachment must leave it alive and use a separate SSH process.
            run("ssh", "-M", "-N", "-f", "peer-a")
            run("ssh", "-O", "check", "peer-a")
            stress = terminal(str(BINARY), "tui", "--config", str(config))
            picker(stress)
            stress.send(b"\r")
            wait(lambda: len(managed("a")) == 1, "stress initial attach failed")
            for iteration in range(int(os.environ.get("ARGOS_SWITCH_COUNT", "100"))):
                attached_children = children(stress)
                assert len(attached_children) == 1, attached_children
                child_pid = int(attached_children[0])
                assert os.getpgid(child_pid) == child_pid
                stress.send(b"\x1b[24~")
                picker(stress)
                assert not managed("a") and not managed("b")
                assert not Path(f"/proc/{child_pid}").exists(), "old SSH child survived"
                destination = "b" if iteration % 2 == 0 else "a"
                # Inventory sorts a, b, local; the first selectable row is a.
                stress.send((b"jjjjj" if destination == "b" else b"") + b"\r")
                wait(lambda: len(managed(destination)) == 1, "switch did not attach")
                assert len(children(stress)) == 1
            stress.send(b"\x02d")
            exited(stress)
            run("ssh", "-O", "check", "peer-a")

            # Local <-> remote uses the same loop; SIGTERM at the origin cleans
            # the owned group and returns the terminal without touching peers.
            local = terminal(*connect, f"host:local:{ids['local']}")
            wait(lambda: len(managed("local")) == 1, "local managed attach failed")
            fcntl.ioctl(local.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 111, 0, 0))
            os.kill(local.process.pid, signal.SIGWINCH)
            wait(lambda: "111|32" in tmux("local", "list-clients", "-F", "#{client_width}|#{client_height}"), "resize lost")
            local.send(b"\x1b[24~")
            picker(local)
            local.send(b"\r")
            wait(lambda: len(managed("a")) == 1, "local to remote failed")
            child_pid = int(children(local)[0])
            os.kill(local.process.pid, signal.SIGTERM)
            exited(local, 143)
            wait(lambda: not managed("a"), "remote client survived origin termination")
            assert not Path(f"/proc/{child_pid}").exists()

            # Native same-server switching does not change the documented
            # last-Argos-selected reconnect target.
            extra = tmux("local", "new-session", "-d", "-P", "-F", "#{session_id}", "-s", "extra", "/bin/sh")
            tracked = terminal(*connect, f"host:local:{ids['local']}")
            wait(lambda: len(managed("local")) == 1, "tracking client did not attach")
            client_pid = next(iter(managed("local")))
            client_tty = tmux("local", "list-clients", "-F", "#{client_tty}")
            tmux("local", "switch-client", "-c", client_tty, "-t", extra)
            wait(lambda: managed("local").get(client_pid) == extra, "native switch failed")
            tracked.send(b"\x1b[24~")
            picker(tracked)
            tracked.send(b"\x1b")
            wait(lambda: list(managed("local").values()) == [ids["local"]], "cancel tracked wrong target")
            tracked.send(b"\x02d")
            exited(tracked)
            tmux("local", "kill-session", "-t", extra)

            # Suspend the origin, resume with its foreground group, then return
            # to the picker. Test both native tmux and SSH attachments.
            for host in ("local", "a"):
                suspended = terminal(*connect, f"host:{host}:{ids[host]}")
                wait(lambda: len(managed(host)) == 1, "suspend client did not attach")
                os.kill(suspended.process.pid, signal.SIGTSTP)
                state = Path(f"/proc/{suspended.process.pid}/status")
                wait(lambda: "State:\tT" in state.read_text(), "origin did not suspend")
                assert termios.tcgetattr(suspended.slave) == suspended.original
                os.kill(suspended.process.pid, signal.SIGCONT)
                wait(lambda: "State:\tT" not in state.read_text(), "origin did not resume")
                suspended.send(b"\x1b[24~")
                picker(suspended)
                suspended.send(b"\x03")
                exited(suspended, 130)

            for sig in (signal.SIGHUP, signal.SIGINT):
                interrupted = terminal(*connect, f"host:local:{ids['local']}")
                wait(lambda: len(managed("local")) == 1, "interrupt client did not attach")
                os.kill(interrupted.process.pid, sig)
                exited(interrupted, 128 + sig)
                wait(lambda: not managed("local"), "interrupted client survived")

            # Exercise real interactive-shell job control, not only a session
            # leader launched directly by the PTY fixture.
            shell = terminal(shutil.which("bash"), "--noprofile", "--norc", "-i")
            shell.send((shlex.join([*connect, f"host:local:{ids['local']}"]) + "\n").encode())
            wait(lambda: len(managed("local")) == 1, "shell-launched client did not attach")
            origin_pid = int(children(shell)[0])
            shell.send(b"\x02\x1a")  # default tmux suspend-client binding
            state = Path(f"/proc/{origin_pid}/status")
            wait(lambda: "State:\tT" in state.read_text(), "native suspend did not stop origin")
            wait(lambda: os.tcgetpgrp(shell.master) == shell.process.pid, "shell did not regain terminal")
            shell.send(b"fg\r")
            wait(lambda: os.tcgetpgrp(shell.master) != shell.process.pid, "fg did not resume attachment")
            shell.send(b"\x1b[24~")
            wait(lambda: b"Origin-side" in shell.output, "resumed shell attachment did not open picker")
            assert os.tcgetpgrp(shell.master) == origin_pid
            shell.send(b"q")
            wait(lambda: not children(shell), "shell-launched origin did not exit")
            shell.send(b"exit\r")
            exited(shell)

            # Killing just the SSH transport leads to recovery, then retry
            # creates one new attachment without losing the remote pane.
            lost = terminal(*connect, f"host:b:{ids['b']}")
            wait(lambda: len(managed("b")) == 1, "loss client did not attach")
            paste_marker = root / "paste-result"
            pasted = f"printf pasted > {shlex.quote(str(paste_marker))}".encode()
            lost.send(b"\x1b[200~" + pasted + b"\x1b[201~\r")
            wait(lambda: paste_marker.exists() and paste_marker.read_text() == "pasted", "bracketed paste lost")
            lost.send(b"sleep 60\r")
            wait(lambda: "sleep" in tmux("b", "list-panes", "-F", "#{pane_current_command}"), "foreground command not running")
            lost.send(b"\x03")
            wait(lambda: "sleep" not in tmux("b", "list-panes", "-F", "#{pane_current_command}"), "Ctrl-C did not reach pane")
            assert lost.process.poll() is None, "pane interrupt killed origin"
            lost.output = b""
            os.kill(int(children(lost)[0]), signal.SIGKILL)
            picker(lost, error=True)
            wait(lambda: not managed("b"), "lost SSH left an attached client")
            lost.send(b"R")
            wait(lambda: len(managed("b")) == 1, "loss retry failed")
            lost.send(b"\x02d")
            exited(lost)

            # Failure stays visible. Retry does not recursively add owners.
            failed = terminal(*connect, "host:a:missing")
            picker(failed, error=True)
            failed.send(b"R")
            picker(failed, error=True)
            failed.send(b"\r")
            wait(lambda: len(managed("a")) == 1, "choose another target failed")
            failed.send(b"\x1b[24~")
            picker(failed)
            failed.send(b"q")
            exited(failed)

            # Host-key and authentication errors remain visible with healthy
            # host rows. Never silently accept a key or fall back to passwords.
            saved_known = known.read_text()
            known.write_text("")
            untrusted = terminal(*connect, f"host:b:{ids['b']}")
            picker(untrusted, error=True)
            untrusted.send(b"q")
            exited(untrusted)
            known.write_text(saved_known)
            saved_authorized = authorized.read_text()
            authorized.write_text("")
            unauthorized = terminal(*connect, f"host:b:{ids['b']}")
            picker(unauthorized, error=True)
            unauthorized.send(b"q")
            exited(unauthorized)
            authorized.write_text(saved_authorized)

            # Proxy configuration is rejected before starting the command.
            saved_config = ssh_config.read_text()
            proxy_marker = root / "proxy-ran"
            ssh_config.write_text(saved_config + f" ProxyCommand touch {proxy_marker}\n")
            proxy = terminal(*connect, f"host:b:{ids['b']}")
            picker(proxy, error=True)
            assert b"ProxyCommand" in proxy.output and not proxy_marker.exists()
            proxy.send(b"q")
            exited(proxy)
            ssh_config.write_text(saved_config)

            # The initial screen, healthy local data, and quit remain responsive
            # during slow SSH config inspection and slow tmux probes. The last
            # case leaves an exited wrapper with a live child holding its pipes.
            for mode, orphan_pipe, exit_code in (("config", False, 0), ("probe", False, 130), ("probe", True, 143)):
                probe_log.write_text("")
                slow = root / f"slow-{mode}"
                slow.touch()
                parent_exit = root / "exit-probe-parent"
                if orphan_pipe:
                    parent_exit.touch()
                started = time.monotonic()
                responsive = terminal(str(BINARY), "tui", "--config", str(config))
                wait(lambda: b"Origin-side" in responsive.output and b"windows" in responsive.output,
                     "slow discovery blocked first render or healthy local sessions", timeout=2)
                assert time.monotonic() - started < 2
                wait(lambda: len(probe_log.read_text().splitlines()) == 2, "slow probes did not start")
                if mode == "config":
                    responsive.output = b""
                    fcntl.ioctl(responsive.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 140, 0, 0))
                    os.kill(responsive.process.pid, signal.SIGWINCH)
                    wait(lambda: b"\x1b[34;1H" in responsive.output, "resize blocked by discovery", timeout=2)
                pids = probe_log.read_text().split()
                started = time.monotonic()
                if exit_code == 143:
                    os.kill(responsive.process.pid, signal.SIGTERM)
                else:
                    responsive.send(b"q" if exit_code == 0 else b"\x03")
                exited(responsive, exit_code)
                assert time.monotonic() - started < 2, "quit waited for discovery deadline"
                wait(lambda: all(not Path(f"/proc/{pid}").exists() for pid in pids), "cancel left probe descendants")
                slow.unlink()
                if orphan_pipe:
                    parent_exit.unlink()

            # Refresh keeps old rows visible but marked stale, coalesces repeated
            # requests, preserves target identity when rows shift, and joins all
            # probes before starting an interactive transport.
            refreshed = terminal(str(BINARY), "tui", "--config", str(config))
            picker(refreshed)
            idle_threads = len(list(Path(f"/proc/{refreshed.process.pid}/task").iterdir()))
            refreshed.send(b"jjjjj")  # select b
            vms = root / "empty-state" / "vms"
            vms.mkdir(parents=True)
            for number in (0, 1):
                (vms / f"refresh-{number}.json").write_text(json.dumps({
                    "schema_version": 1, "id": f"refresh-{number}", "name": f"Refresh VM {number}",
                    "host": "a", "status": "stopped",
                }))
            probe_log.write_text("")
            slow = root / "slow-probe"
            slow.touch()
            refreshed.send(b"r")
            wait(lambda: len(probe_log.read_text().splitlines()) == 2 and b"stale" in refreshed.output,
                 "refresh did not stream stale/loading state")
            wait(lambda: b"Refresh" in refreshed.output, "VM results did not arrive during slow host probes")
            refreshed.send(b"r" * 40)
            time.sleep(0.1)
            assert len(probe_log.read_text().splitlines()) == 2, "refresh started overlapping jobs"
            assert len(list(Path(f"/proc/{refreshed.process.pid}/task").iterdir())) <= idle_threads + 4
            old_pids = probe_log.read_text().split()
            guard.write_text(" ".join(line.split()[0] for line in probe_log.read_text().splitlines()))
            slow.unlink()  # already-running probes still sleep until cancelled
            refreshed.send(b"\r")
            wait(lambda: len(managed("b")) == 1, "refresh changed selected target or blocked attachment")
            assert not overlap.exists(), "new transport overlapped old discovery children"
            wait(lambda: all(not Path(f"/proc/{pid}").exists() for pid in old_pids), "handoff left probe descendants")
            refreshed.send(b"\x02d")
            exited(refreshed)
            guard.unlink()

            for host in sockets:
                assert before[host] == tmux(host, "list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")
            assert clients("a") == {str(peer.process.pid): ids["a"]}
        finally:
            for term in terminals:
                term.close()
            for host in sockets:
                tmux(host, "kill-server", check=False)
            for alias in ("peer-a", "peer-b"):
                run("ssh", "-O", "exit", alias, check=False)
            sshd.terminate()
            try:
                sshd.wait(timeout=5)
            except subprocess.TimeoutExpired:
                sshd.kill()
                sshd.wait()
    print("PASS: managed local/SSH picker, guarded per-client handoff, cancel/retry/quit,")
    print("      repeated cross-host switching, master isolation, owned-child cleanup, resize and persistence.")
    print("      Slow-probe rendering/resize/quit, cancellation, coalesced refresh, and joined handoff.")


if __name__ == "__main__":
    main()
