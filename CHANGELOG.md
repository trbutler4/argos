# Changelog

User-visible changes are collected under Unreleased, then moved into a versioned
section when preparing a release. Release dates are recorded by GitHub Releases.

## [Unreleased]

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
