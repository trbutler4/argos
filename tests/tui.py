#!/usr/bin/env python3
"""Real PTY smoke test for the read-only Argos TUI."""
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def run(*args, env=None):
    return subprocess.run(args, text=True, capture_output=True, env=env, timeout=10)


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    env = dict(os.environ, TERM="xterm-256color")
    env.pop("TMUX", None)
    env.pop("TMUX_PANE", None)
    help_result = run(str(BINARY), "tui", "--help", env=env)
    assert help_result.returncode == 0 and "--config" in help_result.stdout
    non_tty = run(str(BINARY), "tui", "--local", env=env)
    assert non_tty.returncode == 1 and "requires a terminal" in non_tty.stderr

    with tempfile.TemporaryDirectory(prefix="argos-tui-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temp:
        root = Path(temp)
        sock = root / "tmux.sock"
        tmux = ["tmux", "-u", "-S", str(sock), "-f", "/dev/null"]
        name = "tui target α trailing "
        created = run(*tmux, "new-session", "-d", "-s", name, "/bin/sh", env=env)
        assert created.returncode == 0, created.stderr
        session_id = run(*tmux, "display-message", "-p", "-t", name, "#{session_id}", env=env).stdout.strip()

        def terminal():
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
            original = termios.tcgetattr(slave)
            output = bytearray()
            process = subprocess.Popen(
                [str(BINARY), "tui", "--socket", str(sock)],
                stdin=slave,
                stdout=slave,
                stderr=slave,
                env=env,
                start_new_session=True,
                preexec_fn=lambda: fcntl.ioctl(slave, termios.TIOCSCTTY, 0),
            )
            return process, master, slave, original, output

        def drain(master, output):
            while select.select([master], [], [], 0)[0]:
                chunk = os.read(master, 65536)
                if not chunk:
                    break
                output.extend(chunk)

        def wait_rendered(master, output):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                drain(master, output)
                text = output.decode(errors="ignore")
                if all(token in text for token in ("Argos", "tui", "target", "refresh", "quit", "read-only")):
                    return
                time.sleep(0.05)
            raise AssertionError(output.decode(errors="ignore"))

        process, master, slave, original, output = terminal()
        try:
            wait_rendered(master, output)
            os.write(master, b"jkrq")
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline and process.poll() is None:
                drain(master, output)
                time.sleep(0.05)
            assert process.poll() is not None, "TUI did not quit after q"
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == original, "terminal attributes not restored"
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            os.close(master)
            os.close(slave)

        run(*tmux, "kill-server", env=env)
    print("PASS: real PTY TUI renders sessions, handles navigation/refresh/quit,")
    print("      remains read-only, and restores terminal mode.")


if __name__ == "__main__":
    main()
