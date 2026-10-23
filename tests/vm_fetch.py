#!/usr/bin/env python3
"""Disposable real loopback SSH acceptance tests for verified VM file retrieval."""
import hashlib
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


def run(*args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, timeout=20, **kwargs)


def main():
    for tool in ("sshd", "ssh", "ssh-keygen", "sha256sum"):
        assert shutil.which(tool), f"missing {tool}, run inside nix develop"
    with tempfile.TemporaryDirectory(prefix="argos-fetch-", dir=os.environ.get("XDG_RUNTIME_DIR")) as directory:
        root = Path(directory)
        state = root / "state"
        instance = state / "instances" / "fetch-test"
        instance.mkdir(parents=True)
        (state / "vms").mkdir()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        user = pwd.getpwuid(os.getuid()).pw_name
        for name in ("server", "client", "wrong"):
            result = run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root / name))
            assert result.returncode == 0, result.stderr
        authorized = root / "authorized_keys"
        authorized.write_text((root / "client.pub").read_text())
        trust = instance / "known_hosts"
        def trust_key(name):
            fields = (root / f"{name}.pub").read_text().split()
            return f"[127.0.0.1]:{port} {fields[0]} {fields[1]}\n"
        trusted = trust_key("server")
        trust.write_text(trusted)
        config = root / "sshd_config"
        config.write_text(
            f"HostKey {root / 'server'}\nAuthorizedKeysFile {authorized}\n"
            "UsePAM no\nStrictModes no\nPasswordAuthentication no\n"
            f"AuthenticationMethods publickey\nListenAddress 127.0.0.1\nPort {port}\n"
            "LogLevel ERROR\nPidFile none\n"
        )
        source = root / "guest α ' \" ; $(printf should-not-expand) | file.apk "
        payload = bytes(range(256)) * 32768  # Larger than bounded metadata capture.
        source.write_bytes(payload)
        record = dict(schema_version=1, id="fetch-test", name="fetch-test", status="running", host="local",
                      instance_dir=str(instance), ssh_host="127.0.0.1", ssh_port=port, ssh_user=user)
        state_file = state / "vms" / "fetch-test.json"
        state_file.write_text(json.dumps(record))
        wrapper_dir = root / "bin"
        wrapper_dir.mkdir()
        actual_ssh = shutil.which("ssh")
        # Only inject a disposable identity. Actual OpenSSH still enforces the command's trust options.
        wrapper = wrapper_dir / "ssh"
        wrapper.write_text(
            "#!/usr/bin/env python3\nimport os,sys\nfrom pathlib import Path\n"
            "mode=os.environ.get('FETCH_TEST_MODE','')\n"
            "command=sys.argv[-1]\n"
            "if mode == 'failed' and 'exec cat --' in command:\n"
            " sys.stdout.write('partial'); sys.exit(23)\n"
            "if mode == 'corrupt' and 'exec cat --' in command:\n"
            " sys.stdout.write('corrupted'); sys.exit(0)\n"
            "if mode == 'changed' and 'exec cat --' in command:\n"
            " Path(os.environ['FETCH_TEST_SOURCE']).write_bytes(b'changed')\n"
            "if mode == 'race' and 'exec cat --' in command:\n"
            " Path(os.environ['FETCH_TEST_OUTPUT']).write_bytes(b'competitor')\n"
            f"os.execv({actual_ssh!r}, [{actual_ssh!r}, '-i', {str(root / 'client')!r}, '-o', 'IdentitiesOnly=yes'] + sys.argv[1:])\n"
        )
        wrapper.chmod(0o755)
        env = dict(os.environ, PATH=str(wrapper_dir) + os.pathsep + os.environ["PATH"], FETCH_TEST_SOURCE=str(source))
        output = root / "result α ' \".apk"
        env["FETCH_TEST_OUTPUT"] = str(output)

        def fetch(path=source, code=0, mode="", vm_id="fetch-test", state_dir=state):
            result = run(str(BINARY), "vm", "fetch", vm_id, str(path), "--output", str(output),
                         "--state-dir", str(state_dir), "--json",
                         env=dict(env, FETCH_TEST_MODE=mode))
            assert result.returncode == code, (result.returncode, result.stdout, result.stderr)
            if code:
                assert not result.stdout, result.stdout
            assert not list(root.glob(".argos-fetch-*.tmp")), "leaked transfer temporary"
            return result

        daemon = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(config)],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(40):
                assert daemon.poll() is None, daemon.stderr.read()
                probe = run(actual_ssh, "-F", "/dev/null", "-i", str(root / "client"), "-o", "IdentitiesOnly=yes",
                            "-o", "StrictHostKeyChecking=yes", "-o", f"UserKnownHostsFile={trust}",
                            "-p", str(port), f"{user}@127.0.0.1", "true")
                if probe.returncode == 0:
                    break
                time.sleep(0.1)
            else:
                raise AssertionError(probe.stderr)

            fetch("relative.apk", code=2)
            fetch("/bad\npath", code=2)
            fetch(state_dir="relative", code=2)
            fetch(vm_id="bad/id", code=2)
            fetch("/bad\tpath", code=2)
            result = fetch()
            value = json.loads(result.stdout)
            assert value["schema_version"] == 1
            assert value["sha256"] == hashlib.sha256(payload).hexdigest()
            assert output.read_bytes() == payload
            assert output.stat().st_mode & 0o777 == 0o600
            fetch(code=1)
            assert output.read_bytes() == payload
            output.unlink()
            output.symlink_to(root / "absent")
            fetch(code=1)
            assert output.is_symlink() and not (root / "absent").exists()
            output.unlink()
            fetch(code=1, mode="failed")
            assert not output.exists()
            fetch(code=1, mode="corrupt")
            assert not output.exists()
            fetch(code=1, mode="changed")
            assert not output.exists()
            source.write_bytes(payload)
            fetch(code=1, mode="race")
            assert output.read_bytes() == b"competitor"
            output.unlink()
            fetch(root, code=1)  # Directories are not artifacts.
            trust.write_text(trust_key("wrong"))
            before = trust.read_bytes()
            fetch(code=1)
            assert trust.read_bytes() == before and not output.exists()
            trust.write_text("")
            fetch(code=1)
            assert trust.read_bytes() == b""
            trust.unlink()
            trust.symlink_to(root / "server.pub")
            fetch(code=1)
            assert trust.is_symlink()
            trust.unlink()
            fetch(code=1)
            assert not trust.exists() and not output.exists()
            trust.write_text(trusted)
            for host in ("192.0.2.1", "localhost", "::1", "127.0.0.1 -o ProxyCommand=false"):
                record["ssh_host"] = host
                state_file.write_text(json.dumps(record))
                fetch(code=1)
            record["ssh_host"] = "127.0.0.1"
            record["status"] = "stopped"
            state_file.write_text(json.dumps(record))
            fetch(code=1)
            assert json.loads(state_file.read_text())["status"] == "stopped"
            assert trust.read_text() == trusted
        finally:
            daemon.terminate()
            daemon.wait(timeout=5)
    print("vm fetch: real SSH verification, quoting, trust and cleanup passed")


if __name__ == "__main__":
    main()
