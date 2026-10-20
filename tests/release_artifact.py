#!/usr/bin/env python3
"""Verify the downloadable archive, then optionally exercise its real binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from package_release import DATA_FILES, PLATFORM, check_static_elf, package_release
from release import metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--integration", action="store_true")
    args = parser.parse_args()
    _, version = metadata(ROOT)
    prefix = f"argos-v{version}-{PLATFORM}"
    checksum = args.archive.with_suffix(args.archive.suffix + ".sha256").read_text().split()
    assert checksum == [hashlib.sha256(args.archive.read_bytes()).hexdigest(), args.archive.name]
    with tempfile.TemporaryDirectory(prefix="argos-release-") as directory:
        root = Path(directory)
        with tarfile.open(args.archive) as archive:
            members = archive.getmembers()
            expected = {f"{prefix}/{name}" for name in ("bin/argos", *DATA_FILES, "README.md", "CHANGELOG.md", "VERSION")}
            assert {member.name for member in members} == expected
            assert len(members) == len(expected) and all(member.isfile() for member in members)
            archive.extractall(root, filter="data")
        extracted = root / prefix
        binary = extracted / "bin/argos"
        check_static_elf(binary.read_bytes())
        assert binary.stat().st_mode & 0o111
        assert (extracted / "VERSION").read_text().strip() == version
        # No Nix PATH, loader variables, personal inventory, or inherited tmux/SSH
        # context. This must work as an ordinary downloaded executable.
        env = {"HOME": str(root), "XDG_CONFIG_HOME": str(root / "config"), "PATH": "/usr/bin:/bin", "LANG": "C"}
        actual = subprocess.check_output([str(binary), "--version"], env=env, text=True)
        assert actual.strip() == f"argos {version}"
        help_text = subprocess.check_output([str(binary), "--help"], env=env, text=True)
        assert "sessions" in help_text and "tui" in help_text
        status = json.loads(subprocess.check_output([str(binary), "host", "status", "--json"], env=env, text=True))
        assert status["argos_version"] == version
        for name in DATA_FILES:
            text = (extracted / name).read_text()
            assert text.strip() and "/nix/store/" not in text, f"store-path dependency in {name}"
        repacked = package_release(extracted, root / "repacked")
        assert repacked.read_bytes() == args.archive.read_bytes(), "archive is not reproducible"
        if args.integration:
            env = dict(os.environ, ARGOS_BINARY=str(binary), ARGOS_TMUX_BINDING=str(extracted / "share/argos/connect.tmux.conf"))
            for name in ("local_tmux.py", "config_cli.py", "remote_ssh.py", "attach_tmux.py", "connect.py", "tui.py", "completions.py"):
                subprocess.run([sys.executable, str(ROOT / "tests" / name)], env=env, check=True)
    print("PASS: standalone static ELF, correct version, archive contents/checksum, and clean-environment execution.")


if __name__ == "__main__":
    main()
