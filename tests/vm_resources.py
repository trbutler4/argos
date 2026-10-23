#!/usr/bin/env python3
"""Focused VM resource profile, persistence, and disk guard checks."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos")).resolve()


def run(*args):
    return subprocess.run(
        [str(BINARY), *args], capture_output=True, text=True, timeout=10
    )


def create(root, name, profile=None):
    args = [
        "vm", "create", name, "--state-dir", str(root / "state"),
        "--work-root", str(root / "work"), "--json",
    ]
    if profile:
        args += ["--profile", str(profile)]
    result = run(*args)
    assert result.returncode == 0, (args, result.returncode, result.stdout, result.stderr)
    return json.loads(result.stdout)


def base_profile(resources):
    return "[vm.resources]\n" + "\n".join(f"{key} = {value}" for key, value in resources.items()) + "\n"


def record(root, vm_id):
    path = root / "state" / "vms" / f"{vm_id}.json"
    return path, json.loads(path.read_text())


def main():
    assert BINARY.is_file(), "Run cargo build --locked first"
    with tempfile.TemporaryDirectory(prefix="argos-vm-resources-") as temp:
        root = Path(temp)
        os.environ["XDG_CONFIG_HOME"] = str(root / "xdg")

        partial = root / "partial.argos.toml"
        partial.write_text(base_profile({"cpus": 2}))
        partial_value = create(root, "Partial", partial)
        assert partial_value["record"]["resources"] == {
            "cpus": 2, "memory_mib": 4096, "var_disk_mib": 1024, "store_disk_mib": 8192
        }

        custom = root / "custom.argos.toml"
        custom.write_text(base_profile({
            "cpus": 8, "memory_mib": 12288, "var_disk_mib": 64, "store_disk_mib": 128
        }))
        custom_value = create(root, "Custom", custom)
        assert custom_value["record"]["resources"] == {
            "cpus": 8, "memory_mib": 12288, "var_disk_mib": 64, "store_disk_mib": 128
        }
        custom_id = custom_value["record"]["id"]

        for field, value in (("cpus", 0), ("memory_mib", 0), ("var_disk_mib", 0), ("store_disk_mib", 0),
                             ("cpus", 257), ("memory_mib", 1048577), ("var_disk_mib", 4194305), ("store_disk_mib", 4194305)):
            bad = root / f"bad-{field}-{value}.toml"
            bad.write_text(base_profile({field: value}))
            result = run("vm", "create", f"bad-{field}-{value}", "--state-dir", str(root / "state"),
                         "--work-root", str(root / "work"), "--profile", str(bad), "--json")
            assert result.returncode != 0, (field, value, result.stdout, result.stderr)

        state_path, before = record(root, custom_id)
        instance = Path(before["instance_dir"])
        update_profile = root / "update.argos.toml"
        update_profile.write_text(base_profile({"cpus": 6, "memory_mib": 8192, "var_disk_mib": 96, "store_disk_mib": 192}))
        allowed = run("vm", "update", custom_id, "--state-dir", str(root / "state"),
                      "--profile", str(update_profile), "--dry-run", "--json")
        assert allowed.returncode == 0, (allowed.stdout, allowed.stderr)
        assert json.loads(state_path.read_text()) == before, "dry-run mutated state"
        updated = run("vm", "update", custom_id, "--state-dir", str(root / "state"),
                      "--profile", str(update_profile), "--json")
        assert updated.returncode == 0, (updated.stdout, updated.stderr)
        assert json.loads(updated.stdout)["record"]["resources"]["cpus"] == 6
        assert json.loads(updated.stdout)["record"]["resources"]["memory_mib"] == 8192

        # Existing images with a changed allocation are rejected, including dry-run,
        # and the state file remains byte-for-byte unchanged.
        before_guard = state_path.read_bytes()
        guard_profile = root / "guard.argos.toml"
        guard_profile.write_text(base_profile({"cpus": 6, "memory_mib": 8192, "var_disk_mib": 97, "store_disk_mib": 193}))
        (instance / "var.img").write_bytes(b"x")
        (instance / "nix-store-overlay.img").write_bytes(b"y")
        refused = run("vm", "update", custom_id, "--state-dir", str(root / "state"),
                      "--profile", str(guard_profile), "--dry-run", "--json")
        assert refused.returncode != 0 and "does not resize disks" in refused.stderr
        assert state_path.read_bytes() == before_guard
        refused = run("vm", "update", custom_id, "--state-dir", str(root / "state"),
                      "--profile", str(guard_profile), "--json")
        assert refused.returncode != 0 and "does not resize disks" in refused.stderr
        assert state_path.read_bytes() == before_guard

        # CPU/RAM changes are allowed when disk images exist if allocations stay fixed.
        cpu_ram = root / "cpu-ram.argos.toml"
        cpu_ram.write_text(base_profile({"cpus": 7, "memory_mib": 16384, "var_disk_mib": 96, "store_disk_mib": 192}))
        allowed = run("vm", "update", custom_id, "--state-dir", str(root / "state"),
                      "--profile", str(cpu_ram), "--json")
        assert allowed.returncode == 0, (allowed.stdout, allowed.stderr)

        # Exact raw byte lengths permit adopting pre-existing guest-module images.
        adopt = root / "adopt.argos.toml"
        adopt.write_text(base_profile({"cpus": 7, "memory_mib": 16384, "var_disk_mib": 2, "store_disk_mib": 3}))
        (instance / "var.img").write_bytes(b"x" * (2 * 1024 * 1024))
        (instance / "nix-store-overlay.img").write_bytes(b"y" * (3 * 1024 * 1024))
        adopted = run("vm", "update", custom_id, "--state-dir", str(root / "state"),
                      "--profile", str(adopt), "--json")
        assert adopted.returncode == 0, (adopted.stdout, adopted.stderr)

        legacy = root / "legacy-state"
        (legacy / "vms").mkdir(parents=True)
        legacy_record = {
            "schema_version": 1, "id": "legacy", "name": "Legacy", "status": "stopped", "host": "test"
        }
        (legacy / "vms" / "legacy.json").write_text(json.dumps(legacy_record))
        shown = run("vm", "show", "legacy", "--state-dir", str(legacy), "--json")
        assert shown.returncode == 0, (shown.stdout, shown.stderr)
        assert json.loads(shown.stdout)["record"]["resources"] == {
            "cpus": 4, "memory_mib": 4096, "var_disk_mib": 1024, "store_disk_mib": 8192
        }

    print("PASS: VM resource defaults, custom values, bounds, update guards, disk adoption, CPU/RAM updates, and legacy state")


if __name__ == "__main__":
    main()
