# cmd-center

A command center for NixOS machines: **Rust CLI first, then a full-screen
TUI consuming its structured output**.

**One place to find a running task, attach to its tmux session, and open its app,
regardless of which machine hosts it.** New tasks can get their own NixOS VM,
separate Git clone, toolchain, backend processes, and database state.

Status: **first local CLI slice implemented**. Read-only local tmux listing works.
Remote hosts, attachment, inventory loading, the TUI and VMs are not implemented.
Nothing currently provisions or connects to remote hosts.

## Try the first slice

```sh
cd cmd-center
nix develop
cargo run --quiet --locked -- list
cargo run --quiet --locked -- list --json
```

`list` uses your local default tmux server, including the current server when run
inside tmux. Use `list --socket /absolute/path --json` for a specific socket.
Discovery does not create a server or change sessions. No inventory is required.

JSON schema version 1 includes `observed_at_unix_ms` and one local host with
`id` (hostname), `status`, `sessions`, and `error`. Sessions have `id`, exact `name`,
`windows`, and `attached_clients`. Session IDs are scoped to a server lifetime,
not globally persistent task IDs. Human output escapes special characters.
Exit codes: 0 for success (including no server/sessions), 1 for operational errors,
and 2 for invalid arguments. Operational tmux failures include a structured JSON
error, with diagnostics on stderr.

### Nix package and installation

```sh
nix build                       # result/bin/cmd-center
nix run . -- list                # no dev shell needed
```

The release package includes tmux on its runtime PATH. Rust/Cargo are only build
and development dependencies. To use the published package without installation:

```sh
nix run github:trbutler4/cmd-center -- list
```

To install declaratively, add the flake input to your NixOS flake:

```nix
inputs.cmd-center.url = "github:trbutler4/cmd-center";
inputs.cmd-center.inputs.nixpkgs.follows = "nixpkgs";
```

Pass `inputs` through `home-manager.extraSpecialArgs`, then add this to the desired
Home Manager user's module (which accepts `{ inputs, pkgs, ... }`):

```nix
home.packages = [ inputs.cmd-center.packages.${pkgs.stdenv.hostPlatform.system}.default ];
```

Rebuild your NixOS configuration to activate it. Later updates are explicit:
`nix flake update cmd-center`, followed by your normal system rebuild. During local
development use `cargo run` or `nix run .` before publishing and updating the pin.

### Development checks

Inside `nix develop`:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 tests/local_tmux.py
```

The integration test uses a disposable real tmux server on a unique socket and
only cleans up that server. It never kills or modifies your existing sessions.
The dev shell and Rust dependencies are pinned in `flake.lock` and `Cargo.lock`.

## Local configuration (planned)

Machine-specific inventory belongs in `$XDG_CONFIG_HOME/cmd-center/config.toml`,
defaulting to `~/.config/cmd-center/config.toml`, outside this repository.
`examples/config.toml` is a generic proposed schema, not an active configuration.
**The current CLI does not load this file yet.** Host aliases, project paths and
machine defaults will remain local when multi-machine discovery is implemented.
Credentials should stay in existing SSH/keychain tooling, not in this inventory.

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
cmd-center
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

Use `cmd-center`, not `cc` (which commonly names the C compiler).

```text
cmd-center                              # CLI help initially
cmd-center tui                          # full-screen TUI, added after the CLI
cmd-center list                         # human-readable hosts and sessions
cmd-center list --json                  # structured snapshot for the TUI/scripts
cmd-center attach <machine/session>     # attach an existing session
cmd-center env create <project> --host <machine> --name <task>
cmd-center env start <machine/environment>
cmd-center env stop <machine/environment>
cmd-center open <machine/environment> [service]
cmd-center env inspect <machine/environment>
cmd-center env destroy <machine/environment>  # explicit destructive confirmation
```

Only local `list`, `list --json`, and help are implemented today. Other commands
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
