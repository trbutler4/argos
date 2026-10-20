#!/usr/bin/env python3
"""Small release metadata tool: validate, bump, or print curated notes.

Cargo.toml is authoritative. This tool never commits, tags, or publishes.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
HEADING = re.compile(r"^## \[([^\]]+)\]\s*$", re.MULTILINE)


def version_key(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError(f"expected a stable MAJOR.MINOR.PATCH version, got {value!r}")
    return tuple(map(int, match.groups()))


def section(text, name):
    headings = list(HEADING.finditer(text))
    matches = [i for i, heading in enumerate(headings) if heading.group(1) == name]
    if len(matches) != 1:
        raise ValueError(f"CHANGELOG.md must contain exactly one ## [{name}] heading")
    index = matches[0]
    start = headings[index].end()
    end = headings[index + 1].start() if index + 1 < len(headings) else len(text)
    return start, end, text[start:end].strip()


def require_notes(notes):
    if not notes or not re.search(r"^[-*] \S", notes, re.MULTILINE):
        raise ValueError("release needs user-visible changelog entries")
    if re.search(r"\b(TODO|TBD)\b", notes):
        raise ValueError("release changelog still contains TODO/TBD placeholders")


def metadata(root):
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    package = manifest["package"]
    version = package["version"]
    version_key(version)
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    roots = [p for p in lock["package"] if p["name"] == package["name"] and "source" not in p]
    if len(roots) != 1 or roots[0]["version"] != version:
        raise ValueError("Cargo.lock root package version must match Cargo.toml")
    return package["name"], version


def validate(root, tags=(), base_version=None, *, tag_at_head=False, code_changed=False):
    _, version = metadata(root)
    key = version_key(version)
    if base_version is not None and key < version_key(base_version):
        raise ValueError(f"version decreased from {base_version} to {version}")
    released = [version_key(tag[1:]) for tag in tags if tag.startswith("v") and VERSION.fullmatch(tag[1:])]
    if released and key < max(released):
        raise ValueError("version is older than an existing stable release")
    if base_version is not None and version != base_version and f"v{version}" in tags and not tag_at_head:
        raise ValueError("a version bump must use a new, unreleased version")
    changelog = (root / "CHANGELOG.md").read_text()
    _, _, unreleased = section(changelog, "Unreleased")
    if code_changed and version == base_version:
        try:
            require_notes(unreleased)
        except ValueError as error:
            raise ValueError("record code/build changes under Unreleased or prepare a version bump") from error
    _, _, notes = section(changelog, version)
    require_notes(notes)
    return version, notes


def bump(root, new_version, tags=()):
    name, current = metadata(root)
    if version_key(new_version) <= version_key(current):
        raise ValueError("new version must be greater than the current version")
    released = [version_key(tag[1:]) for tag in tags if tag.startswith("v") and VERSION.fullmatch(tag[1:])]
    if released and version_key(new_version) <= max(released):
        raise ValueError("new version must be greater than published versions")
    changelog = (root / "CHANGELOG.md").read_text()
    if any(h.group(1) == new_version for h in HEADING.finditer(changelog)):
        raise ValueError("changelog already contains the requested version")
    start, end, notes = section(changelog, "Unreleased")
    require_notes(notes)
    changelog = changelog[:start] + f"\n\n## [{new_version}]\n\n{notes}\n\n" + changelog[end:]

    # Preserve manifest formatting and every dependency version.
    manifest = (root / "Cargo.toml").read_text()
    package_block = re.search(r"(?ms)^\[package\]\s*\n.*?(?=^\[|\Z)", manifest)
    if package_block is None:
        raise ValueError("Cargo.toml is missing [package]")
    updated, count = re.subn(r'(?m)^(\s*version\s*=\s*)"[^"]+"', rf'\g<1>"{new_version}"', package_block.group())
    if count != 1:
        raise ValueError("cannot identify package version in Cargo.toml")
    manifest = manifest[:package_block.start()] + updated + manifest[package_block.end():]
    lock = (root / "Cargo.lock").read_text()
    blocks = list(re.finditer(r"(?ms)^\[\[package\]\]\s*\n.*?(?=^\[\[package\]\]|\Z)", lock))
    for block in blocks:
        value = tomllib.loads(block.group())["package"][0]
        if value["name"] == name and "source" not in value:
            updated, count = re.subn(r'(?m)^(version\s*=\s*)"[^"]+"', rf'\g<1>"{new_version}"', block.group())
            if count != 1:
                raise ValueError("cannot identify root package version in Cargo.lock")
            lock = lock[:block.start()] + updated + lock[block.end():]
            break
    else:
        raise ValueError("cannot identify root package in Cargo.lock")
    # Validate all edits before changing any files; Git remains the review boundary.
    for filename, contents in (("Cargo.toml", manifest), ("Cargo.lock", lock), ("CHANGELOG.md", changelog)):
        (root / filename).write_text(contents)
    return new_version


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check", help="validate versions, changelog, and release ordering")
    check.add_argument("--base-ref", help="also compare with this PR base commit")
    prepare = commands.add_parser("bump", help="move Unreleased notes and update Cargo versions")
    prepare.add_argument("version")
    commands.add_parser("notes", help="print the current release notes")
    args = parser.parse_args()
    try:
        tags = git("tag", "--list", "v*").splitlines()
        if args.command == "bump":
            print(f"Prepared {bump(ROOT, args.version, tags)}; review and commit the three changed files.")
            return
        base = None
        code_changed = False
        if args.command == "check" and args.base_ref:
            commit = git("rev-parse", "--verify", "--end-of-options", args.base_ref + "^{commit}")
            base = tomllib.loads(git("show", f"{commit}:Cargo.toml"))["package"]["version"]
            changed = git("diff", "--name-only", commit, "--").splitlines()
            code_changed = any(path.startswith("src/") or path in {"Cargo.toml", "Cargo.lock", "flake.nix", "flake.lock"} for path in changed)
        tag = f"v{metadata(ROOT)[1]}"
        tag_at_head = tag in tags and git("rev-parse", f"refs/tags/{tag}^{{commit}}") == git("rev-parse", "HEAD")
        version, notes = validate(ROOT, tags, base, tag_at_head=tag_at_head, code_changed=code_changed)
        if args.command == "notes":
            print(f"# Argos {version}\n\n{notes}")
        else:
            print(f"Validated Argos {version} (v{version})")
            plan = (f"v{version} is already tagged; a new binary release needs a version bump."
                    if tag in tags and not tag_at_head else f"v{version} is ready for publication or recovery on main.")
            print(plan)
            if path := os.environ.get("GITHUB_STEP_SUMMARY"):
                with open(path, "a") as summary:
                    summary.write(f"## Version plan\n\n{plan}\n")
            if path := os.environ.get("GITHUB_OUTPUT"):
                with open(path, "a") as output:
                    output.write(f"version={version}\ntag=v{version}\n")
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
