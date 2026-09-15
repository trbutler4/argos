#!/usr/bin/env python3
"""Real PTY + tmux attachment checks. Touch only disposable servers and clients."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import shlex
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def main():
    env = dict(os.environ, TERM="xterm-256color")
    env.pop("TMUX", None)
    env.pop("TMUX_PANE", None)
    clients = []

    def run(*args):
        return subprocess.run(args, env=env, text=True, capture_output=True, timeout=8)

    def drain():
        for process, master, slave, original in clients:
            while select.select([master], [], [], 0)[0]:
                try:
                    if not os.read(master, 65536):
                        break
                except OSError:
                    break

    def wait(check, description):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            drain()
            if check():
                return
            time.sleep(0.03)
        raise AssertionError(description)

    def terminal(args):
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        process = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave,
                                   env=env, start_new_session=True,
                                   preexec_fn=lambda: fcntl.ioctl(slave, termios.TIOCSCTTY, 0))
        item = (process, master, slave, original)
        clients.append(item)
        return item

    with tempfile.TemporaryDirectory(prefix="argos-attach-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temp:
        root = Path(temp)
        sock = root / "tmux, test.sock"
        second = root / "second.sock"
        base = ["tmux", "-u", "-S", str(sock), "-f", "/dev/null"]

        def tmux(*args):
            r = run(*base, *args)
            assert r.returncode == 0, (args, r.stderr)
            return r.stdout.removesuffix("\n")

        def attached():
            text = tmux("list-clients", "-F", "#{client_pid}|#{session_id}")
            return dict(line.split("|", 1) for line in text.splitlines())

        def shell_command(pane, args, marker):
            command = shlex.join(args) + '; printf "%s" "$?" > ' + shlex.quote(str(marker))
            tmux("send-keys", "-t", pane, "-l", command)
            tmux("send-keys", "-t", pane, "Enter")
            wait(marker.exists, "shell did not report command completion")
            return marker.read_text()

        try:
            source_name = "source ' ; $(literal) |"
            target_name = "target α trailing "
            source = tmux("new-session", "-d", "-P", "-F", "#{session_id}", "-s", source_name, "/bin/sh")
            target = tmux("new-session", "-d", "-P", "-F", "#{session_id}", "-s", target_name, "/bin/sh")
            source_pane = tmux("display-message", "-p", "-t", source, "#{pane_id}")
            target_pane = tmux("display-message", "-p", "-t", target, "#{pane_id}")
            counter = root / "counter"
            tmux("new-window", "-d", "-t", target, "/bin/sh", "-c",
                 'i=0; while :; do i=$((i+1)); printf "%s" "$i" > ' + shlex.quote(str(counter)) + '; sleep 0.1; done')
            wait(lambda: counter.exists() and counter.read_text().isdigit(), "counter failed to start")
            prefix_before = tmux("show-options", "-g", "prefix")
            before = tmux("list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")
            config = root / "config.toml"
            config.write_text('schema_version=1\n[client]\nmachine_id="local"\n[machines.local]\nsocket=' + json.dumps(str(sock)) + '\n[machines.remote]\nssh_alias="unused.invalid"\n')
            prefix = [str(BINARY), "attach"]
            for arguments, code in [([], 2), (["missing", "--socket", str(sock)], 1),
                                    (["source", "--socket", str(sock)], 1),
                                    ([source, "--socket", str(sock), "--json"], 2),
                                    ([source, "--config", str(config), "--host", "remote"], 2),
                                    ([source, "--socket", str(sock)], 1)]:
                r = run(*prefix, *arguments)
                assert r.returncode == code, (arguments, r.returncode, r.stdout, r.stderr)
            assert attached() == {}, "noninteractive validation unexpectedly attached a client"

            # Keep another client on the target to prove attach/switch never detaches it.
            peer = terminal([*base, "attach-session", "-t", target])
            wait(lambda: attached().get(str(peer[0].pid)) == target, "peer did not attach")
            actor = terminal([*prefix, source_name, "--config", str(config)])
            wait(lambda: attached().get(str(actor[0].pid)) == source, "Argos did not attach from a plain terminal")
            assert attached().get(str(peer[0].pid)) == target
            fcntl.ioctl(actor[2], termios.TIOCSWINSZ, struct.pack("HHHH", 31, 101, 0, 0))
            os.kill(actor[0].pid, signal.SIGWINCH)
            wait(lambda: f"{actor[0].pid}|101|31" in tmux("list-clients", "-F", "#{client_pid}|#{client_width}|#{client_height}"),
                 "tmux did not receive terminal resize")

            # Execute Argos from the real tmux shell, not a fabricated TMUX environment.
            marker = root / "switch-status"
            status = shell_command(source_pane, [*prefix, "source", "--config", str(config)], root / "missing-status")
            assert status != "0", "prefix unexpectedly matched an exact target"
            assert attached().get(str(actor[0].pid)) == source
            status = shell_command(source_pane, [*prefix, source_name, "--config", str(config)], root / "same-status")
            assert status == "0", (status, tmux("capture-pane", "-p", "-t", source_pane))
            status = shell_command(source_pane, [*prefix, target_name, "--local"], marker)
            assert status == "0", (status, tmux("capture-pane", "-p", "-t", source_pane))
            wait(lambda: attached().get(str(actor[0].pid)) == target, "invoking client did not switch")
            assert attached().get(str(peer[0].pid)) == target, "another client was displaced"
            assert before == tmux("list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")

            # With two clients on the same session there is no safe implicit client choice.
            status = shell_command(target_pane, [*prefix, source, "--config", str(config)], root / "ambiguous-status")
            assert status != "0"
            assert set(attached().values()) == {target}, "ambiguous attach moved a client"

            # Never nest into a different tmux server from an existing tmux pane.
            other_base = ["tmux", "-u", "-S", str(second), "-f", "/dev/null"]
            r = run(*other_base, "new-session", "-d", "-s", "other", "/bin/sh")
            assert r.returncode == 0, r.stderr
            status = shell_command(target_pane, [*prefix, "other", "--socket", str(second)], root / "cross-server-status")
            assert status != "0"
            assert set(attached().values()) == {target}

            # Detach using tmux's real default prefix, then confirm terminal restoration.
            wait(lambda: counter.read_text().isdigit(), "counter not readable")
            count_before_detach = int(counter.read_text())
            os.write(actor[1], b"\x02d")
            wait(lambda: actor[0].poll() is not None, "Argos/native tmux did not detach")
            assert actor[0].returncode == 0
            restored = termios.tcgetattr(actor[2])
            assert restored == actor[3], "terminal attributes not restored after detach"
            wait(lambda: counter.read_text().isdigit() and int(counter.read_text()) > count_before_detach,
                 "backend counter did not advance after detach")
            assert tmux("show-options", "-g", "prefix") == prefix_before
            assert attached().get(str(peer[0].pid)) == target
            assert before == tmux("list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")
        finally:
            for socket_path in (sock, second):
                run("tmux", "-N", "-S", str(socket_path), "kill-server")
            for process, master, slave, original in clients:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                os.close(master)
                os.close(slave)
    print("PASS: real PTY attach, same-server switch, other-client preservation, exact names,")
    print("      ambiguous/cross-server refusal, detach, terminal restoration and unchanged pane PIDs.")


if __name__ == "__main__":
    main()
