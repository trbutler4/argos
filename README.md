# Argos

A command center for NixOS machines: **Rust CLI first, then a full-screen
TUI consuming its structured output**.

**One place to find a running task, attach to its tmux session, and open its app,
regardless of which machine hosts it.** New tasks can get their own NixOS VM,
separate Git clone, toolchain, backend processes, and database state.

Status: **read-only multi-machine CLI discovery**. Local and SSH tmux listing
use a private machine inventory, bounded concurrency and per-host deadlines.
Local and SSH tmux attachment are available. The TUI and VMs are not implemented.
No remote installation or provisioning is performed.

## Try the CLI

```sh
cd argos
nix develop
cargo run --quiet --locked -- list
cargo run --quiet --locked -- list --json
```

`list` reads your machine inventory when present, otherwise it lists only the local
default tmux server. Use `list --local` to explicitly stay local, or
`list --socket /absolute/path --json` for a specific local socket. These local modes
bypass implicit inventory loading. Discovery does not create servers or change sessions.

JSON schema version 1 includes `observed_at_unix_ms` and a deterministic list of hosts with
`id` (hostname), `status`, `sessions`, and `error`. Sessions have `id`, exact `name`,
`windows`, and `attached_clients`. Session IDs are scoped to a server lifetime,
not globally persistent task IDs. Human output escapes special characters.
Exit codes: 0 for success (including no server/sessions), 1 for operational errors,
and 2 for invalid arguments. Operational tmux failures include a structured JSON
error, with diagnostics on stderr.

### Attach to a local session

```sh
argos attach 'session name'
argos attach '$4' --config /path/inventory.toml
```

Targets are exact session names or IDs from `list --json`, never prefixes or patterns.
Quote IDs so the shell does not expand `$`. Attachment selects the configured client
machine by default, not every machine in the inventory. `--host` can explicitly
select that local machine. Remote hosts require `--host HOST_ID` and a configured `ssh_alias`; Argos first
discovers the exact session ID, then attaches over SSH. `--config`, `--local` and
`--socket` use the same config-selection conventions as `list`.

- From a plain terminal, Argos hands the terminal to native tmux. Detach using your
  existing tmux prefix followed by `d`. Shells and backends keep running.
- Inside the same local tmux server, Argos switches the invoking session's sole attached
  client instead of nesting tmux. It does not detach other clients on the target.
- For a remote host from inside local tmux, Argos opens a local connection window
  named `argos:<host>:<session>` running SSH and remote tmux. Detaching that remote
  tmux client closes the connection window; it does not kill the remote session.
- If multiple clients are attached to the invoking session, Argos refuses to guess
  which terminal to switch. Cross-server nesting is also refused in this slice.
- Attachment is interactive, not JSON output. It requires a real TTY in either mode.

For development, build and run directly without a NixOS rebuild:

```sh
nix develop --command cargo build --locked
./target/debug/argos list --config ./config.local.toml
./target/debug/argos attach 'local-session' --config ./config.local.toml
./target/debug/argos attach 'remote-session' --config ./config.local.toml --host REMOTE_HOST
```

`config.local.toml` is gitignored and directly editable. Pass it explicitly to keep
development independent of the installed config. Building this binary does not
update your installed version or NixOS input pin.

### Nix package and installation

```sh
nix build                       # result/bin/argos
nix run . -- list                # no dev shell needed
```

The release package provides tmux and OpenSSH as PATH fallbacks while preserving
user-provided wrappers first. Rust/Cargo are only build and development
dependencies. To use the published package without installation:

```sh
nix run github:trbutler4/argos -- list
```

To install declaratively, add the flake input to your NixOS flake:

```nix
inputs.argos.url = "github:trbutler4/argos";
inputs.argos.inputs.nixpkgs.follows = "nixpkgs";
```

Pass `inputs` through `home-manager.extraSpecialArgs`, then add this to the desired
Home Manager user's module (which accepts `{ inputs, pkgs, ... }`):

```nix
home.packages = [ inputs.argos.packages.${pkgs.stdenv.hostPlatform.system}.default ];
```

Rebuild your NixOS configuration to activate it. Later updates are explicit:
`nix flake update argos`, followed by your normal system rebuild. During local
development use `cargo run` or `nix run .` before publishing and updating the pin.

### Development checks

Inside `nix develop`:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 tests/local_tmux.py
python3 tests/config_cli.py
python3 tests/remote_ssh.py
python3 tests/attach_tmux.py
python3 tests/remote_attach.py
```

The integration test uses a disposable real tmux server on a unique socket and
only cleans up that server. It never kills or modifies your existing sessions.
The SSH test uses a real rootless loopback sshd and real tmux with disposable keys
and trust files. A temporary PATH adapter supplies the test SSH configuration to
the real SSH executable. It does not fake responses or modify user SSH settings.
Run that test with the unwrapped debug binary, not the Nix-wrapped executable.
The dev shell and Rust dependencies are pinned in `flake.lock` and `Cargo.lock`.

## Private machine inventory

Copy `examples/config.toml` to `$XDG_CONFIG_HOME/argos/config.toml` (default
`~/.config/argos/config.toml`) and customize it. Keep this file outside the public
repository. All names in the example are fictional.

```toml
schema_version = 1

[client]
machine_id = "workstation"
connect_timeout_seconds = 3
max_parallel_probes = 4

[machines.workstation]

[machines.server]
ssh_alias = "server"
```

The client ID must name a machine in the inventory. That entry is queried locally.
Other entries require an existing SSH alias or hostname, optionally `user@host`.
Each entry can set `socket = "/path/to/tmux.sock"` on its own machine. The timeout
is a total per-host discovery deadline, not just a connection timeout. Parallelism
is limited by `max_parallel_probes`. Unknown fields and unsupported schema versions
are rejected rather than silently ignored. The future VM/project schema is separately
proposed in `examples/environment-proposal.toml` and is not accepted by this loader.

```sh
argos list                             # all configured hosts
argos list --json                       # successes plus structured per-host errors
argos list --host server                # one configured host
argos list --config /path/inventory.toml
argos list --local                      # no remote connections
```

`--host` requires an inventory. `--socket` is local-only and cannot be combined
with `--config` or `--host`. Missing default inventory falls back to local listing,
but a missing explicitly selected file is an error. Configuration errors exit 2.
A failed host does not hide healthy hosts: JSON retains every requested host and
exits 1 if any failed. Successful empty results exit 0.

SSH uses existing user SSH configuration and keys, requires known host keys, and
never prompts for passwords or accepts unknown keys. Forwarding is disabled for
probes. Run your usual `ssh <alias>` separately to establish trust/authentication
when needed. An unavailable agent, rejected key, offline host or missing remote
tmux is reported explicitly. No credentials belong in this inventory.

## The workflow

- All hosts run NixOS and already communicate over Tailscale.
- SSH provides terminal access, tunnels, and explicit file transfers over the tailnet.
- Existing tmux sessions remain usable without migration or VM adoption.
- Each managed task stays on its original machine. No live migration or file sync.
- Each task VM has a separate clone, not a Git worktree pointing at host metadata.
- Use devenv inside the clone for project tooling and process management.
- Keep the browser, terminal UI, and optional Hyprland integration on the client.
- This is a tool for one person's workflow, not a multi-tenant platform or a product
  that needs cross-platform installers and a plugin marketplace.

## What the interface should feel like

```text
argos
  workstation                                     online
    config            host tmux     attached
    app-feature       VM running    stack ready       [app]
    app-fix           VM running    stack starting
  server                                          online
    experiments       host tmux     detached
  laptop                                          unreachable · last seen 12m ago
    docs              host tmux     last observed running
```

All host and project names in examples are fictional. Do not infer LLM activity
from a process merely being alive. Offline hosts have unknown current state,
not a fabricated stopped state.

Actions: search, attach, open service, inspect logs/status, and, for **managed task
VMs only**, create/start/stop. Deleting an environment is a separate confirmed action.

An environment is not a machine and a session is not an environment:

```text
machine: stable NixOS host, reachable through its existing SSH alias
  environment: optional managed VM + clone + persistent data + service definitions
    session: persistent tmux shell, agent, or interactive tool
```

Existing host tmux sessions can be listed and attached without registering a project.

## Proposed command surface

The executable is named `argos`.

```text
argos                              # CLI help initially
argos tui                          # full-screen TUI, added after the CLI
argos list                         # human-readable hosts and sessions
argos list --json                  # structured snapshot for the TUI/scripts
argos attach <machine/session>     # attach an existing session
argos env create <project> --host <machine> --name <task>
argos env start <machine/environment>
argos env stop <machine/environment>
argos open <machine/environment> [service]
argos env inspect <machine/environment>
argos env destroy <machine/environment>  # explicit destructive confirmation
```

Local/SSH `list`, JSON output, filtering, local/SSH `attach`, and help are implemented today. Other commands
are proposed, not installed commands. `start` will boot a stopped VM.
It does **not** restore process memory. Detach/switch to keep agents and backends
running; stopping a VM ends its processes while retaining its disk.

## Build order

1. **Connect to existing work, CLI first.** M0a delivers Rust commands for listing
   and attaching to tmux sessions on two real hosts, with human and JSON output.
   M0b adds the full-screen TUI as a consumer of those commands. Disconnecting the
   client never kills the remote session.
2. **Prove the isolated task environment.** Run two real project stacks in two
   separate-clone VMs on one host, using the same internal ports and independent
   database data. Validate tooling persistence across a reboot.
3. **Join the two workflows.** Create and operate managed environments from the same
   interface, attach through the host, and open each app through a local tunnel.
4. **Polish for daily use.** Better status/log views, optional Hyprland launch action,
   stable service origins, and lightweight LLM status hooks where actually supported.

Do not build a generic orchestration framework before the first two milestones work.

**First usable release: M0a, the cross-machine tmux CLI.** M0b layers the TUI on
top. Neither requires VMs or browser access. **Initial full task-environment workflow: M2**, after the M1 VM
proof. M3 is optional polish, not part of either completion boundary.

## Design choices and open decisions

- **Confirmed workflow:** NixOS hosts over Tailscale, existing tmux sessions,
  stationary task VMs, separate clones, Rust CLI-first delivery followed by a TUI,
  and no distribution-packaging priority.
- **Proposed design:** SSH transport, devenv project definitions,
  and host-authoritative state.
- **VM candidate:** microvm.nix, starting with a QEMU/KVM prototype. Persistent
  writable Nix state, networking, and service ownership are feasibility gates.
- **Implementation language:** Rust, chosen to build familiarity with the language
  for hands-on practice. This is a learning preference, not a
  claim that Rust will outperform Go on an I/O-bound SSH/VM workflow. Use native
  SSH/tmux commands and keep the TUI separate from discovery and lifecycle operations.
- **TUI framework:** choose a Rust framework for M0b, not a custom terminal renderer.
- **Not confirmed:** first project for the VM proof. `example-app` is a candidate, not an
  agreed requirement. Start with two modest VMs and measure resources on the target host.

## Documents

- [Architecture and workflow decisions](docs/architecture.md)
- [Milestones, acceptance checks, and first implementation tasks](docs/plan.md)
- [Example proposed inventory](examples/config.toml)

The user's NixOS config repo should eventually import this project's small host and
Home Manager modules. This repository owns the tool; it does not replace that repo
or modify host settings merely by opening the dashboard.
