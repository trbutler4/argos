#!/usr/bin/env python3
"""Default sessions attach owns the client; nested entry cannot create a chain."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import termios
import time

from connect import BINARY, BINDING, Terminal


def main():
    terminals = []
    with tempfile.TemporaryDirectory(prefix="argos-attach-", dir=os.environ.get("XDG_RUNTIME_DIR")) as directory:
        root = Path(directory)
        env = dict(os.environ, TERM="xterm-256color", ARGOS_STATE_DIR=str(root / "state"))
        for name in ("TMUX", "TMUX_PANE", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"):
            env.pop(name, None)
        socket = root / "tmux, quote ' α.sock"
        base = ["tmux", "-u", "-S", str(socket), "-f", "/dev/null"]

        def run(*args):
            return subprocess.run(args, env=env, text=True, capture_output=True, timeout=10)

        def tmux(*args):
            result = run(*base, *args)
            assert result.returncode == 0, result.stderr
            return result.stdout.removesuffix("\n")

        def terminal(args):
            term = Terminal(args, env)
            terminals.append(term)
            return term

        def wait(check):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                for term in terminals:
                    term.drain()
                if check():
                    return
                time.sleep(0.02)
            raise AssertionError([term.output[-2000:] for term in terminals])

        def clients():
            return tmux("list-clients", "-F", "#{client_pid}").splitlines()

        try:
            name = "target α ' ; $(literal) trailing "
            target = tmux("new-session", "-d", "-P", "-F", "#{session_id}", "-s", name, "/bin/sh")
            tmux("source-file", str(BINDING))
            before = tmux("list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")
            prefix_before = tmux("show-options", "-g", "prefix")
            config = root / "config.toml"
            config.write_text('schema_version=1\n[client]\nmachine_id="local"\n[machines.local]\nsocket=' + json.dumps(str(socket)))
            attach = [str(BINARY), "sessions", "attach", name, "--config", str(config)]
            assert run(*attach).returncode == 1, "non-terminal attachment accepted"
            peer = terminal([*base, "attach-session", "-t", target])
            wait(lambda: len(clients()) == 1)
            actor = terminal(attach)
            wait(lambda: len(clients()) == 2)
            assert str(actor.process.pid) not in clients(), "Argos exec'd away its owner"
            children = Path(f"/proc/{actor.process.pid}/task/{actor.process.pid}/children").read_text().split()
            assert set(clients()) == {str(peer.process.pid), *children}

            # Actual command executed inside a tmux pane, not fabricated nesting.
            marker = root / "nested-status"
            command = shlex.join(attach) + '; printf "%s" "$?" > ' + shlex.quote(str(marker))
            tmux("send-keys", "-t", target, "-l", command)
            tmux("send-keys", "-t", target, "Enter")
            wait(lambda: marker.exists() and marker.read_text() == "1")
            assert tmux("list-windows", "-F", "#{window_id}").count("\n") == 0

            for _ in range(2):
                actor.send(b"\x1b[24~")
                wait(lambda: b"Origin-side" in actor.output)
                assert clients() == [str(peer.process.pid)]
                actor.send(b"\x1b")
                wait(lambda: len(clients()) == 2)
            actor.send(b"\x02d")
            wait(lambda: actor.process.poll() is not None)
            assert actor.process.returncode == 0
            assert termios.tcgetattr(actor.slave) == actor.original
            assert before == tmux("list-panes", "-a", "-F", "#{pane_id}|#{pane_pid}")
            assert prefix_before == tmux("show-options", "-g", "prefix")
            assert clients() == [str(peer.process.pid)]
        finally:
            for term in terminals:
                term.close()
            run(*base, "kill-server")
    print("PASS: default sessions attach owns native tmux, exact names, guarded picker/reconnect,")
    print("      nested entry refusal, second-client preservation, terminal restoration and unchanged panes.")


if __name__ == "__main__":
    main()
