#!/usr/bin/env python3
"""Exercise the real CLI and an isolated real tmux server. Never touch user sessions."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("CMD_CENTER_BINARY", ROOT / "target/debug/cmd-center"))


def run(*args, env=None):
    return subprocess.run(args, text=True, capture_output=True, env=env, timeout=15)


def cli(*args, code=0, env=None):
    result = run(str(BINARY), *args, env=env)
    assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
    return result


def snapshot(path, *, code=0, env=None):
    result = cli("list", "--json", "--socket", str(path), code=code, env=env)
    value = json.loads(result.stdout)
    assert value["schema_version"] == 1
    assert isinstance(value["observed_at_unix_ms"], int)
    assert value["observed_at_unix_ms"] > 0
    assert len(value["hosts"]) == 1
    assert value["hosts"][0]["id"] == socket.gethostname()
    return value["hosts"][0]


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    assert "list" in cli().stdout
    assert "list" in cli("--help").stdout
    assert "--json" in cli("list", "--help").stdout
    # Clap must reject invalid commands/options rather than connecting anywhere.
    cli("not-a-command", code=2)
    cli("list", "--not-an-option", code=2)
    cli("list", "--socket", code=2)

    base = os.environ.get("XDG_RUNTIME_DIR")
    with tempfile.TemporaryDirectory(prefix="cc-test-", dir=base) as directory:
        directory = Path(directory)
        path = directory / "tmux.sock"
        tmux_args = ["tmux", "-u", "-S", str(path), "-f", "/dev/null"]

        def tmux(*args):
            result = run(*tmux_args, *args)
            assert result.returncode == 0, (args, result.stdout, result.stderr)
            return result.stdout

        # Missing server is an empty successful result and discovery must not start it.
        empty = snapshot(path)
        assert empty["status"] == "ok" and empty["sessions"] == []
        assert empty["error"] is None
        assert not path.exists()
        assert "no" in cli("list", "--socket", str(path)).stdout.lower()

        # An invalid socket object is a genuine operational error, not an empty host.
        invalid = directory / "not-a-socket"
        invalid.write_text("not a tmux socket\n")
        failed = snapshot(invalid, code=1)
        assert failed["status"] == "error" and failed["error"]["code"]

        # Test missing runtime dependency through the actual binary's public interface.
        no_tools = dict(os.environ, PATH=str(directory / "empty-path"))
        if os.environ.get("CMD_CENTER_PACKAGED"):
            # The Nix wrapper must supply its own tmux runtime dependency.
            assert snapshot(path, env=no_tools)["status"] == "ok"
        else:
            failed = snapshot(path, code=1, env=no_tools)
            assert failed["status"] == "error"
            assert failed["error"]["code"] == "tmux_not_found"

        try:
            names = ["quotes ' \" ; $(literal) trailing ", "unicode α and \"quoted\""]
            ids = [tmux("new-session", "-d", "-P", "-F", "#{session_id}",
                        "-s", name, "sleep 300").strip() for name in names]
            tmux("new-window", "-d", "-t", ids[0], "sleep 300")
            before = tmux("list-panes", "-a", "-F", "#{pane_id}:#{pane_pid}")
            first = snapshot(path)
            assert first["status"] == "ok" and first["error"] is None
            actual = {entry["id"]: entry for entry in first["sessions"]}
            assert set(actual) == set(ids), actual
            for i, name in enumerate(names):
                assert actual[ids[i]]["name"] == name, actual
                assert actual[ids[i]]["windows"] == (2 if i == 0 else 1)
                assert actual[ids[i]]["attached_clients"] == 0
            second = snapshot(path)
            assert first["sessions"] == second["sessions"]
            minimal = {"HOME": os.environ["HOME"], "PATH": os.environ["PATH"], "LC_ALL": "C"}
            assert snapshot(path, env=minimal)["sessions"] == first["sessions"]
            human = cli("list", "--socket", str(path))
            assert "literal" in human.stdout and "unicode" in human.stdout
            assert "\x1b" not in human.stdout
            assert "\\\"quoted\\\"" in human.stdout
            after = tmux("list-panes", "-a", "-F", "#{pane_id}:#{pane_pid}")
            assert before == after, "Discovery changed a test session's processes"
        finally:
            # Only the unique test server can be terminated by this cleanup.
            run(*tmux_args, "kill-server")

        assert snapshot(path)["sessions"] == []
    print("PASS: real CLI help/usage, no-server, bad socket, missing tmux, exact unusual names,")
    print("      JSON schema, numeric metadata, human output and unchanged real tmux processes.")


if __name__ == "__main__":
    main()
