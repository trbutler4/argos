# Versioning and releases

`Cargo.toml` is the authoritative application version. `Cargo.lock` must contain
the same root-package version; both Nix packages read the manifest directly.
Inventory, JSON, and persisted VM schema versions are separate compatibility
contracts and are not bumped merely because the application version changes.

## Version policy

Publish stable `MAJOR.MINOR.PATCH` versions with immutable `vMAJOR.MINOR.PATCH`
tags. Prerelease/build-metadata versions are not part of this initial workflow.

| Change | Before 1.0 | From 1.0 onward |
| --- | --- | --- |
| Compatible fixes, documentation, packaging | PATCH | PATCH |
| New capabilities | MINOR | MINOR |
| Breaking CLI/config/workflow changes | MINOR, with migration notes | MAJOR, with migration notes |

For example, 0.2.0 introduces mandatory connection ownership and rejects nested
entry, so it is a minor release rather than a patch to 0.1.0. Release versions
must increase; tags and published binary assets are never moved or overwritten.

## Normal development

Collect concise, user-visible notes in `CHANGELOG.md` under `## [Unreleased]`.
CI requires notes or a version bump when Rust code or Cargo/Nix build inputs
change relative to the PR base. Documentation-only changes need not cut a release.
Commit messages remain ordinary imperative descriptions; no conventional-commit
parser or version inference service is used.

Main can contain unreleased work at the current package version. CI still
validates and builds it, but an existing published tag does not produce another
release. The workflow summary explains whether a version bump is needed.

## Prepare a release on a branch

After adding Unreleased notes, choose the next version explicitly:

```sh
python3 scripts/release.py bump 0.2.1
python3 scripts/release.py check --base-ref origin/main
```

The bump command updates only the root versions in `Cargo.toml` and `Cargo.lock`,
and moves Unreleased notes into the new changelog section. Review and commit all
three files. It does not commit, tag, push, or publish anything itself.

Checks reject lockfile mismatches, version regressions, reused release versions,
missing/duplicate changelog sections, and TODO/TBD release notes. Rerunning the
original release commit is allowed when its tag already exists, so a failed
publication can be recovered.

## Build and test the downloadable binary

```sh
nix build .#argos-release --out-link result-release
nix develop --command python3 scripts/package_release.py --package result-release
nix develop --command python3 tests/release_artifact.py \
  dist/argos-v0.2.0-x86_64-linux.tar.gz --integration
```

Use the current version in the archive filename. `argos-release` builds a static
musl x86-64 Linux executable, not the normal Nix wrapper. The archive includes
the binary, dynamic shell completions, optional tmux binding, changelog, version
file, and installation instructions. Native SSH/tmux remain runtime dependencies;
VM lifecycle still needs Nix/KVM. There is no general installer/platform matrix.

Packaging rejects a wrapper script, wrong architecture, dynamic interpreter,
shared-library dependency, missing data files, or mismatched `--version`.
The archive has stable ordering/modes/timestamps and a SHA-256 file that refers
to the archive basename, so it can be verified directly in a download directory.
Artifact tests extract the tarball and run its binary with no Nix PATH or loader
variables, then exercise real SSH/tmux/PTY integration when requested.

The normal `nix build` package remains wrapped with its Nix runtime tools. Nix
users can pin an exact release via `github:trbutler4/argos/v<version>`.

## CI and publication

PRs, main pushes, and manual workflow runs validate metadata, run Rust and CLI
integration checks, build/package the static binary, and test the extracted
artifact. The resulting `release-candidate-<commit>` workflow artifact is useful
for branch review. It is not a published version.
Reruns may replace that run's candidate artifact; published release assets remain
immutable.

Only a successful run on `main` can publish:

1. Verify the downloaded candidate checksum.
2. Skip publication if this version is already published; validation still ran.
3. Create an annotated version tag at the validated commit, or verify that an
   existing tag points to that exact commit.
4. Create/reuse a draft release and upload archive plus checksum.
5. Publish the draft only after uploads succeed.

On failure after tagging, rerun the **original workflow run**. A draft can be
resumed from the same commit. A different commit cannot take over its tag; fix
code under a new version instead. This workflow never force-updates tags or
overwrites published release assets.

The initial standalone target is x86-64 Linux. Physical-host/clipboard acceptance
and optional real-VM tests remain separate from the ordinary hosted CI suite.
