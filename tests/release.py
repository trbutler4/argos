#!/usr/bin/env python3
"""Release metadata and packaging regressions; all edits use temporary fixtures."""
from pathlib import Path
import struct
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import release
from package_release import check_static_elf


class MetadataTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="argos-release-metadata-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "Cargo.toml").write_text('[package]\nname = "argos"\nversion = "0.2.0"\n\n[dependencies]\nexample = "0.2.0"\n')
        (self.root / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "argos"\nversion = "0.2.0"\n\n[[package]]\nname = "example"\nversion = "0.2.0"\nsource = "registry+fixture"\n')
        (self.root / "CHANGELOG.md").write_text('# Changelog\n\n## [Unreleased]\n\n- Fix reconnect.\n\n## [0.2.0]\n\n- Managed connections.\n')

    def test_current_release_validates(self):
        self.assertEqual(release.validate(self.root, ["v0.1.0"], "0.1.0")[0], "0.2.0")

    def test_normal_development_at_published_version_is_allowed(self):
        release.validate(self.root, ["v0.2.0"], "0.2.0")

    def test_same_commit_can_resume_a_partially_published_release(self):
        release.validate(self.root, ["v0.2.0"], "0.1.0", tag_at_head=True)

    def test_code_changes_need_unreleased_notes_or_a_version_bump(self):
        release.validate(self.root, ["v0.2.0"], "0.2.0", code_changed=True)
        path = self.root / "CHANGELOG.md"
        path.write_text(path.read_text().replace("- Fix reconnect.", ""))
        with self.assertRaisesRegex(ValueError, "record code/build changes"):
            release.validate(self.root, ["v0.2.0"], "0.2.0", code_changed=True)
        release.validate(self.root, ["v0.1.0"], "0.1.0", code_changed=True)

    def test_lock_mismatch_is_rejected(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace('version = "0.2.0"', 'version = "0.1.0"', 1))
        with self.assertRaisesRegex(ValueError, "must match"):
            release.validate(self.root)

    def test_versions_cannot_go_backwards_or_reuse_a_release(self):
        for tags, base in ((["v0.3.0"], None), ([], "0.3.0"), (["v0.2.0"], "0.1.0")):
            with self.assertRaises(ValueError):
                release.validate(self.root, tags, base)

    def test_notes_are_required(self):
        for notes in ("", "### Fixed\n", "- TODO: write notes"):
            (self.root / "CHANGELOG.md").write_text(f"## [Unreleased]\n\n## [0.2.0]\n\n{notes}\n")
            with self.assertRaises(ValueError):
                release.validate(self.root)

    def test_duplicate_release_heading_is_rejected(self):
        path = self.root / "CHANGELOG.md"
        path.write_text(path.read_text() + "\n## [0.2.0]\n\n- Duplicate.\n")
        with self.assertRaisesRegex(ValueError, "exactly one"):
            release.validate(self.root)

    def test_bump_updates_only_root_versions_and_moves_notes(self):
        release.bump(self.root, "0.3.0", ["v0.2.0"])
        manifest = tomllib.loads((self.root / "Cargo.toml").read_text())
        lock = tomllib.loads((self.root / "Cargo.lock").read_text())
        self.assertEqual(manifest["package"]["version"], "0.3.0")
        self.assertEqual(manifest["dependencies"]["example"], "0.2.0")
        self.assertEqual([p["version"] for p in lock["package"]], ["0.3.0", "0.2.0"])
        version, notes = release.validate(self.root, ["v0.2.0"], "0.2.0")
        self.assertEqual((version, notes), ("0.3.0", "- Fix reconnect."))
        self.assertEqual(release.section((self.root / "CHANGELOG.md").read_text(), "Unreleased")[2], "")

    def test_versions_are_ordered_numerically(self):
        release.bump(self.root, "0.2.10", ["v0.2.9"])
        self.assertEqual(release.validate(self.root, ["v0.2.9"], "0.2.0")[0], "0.2.10")

    def test_failed_bump_leaves_files_untouched(self):
        before = {path: path.read_bytes() for path in self.root.iterdir()}
        for value in ("0.1.9", "0.2.0", "0.3.0-rc.1", "01.0.0", "0.3"):
            with self.assertRaises(ValueError):
                release.bump(self.root, value)
            self.assertEqual(before, {path: path.read_bytes() for path in self.root.iterdir()})


class BinaryTests(unittest.TestCase):
    @staticmethod
    def elf(kind=1, machine=62):
        data = bytearray(120)
        data[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<H", data, 18, machine)
        struct.pack_into("<Q", data, 32, 64)
        struct.pack_into("<HH", data, 54, 56, 1)
        struct.pack_into("<I", data, 64, kind)
        return data

    def test_nix_wrapper_is_not_a_binary(self):
        with self.assertRaisesRegex(ValueError, "not a wrapper"):
            check_static_elf(b"#!/bin/sh\nexec /nix/store/example/bin/argos \"$@\"\n")

    def test_dynamic_interpreter_and_wrong_arch_are_rejected(self):
        for data in (self.elf(kind=3), self.elf(machine=183)):
            with self.assertRaises(ValueError):
                check_static_elf(data)

    def test_shared_library_dependency_is_rejected(self):
        data = self.elf(kind=2) + bytearray(16)
        struct.pack_into("<Q", data, 72, 120)
        struct.pack_into("<Q", data, 96, 16)
        struct.pack_into("<q", data, 120, 1)
        with self.assertRaisesRegex(ValueError, "shared libraries"):
            check_static_elf(data)

    def test_static_header_is_accepted(self):
        check_static_elf(self.elf())


if __name__ == "__main__":
    unittest.main()
