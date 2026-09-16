# argos contributor guidance

This is a personal tool for one user's NixOS + Tailscale + tmux workflow.
Read README.md, docs/architecture.md and docs/plan.md before implementation.

## Current phase

The Rust CLI implements local/SSH `list`, JSON output, strict private TOML
inventory, host filtering, deadlines, a read-only `tui` session browser, and a
read-only `host status` helper for installed host instances. `vm list` reads
local VM state, `vm create` writes local state/workdir scaffolds and optional
guest tmux config from `[vm.guest_tmux]`, `vm start`/`vm stop` run the local
microVM process from that state, and `vm shell`/`vm tmux` SSH into the guest.
The intended development entry point is VM-owned tmux with `argos vm tmux`; the
legacy `attach` command has been removed to keep the workflow focused on
isolated project VMs. `examples/config.toml` is active inventory schema, while
`examples/environment-proposal.toml` is a future proposal. Do not mark
implementation milestones complete from documentation checks.

## Keep the design focused

- Reuse SSH, tmux, NixOS/systemd and devenv. Do not replace their core functions.
- The normal interactive development path is a VM-owned tmux session per project.
- Each task VM stays on its home machine and has a separate clone and persistent data.
- No live migration, automatic synchronization, public ingress, multi-tenant control
  plane or general distribution-packaging effort.
- Build the Rust CLI first, then a full-screen Rust TUI consuming CLI JSON output.
  The CLI is the canonical operational interface, not just a diagnostic companion.
  The TUI framework remains open. Prefer straightforward idiomatic Rust, explain
  important ownership/concurrency tradeoffs briefly, and avoid unnecessary generics,
  unsafe code or performance tuning. Never parse human-readable CLI tables in the TUI
  or duplicate SSH/tmux/lifecycle logic there.
- Project/host names in mockups are illustrative. Do not silently pick `example-app` or
  connect to/provision hosts merely because they appear in an example.

## Safety and validation

- Reading inventory is not consent to mutate hosts or kill sessions.
- Preserve host-key verification and existing tmux bindings.
- Never mount the whole host home or forward an SSH agent into task guests by default.
- Secrets must not enter source control or Nix store paths.
- Stop keeps persistent data. Destroy is a separate explicitly confirmed operation.
- Protect unrelated local/remote sessions and use disposable test data.
- Test through real SSH, tmux, microvm/QEMU, systemd and browser interfaces as
  milestones mature.
  Synthetic tests alone do not prove persistence, PTY handling or full-stack readiness.
- Keep provisional decisions and external blockers explicit. Update scope if findings
  invalidate a VM backend or lifecycle assumption rather than hiding it behind an abstraction.
- Commit focused changes. Enter `nix develop`, then run `cargo fmt --check`,
  `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo build --locked`, `python3 tests/host_status.py`,
  `python3 tests/vm_list.py`, `python3 tests/local_tmux.py`,
  `python3 tests/config_cli.py`, `python3 tests/remote_ssh.py`, and
  `python3 tests/tui.py`.

## Public repository boundary

- Keep machine-specific inventory out of Git, normally at
  `$XDG_CONFIG_HOME/argos/config.toml` (default `~/.config/argos/config.toml`).
  A gitignored checkout-local `config.local.toml` may be selected explicitly for
  development. Commit only generic examples.
- Do not commit local hostnames, usernames, project paths, private endpoints,
  credentials, captured session listings, or personal Git author email addresses.
