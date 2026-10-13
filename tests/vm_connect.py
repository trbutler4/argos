#!/usr/bin/env python3
"""Opt-in real microVM connection acceptance; creates and removes one test VM.

Requires nix develop, local KVM, and permission to run a disposable microVM.
Only generated SSH credentials, isolated HOME/state, and fixture tmux servers
are used. The guest never receives the user's credentials or checkout.
"""
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import tempfile
import termios
import time

from connect import BINARY, BINDING, Terminal


def main():
    assert os.access("/dev/kvm", os.R_OK | os.W_OK), "local KVM access required"
    root = Path(tempfile.mkdtemp(prefix="argos-vm-connect-", dir=os.environ.get("XDG_RUNTIME_DIR")))
    home = root / "home"
    ssh_dir = home / ".ssh"
    ssh_dir.mkdir(parents=True, mode=0o700)
    state = root / "state"
    tmux_tmp = root / "tmux"
    tmux_tmp.mkdir()
    config_dir = root / "config" / "argos"
    config_dir.mkdir(parents=True)
    host_socket = root / "host.sock"
    config = config_dir / "config.toml"
    config.write_text(
        'schema_version=1\n[client]\nmachine_id="fixture"\n[machines.fixture]\n'
        f'socket={json.dumps(str(host_socket))}\n[vm.guest_tmux]\nconfig_path={json.dumps(str(BINDING))}\n'
    )
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(root / "config"),
               ARGOS_STATE_DIR=str(state), TMUX_TMPDIR=str(tmux_tmp), TERM="xterm-256color")
    for key in ("TMUX", "TMUX_PANE", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "SSH_AUTH_SOCK", "ARGOS_CONNECT_CLIENT"):
        env.pop(key, None)
    env["NIX_CONFIG"] = env.get("NIX_CONFIG", "") + "\nexperimental-features = nix-command flakes\n"
    key = ssh_dir / "id_ed25519"
    ssh_config = root / "ssh_config"
    ssh_config.write_text(f"Host *\n IdentityFile {key}\n IdentitiesOnly yes\n BatchMode yes\n")
    wrapper_dir = root / "bin"
    wrapper_dir.mkdir()
    wrapper = wrapper_dir / "ssh"
    wrapper.write_text(f'#!/bin/sh\nexec {shlex.quote(shutil.which("ssh"))} -F {shlex.quote(str(ssh_config))} "$@"\n')
    wrapper.chmod(0o755)
    env["PATH"] = str(wrapper_dir) + os.pathsep + env["PATH"]
    # A valid state ID can end in '_'; the derived guest hostname must still boot.
    identifier = "connect-check-" + root.name.rsplit("-", 1)[1] + "_"
    terminals = []
    created = False

    def run(*args, timeout=20, check=True):
        result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=timeout)
        if check:
            assert result.returncode == 0, (args, result.stdout[-4000:], result.stderr[-6000:])
        return result

    def terminal(*args):
        term = Terminal(list(args), env)
        terminals.append(term)
        return term

    def wait(check, message):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            for term in terminals:
                term.drain()
            if check():
                return
            time.sleep(0.05)
        raise AssertionError((message, [term.output[-3000:] for term in terminals]))

    try:
        run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "argos-fixture", "-f", str(key))
        for command in ("tmux", "shell", "up"):
            nested = Terminal([str(BINARY), "vm", command, "nested-check"], dict(env, TMUX="nested"))
            terminals.append(nested)
            wait(lambda: nested.process.poll() is not None, "nested VM entry did not fail")
            assert nested.process.returncode == 1 and b"original terminal" in nested.output
            assert not (state / "vms" / "nested-check.json").exists(), "nested up mutated VM state"
        run(str(BINARY), "vm", "create", "Connection test", "--id", identifier, "--config", str(config), "--json")
        created = True
        print("Building and starting disposable microVM...", flush=True)
        run(str(BINARY), "vm", "start", identifier, "--config", str(config), "--json", timeout=1200)
        record = json.loads((state / "vms" / f"{identifier}.json").read_text())
        instance = Path(record["instance_dir"])
        known_hosts = instance / "known_hosts"
        # This public-key enrollment is the existing first-use VM trust model.
        # Reconnects must preserve it rather than replacing it on every attach.
        first = terminal(str(BINARY), "vm", "tmux", identifier)
        wait(known_hosts.exists, "guest trust was not enrolled")
        guest_prefix = ["ssh", "-p", str(record["ssh_port"]), "-o", "ControlPath=none",
                        "-o", f"UserKnownHostsFile={known_hosts}", "-o", "StrictHostKeyChecking=yes", "root@127.0.0.1"]

        def guest(command, check=True):
            return run(*guest_prefix, command, check=check).stdout.strip()

        def client_count():
            return len(guest("tmux list-clients -F '#{client_pid}'", check=False).splitlines())

        wait(lambda: client_count() == 1, "vm tmux did not attach")
        pane_before = guest("tmux list-panes -a -F '#{pane_id}|#{pane_pid}'")
        assert "ARGOS_CONNECT_CLIENT=1" not in guest("tmux show-environment -g"), "marker leaked into guest server"
        pinned_key = known_hosts.read_bytes()
        first.send(b"\x1b[24~")
        wait(lambda: b"Origin-side" in first.output, "VM did not return to origin picker")
        assert client_count() == 0
        first.send(b"\x1b")
        wait(lambda: client_count() == 1, "VM reconnect failed")
        first.send(b"\x02d")
        wait(lambda: first.process.poll() is not None, "VM detach did not finish")
        assert first.process.returncode == 0 and termios.tcgetattr(first.slave) == first.original
        assert known_hosts.read_bytes() == pinned_key

        # A local-only picker and an explicit VM state root are independent of
        # the selected VM target. No inventory or ambient state fallback is needed.
        standalone = Terminal(
            [str(BINARY), "sessions", "attach", f"vm:{identifier}", "--state-dir", str(state)],
            dict(env, XDG_CONFIG_HOME=str(root / "no-inventory"), ARGOS_STATE_DIR=str(root / "wrong-state")),
        )
        terminals.append(standalone)
        wait(lambda: client_count() == 1, "VM entry without inventory failed")
        standalone.send(b"\x1b[24~")
        wait(lambda: b"Origin-side" in standalone.output, "standalone VM picker failed")
        standalone.send(b"\x1b")
        wait(lambda: client_count() == 1, "standalone VM reconnect lost state root")
        standalone.send(b"\x02d")
        wait(lambda: standalone.process.poll() is not None, "standalone VM detach failed")
        assert standalone.process.returncode == 0

        run("tmux", "-S", str(host_socket), "-f", "/dev/null", "new-session", "-d", "-s", "host-fixture", "/bin/sh")
        run("tmux", "-S", str(host_socket), "source-file", str(BINDING))
        browser = terminal(str(BINARY), "tui", "--config", str(config), "--state-dir", str(state))
        wait(lambda: b"Origin-side" in browser.output and b"Connection" in browser.output and b"READY" in browser.output, "VM row missing")
        # Last row is the isolated VM; host inventory has one other session.
        browser.send(b"j" * 100 + b"\r")
        wait(lambda: client_count() == 1, "TUI did not enter VM")
        browser.send(b"\x1b[24~")
        wait(lambda: b"Origin-side" in browser.output and b"READY" in browser.output, "VM picker handoff failed")
        browser.send(b"\r")
        wait(lambda: bool(run("tmux", "-S", str(host_socket), "list-clients", "-F", "#{client_pid}").stdout.strip()), "VM-to-host switch failed")
        browser.send(b"\x1b[24~")
        wait(lambda: b"Origin-side" in browser.output and b"READY" in browser.output, "host picker handoff failed")
        browser.send(b"j" * 100 + b"\r")
        wait(lambda: client_count() == 1, "host-to-VM switch failed")
        browser.send(b"\x02d")
        wait(lambda: browser.process.poll() is not None, "browser detach failed")
        assert browser.process.returncode == 0

        # The sessions alias and VM shell also retain Argos as the owner.
        for args in (["sessions", "attach", f"vm:{identifier}", "--state-dir", str(state)],
                     ["vm", "up", "Connection test", "--id", identifier, "--config", str(config)],
                     ["vm", "shell", identifier]):
            actor = terminal(str(BINARY), *args)
            if args[:2] != ["vm", "shell"]:
                wait(lambda: client_count() == 1, "sessions VM attach failed")
            else:
                actor.send(b"printf ready > /tmp/argos-shell-ready\r")
                wait(lambda: guest("cat /tmp/argos-shell-ready 2>/dev/null", check=False) == "ready", "guest shell not reachable")
            assert actor.process.poll() is None
            children = Path(f"/proc/{actor.process.pid}/task/{actor.process.pid}/children").read_text().split()
            assert len(children) == 1, "VM SSH not owned"
            os.kill(actor.process.pid, signal.SIGTERM)
            wait(lambda: actor.process.poll() is not None, "VM signal cleanup failed")
            assert actor.process.returncode == 143 and termios.tcgetattr(actor.slave) == actor.original
            wait(lambda: client_count() == 0, "guest tmux client survived termination")
        known_hosts.write_text("")
        untrusted = terminal(str(BINARY), "vm", "tmux", identifier)
        wait(lambda: b"failed:" in untrusted.output, "missing pinned trust was silently replaced")
        assert known_hosts.read_bytes() == b"", "reconnect overwrote pinned trust"
        untrusted.send(b"q")
        wait(lambda: untrusted.process.poll() is not None, "trust failure picker did not quit")
        known_hosts.write_bytes(pinned_key)
        assert pane_before == guest("tmux list-panes -a -F '#{pane_id}|#{pane_pid}'")
        print("PASS: real microVM tmux/shell ownership, first-server marker isolation, guest-host switching,")
        print("      reconnect, pane persistence, pinned trust, and signal cleanup.")
    finally:
        for term in terminals:
            term.close()
        stopped = not created or run(str(BINARY), "vm", "stop", identifier, "--json", timeout=60, check=False).returncode == 0
        run("tmux", "-N", "-S", str(host_socket), "kill-server", check=False)
        # TMUX_TMPDIR is fixture-private, including the VM console server.
        run("tmux", "kill-server", check=False)
        if stopped:
            shutil.rmtree(root)
        else:
            print(f"VM cleanup failed; preserving test state at {root}", flush=True)


if __name__ == "__main__":
    main()
