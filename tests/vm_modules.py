#!/usr/bin/env python3
"""Profile module snapshots and clean-source boundary. No real VM is booted."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def main():
    with tempfile.TemporaryDirectory(prefix="argos-mod-", dir=os.environ.get("XDG_RUNTIME_DIR", "/tmp")) as directory:
        root = Path(directory)
        state = root / "state"
        home = root / "home"
        home.mkdir()
        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"))

        def run(*args, check=True):
            result = subprocess.run([str(BINARY), *args, "--state-dir", str(state), "--json"],
                                    env=env, text=True, capture_output=True, timeout=20)
            if check:
                assert result.returncode == 0, (result.stdout, result.stderr)
            return result

        bundle = root / "bundle with space"
        bundle.mkdir()
        (bundle / "default.nix").write_text('{ imports = [ ./extra.nix ]; }\n')
        (bundle / "extra.nix").write_text('{ environment.variables.MODULE_TEST = "old"; }\n')
        plain = root / "plain.nix"
        plain.write_text('{ environment.variables.PLAIN_TEST = "yes"; }\n')
        profile = root / "profile.toml"
        profile.write_text('[vm]\nmodules = ["bundle with space", "plain.nix"]\n')
        vm_id = f"module-test-{os.getpid()}"
        args = ["vm", "create", "Module test", "--id", vm_id, "--profile", str(profile)]
        dry = run(*args, "--dry-run")
        assert not state.exists(), "dry-run wrote VM state"
        assert json.loads(dry.stdout)["record"]["guest_modules"] == [str(bundle), str(plain)]
        result = json.loads(run(*args).stdout)
        instance = Path(result["instance_dir"])
        snapshot = instance / "guest-modules"
        assert (snapshot / "0/extra.nix").read_text().find('"old"') >= 0
        assert (snapshot / "1/default.nix").read_text() == plain.read_text()
        flake = (instance / "flake.nix").read_text()
        assert "./guest-modules/0/default.nix" in flake
        assert "./guest-modules/1/default.nix" in flake
        (bundle / "extra.nix").write_text('{ environment.variables.MODULE_TEST = "new"; }\n')
        assert '"old"' in (snapshot / "0/extra.nix").read_text(), "start inputs are not immutable snapshots"
        run("vm", "update", vm_id, "--profile", str(profile))
        assert '"new"' in (snapshot / "0/extra.nix").read_text()
        assert "./guest-modules/1/default.nix" in (instance / "flake.nix").read_text()

        # Dry-run must reject special files buried in a module bundle, without
        # changing the previously staged inputs or record.
        before = (state / "vms" / f"{vm_id}.json").read_bytes()
        os.symlink(plain, bundle / "forbidden.nix")
        bad = run("vm", "update", vm_id, "--profile", str(profile), "--dry-run", check=False)
        assert bad.returncode != 0 and "symlink" in bad.stderr, bad.stderr
        assert (state / "vms" / f"{vm_id}.json").read_bytes() == before
        (bundle / "forbidden.nix").unlink()

        # Starts consume only captured modules, even when original sources move.
        bundle.rename(root / "moved bundle")
        plain.unlink()

        # Exercise the CLI's actual Nix build boundary with a failing adapter.
        # It inspects the filesystem passed to Nix; it does not simulate boot.
        fakebin = root / "bin"
        fakebin.mkdir()
        observed = root / "observed.json"
        nix = fakebin / "nix"
        nix.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
reference = sys.argv[2]
assert reference.startswith("path:"), reference
source = pathlib.Path(reference[5:].split("#")[0]).resolve()
pathlib.Path(os.environ["OBSERVED"]).write_text(json.dumps({"path": str(source), "files": sorted(str(p.relative_to(source)) for p in source.rglob("*"))}))
raise SystemExit(1)
''')
        nix.chmod(0o755)
        env.update(PATH=str(fakebin) + os.pathsep + env["PATH"], OBSERVED=str(observed))
        (instance / "var.img").write_bytes(b"persistent disk")
        (instance / "known_hosts").write_text("must not enter Nix\n")
        (instance / ".git").mkdir()
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(instance / "control.socket"))
            failure = run("vm", "start", vm_id, check=False)
            assert failure.returncode != 0, failure.stdout
        observation = json.loads(observed.read_text())
        files = observation["files"]
        assert "flake.nix" in files and "guest-modules/0/extra.nix" in files, files
        assert not {"var.img", "known_hosts", ".git", "control.socket"}.intersection(files), files
        assert not Path(observation["path"]).exists(), "failed build left source staging behind"
        assert (instance / "var.img").read_bytes() == b"persistent disk"

        profile.unlink()
        missing_profile = run("vm", "update", vm_id, "--dry-run", check=False)
        assert missing_profile.returncode != 0 and "profile is unavailable" in missing_profile.stderr
        assert (snapshot / "0/extra.nix").is_file()

        # Old schema-1 records need no migration before read-only discovery.
        record_path = state / "vms" / f"{vm_id}.json"
        record = json.loads(record_path.read_text())
        record.pop("guest_modules")
        record_path.write_text(json.dumps(record))
        assert json.loads(run("vm", "show", vm_id).stdout)["record"]["guest_modules"] == []
    print("VM module and clean source checks passed")


if __name__ == "__main__":
    main()
