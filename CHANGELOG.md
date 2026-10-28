# Changelog

User-visible changes are collected under Unreleased, then moved into a versioned
section when preparing a release. Release dates are recorded by GitHub Releases.

## [Unreleased]

## [0.3.1]

### Fixed

- VM application-port allocation skips TCP ports already bound by other host
  processes, including VMs owned by another user. Updating a stopped VM remaps
  occupied application ports while preserving its SSH identity and available
  mappings. Running VM mappings remain stable. Availability probes are advisory,
  not atomic reservations against later binds by unrelated processes.

## [0.3.0]

### Added

- Explicit `vm.defaults.share_host_git = false` isolates guest environments from
  controller SSH private keys and host Git configuration, with persisted policy.
- Global `vm.defaults.modules` merges snapshotted guest modules with repository
  profiles, so agent tooling can be installed without replacing project setup.

- Passive `agents list [--state-dir PATH] [--json]` and responsive VM picker
  overview for Jcode client-connected debug sessions, with scoped counts,
  observation timestamps, per-guest errors and visibly stale cached metadata.
  Discovery never enables debug control, starts guests/daemons or changes SSH trust.

- Profile-defined VM CPU, memory, `/var`, and writable Nix store disk resources,
  with advisory host capacity warnings and guards against implicit disk resizing.
- Declarative, snapshotted guest NixOS module files or bundles that survive updates.
- `vm fetch` for checksummed, no-overwrite guest artifact retrieval over trusted SSH.
- An opt-in headless Android emulator and Maestro guest profile example.

### Fixed

- Nix runner builds use bounded source-only staging, excluding runtime sockets,
  disk images, SSH keys and repository metadata without requiring a private Git repo.

## [0.2.0]

### Changed

- All interactive entry now uses an origin-side connection owner: `tui`,
  `sessions attach`, `connect`, VM tmux/shell, and VM/task up attachment.
- Nested tmux/SSH entry is rejected instead of opening another SSH window.
  Start Argos in a plain terminal; use the optional guarded return-to-picker
  binding to switch connections. Existing tmux bindings are not changed.
- VM list output is concise by default; use `--detailed` for metadata.
- VM reconnects preserve recorded SSH host keys instead of replacing trust.

### Added

- Shared host/VM picker with responsive streaming discovery, cancellation,
  retry, and stable selection during refresh.
- Task-oriented VM workflow, current-checkout repo defaults, VM restart, and
  global VM package/file defaults.
- Dynamic bash, zsh, and fish completions.
- Standalone static x86-64 Linux binary releases with checksums, completions,
  the optional tmux binding, and installation instructions.

### Fixed

- Attachment and discovery cleanup now reaps owned process groups and restores
  terminal state without leaving detached pipe-reader threads.
- First-Tab zsh completion, transient terminal-reset flush errors, and derived
  guest hostnames for long or underscore-ending VM IDs.

### Compatibility

- Managed interactive connections require Linux and direct SSH. SSH
  ProxyCommand/ProxyJump and nested entry are not supported.
- Escape reconnects the last Argos-selected target, not later native tmux switches.
- Inventory, session JSON, and persisted VM schema versions remain at 1.

## [0.1.0]

- Initial CLI, host/session discovery, TUI, and local VM workflow.
- The original download contained a Nix wrapper rather than a standalone binary;
  use the tagged Nix package for this historical release. Fixed in 0.2.0.
