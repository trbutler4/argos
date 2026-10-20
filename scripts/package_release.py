#!/usr/bin/env python3
"""Package the standalone Nix output, refusing wrapper/dynamically linked binaries."""
import argparse
import gzip
import hashlib
import io
from pathlib import Path
import struct
import subprocess
import tarfile

from release import ROOT, validate

PLATFORM = "x86_64-linux"
DATA_FILES = (
    "share/argos/connect.tmux.conf",
    "share/bash-completion/completions/argos.bash",
    "share/zsh/site-functions/_argos",
    "share/fish/vendor_completions.d/argos.fish",
)


def check_static_elf(data):
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("release binary must be a 64-bit little-endian ELF, not a wrapper script")
    if struct.unpack_from("<H", data, 18)[0] != 62:
        raise ValueError("release binary must target x86-64")
    offset = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    if size < 56 or not count or offset + size * count > len(data):
        raise ValueError("invalid ELF program headers")
    for index in range(count):
        header = offset + index * size
        kind = struct.unpack_from("<I", data, header)[0]
        if kind == 3:  # PT_INTERP
            raise ValueError("release binary has a dynamic/store-path interpreter")
        if kind == 2:  # PT_DYNAMIC (static PIE may still have relocation metadata)
            start = struct.unpack_from("<Q", data, header + 8)[0]
            length = struct.unpack_from("<Q", data, header + 32)[0]
            if start + length > len(data) or length % 16:
                raise ValueError("invalid ELF dynamic segment")
            for entry in range(start, start + length, 16):
                tag = struct.unpack_from("<q", data, entry)[0]
                if tag == 0:
                    break
                if tag == 1:  # DT_NEEDED
                    raise ValueError("release binary depends on shared libraries")


def package_release(package, output, root=ROOT):
    version, _ = validate(root)
    binary = package / "bin/argos"
    check_static_elf(binary.read_bytes())
    actual = subprocess.check_output([str(binary), "--version"], text=True).strip()
    if actual != f"argos {version}":
        raise ValueError(f"binary version mismatch: expected argos {version}, got {actual!r}")
    files = {"bin/argos": binary.read_bytes()}
    for name in DATA_FILES:
        files[name] = (package / name).read_bytes()
    files["README.md"] = (root / "docs/binary-release.md").read_bytes()
    files["CHANGELOG.md"] = (root / "CHANGELOG.md").read_bytes()
    files["VERSION"] = (version + "\n").encode()
    prefix = f"argos-v{version}-{PLATFORM}"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{prefix}.tar.gz"
    # Stable ordering, ownership, modes, and timestamps make reruns reproducible.
    with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as tar:
            for name, data in sorted(files.items()):
                info = tarfile.TarInfo(f"{prefix}/{name}")
                info.size = len(data)
                info.mode = 0o755 if name == "bin/argos" else 0o644
                tar.addfile(info, io.BytesIO(data))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + ".sha256").write_text(f"{checksum}  {archive.name}\n")
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    try:
        print(package_release(args.package.resolve(), args.output))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
