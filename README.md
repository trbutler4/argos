# Argos

Argos is a personal command center for tmux sessions and isolated NixOS development VMs.

It has two separate concepts:

- **Sessions** are attachable tmux entry points. They may be unmanaged tmux sessions on any configured host, or tmux running inside an Argos VM.
- **VMs** are managed local microVM development environments. Argos creates, starts, stops, updates, and removes them. It does not manage project processes.

The normal project workflow is: create a VM from a repo, enter the VM-owned tmux session, then use the repo's own tools such as `nix develop`, `mise`, `just`, database scripts, or app runners.

## Current scope

Implemented:

- Local and SSH discovery of existing tmux sessions.
- Attach to host tmux sessions.
- Create and manage local microVMs.
- Task-oriented VM creation for parallel repo work.
- Attach to VM-owned tmux over SSH.
- A basic TUI over the same session and VM model.
- Repo profiles through `.argos.toml`.
- Per-VM host port allocation for services running inside the guest.
- Profile-defined CPU, memory and persistent disk resources.
- Snapshotted guest NixOS modules and clean, runtime-free Nix build inputs.
- Checksummed retrieval of guest artifacts with `argos vm fetch`.

See [Guest profiles and artifacts](docs/guest-profiles.md) for the configuration
contract and the opt-in [Android example](examples/android/profile.toml).

Not implemented yet:

- Remote VM creation through the controller.
- Tailscale HTTPS naming.
- Project process management.
- Cloud provisioning.

## Install or run

From the repo:

```sh
nix develop
cargo run --locked -- --help
cargo run --locked -- list
cargo run --locked -- tui --config ./config.local.toml
```

Build the packaged binary:

```sh
nix build
./result/bin/argos --help
```

Run from GitHub without installing:

```sh
nix run github:trbutler4/argos -- list
```

As a Nix flake input:

```nix
inputs.argos.url = "github:trbutler4/argos";
inputs.argos.inputs.nixpkgs.follows = "nixpkgs";
```

Then install the package for a host or Home Manager user:

```nix
inputs.argos.packages.${pkgs.stdenv.hostPlatform.system}.default
```

## Config

The Argos config is a TOML machine inventory. The default path is `~/.config/argos/config.toml`. A local development config such as `config.local.toml` is also fine.

```toml
schema_version = 1

[client]
machine_id = "workstation"
connect_timeout_seconds = 3
max_parallel_probes = 4

[machines.workstation]
# local machine, default tmux socket

[machines.desktop]
ssh_alias = "desktop"
# socket = "/absolute/path/to/tmux.sock"

[vm.guest_tmux]
config_path = "/absolute/path/to/tmux.conf"
# or:
# config_text = """
# set -g mouse on
# """

[vm.defaults]
# Installed in every VM created with this Argos config. These are Nixpkgs attrs.
packages = ["ripgrep", "jq", "zsh"]
# Optional guest NixOS modules or module bundles, snapshotted into each VM.
# Relative paths resolve from this Argos config file's directory.
# modules = ["guest-base.nix", "guest-tools"]

[[vm.defaults.files]]
# Non-secret file copied into every VM at activation time.
target = "/root/.config/example"
source_path = "/absolute/path/to/example"
mode = "0644"

[[vm.defaults.files]]
target = "/root/.config/inline-example"
text = "inline file contents\n"
```

`--config` always means Argos config. Commands only read the sections they need.

SSH hosts must already work noninteractively, for example:

```sh
ssh -T -o BatchMode=yes desktop true
```

## Sessions

List sessions:

```sh
argos sessions list --config ./config.local.toml
argos list --config ./config.local.toml
argos list --local
```

Attach to a host tmux session:

```sh
argos sessions attach host:desktop:qvattro --config ./config.local.toml
argos sessions attach qvattro --host desktop --config ./config.local.toml
```

Attach to a VM-owned tmux session:

```sh
argos sessions attach vm:trade-feature
argos vm tmux trade-feature
```

Host tmux sessions remain user-owned. Argos manages every interactive attachment
and its transport, while tmux owns the sessions and pane processes.

### Managed connections are the default

From a plain Linux terminal outside tmux and SSH:

```sh
argos connect --config ./config.local.toml
argos connect host:desktop:work --config ./config.local.toml
argos connect work --local
argos tui --config ./config.local.toml
argos sessions attach host:desktop:work --config ./config.local.toml
argos sessions attach vm:my-task
```

`tui`, `sessions attach`, and `connect` use the same origin-side connection
owner and picker. `vm tmux`, `vm shell`, and the attachment phase of `vm up` /
`task up` also keep Argos supervising SSH. There is no unmanaged attach mode and
no fallback that opens another SSH window inside tmux. Detectable nested entry
is rejected with guidance to return to the origin picker. Lifecycle-only
`--no-attach` / `--json` operations may still run inside tmux or SSH.

Host tmux and local VM tmux share the picker and switching loop. VM shell is a
supervised plain SSH shell; exit the shell to return to the caller. SSH
ProxyCommand/ProxyJump configurations are not supported yet.

The optional [tmux binding](examples/connect.tmux.conf) uses F12 to detach only
the invoking managed client and return to the origin picker. Ownership does not
depend on installing this key. Review your existing
F12 binding before sourcing the file on a destination server. The Nix package
installs it at `share/argos/connect.tmux.conf`; it is never loaded automatically.
The guard requires Linux `/proc` and GNU grep on the destination. Unmanaged
clients receive guidance and stay attached.

- **Enter:** attach to the selected host or VM session.
- **Escape:** exit the initial picker; otherwise reconnect the last
  **Argos-selected** target. Native same-server tmux switches do not update it.
- **Shift-R:** retry the last target after failure; **r:** refresh discovery.
- **q:** quit the picker. Normal tmux detach exits managed mode to the shell.

The picker renders immediately and adds host/VM results as they arrive. Slow or
failed hosts do not block navigation, resize, or quit. Local probes run first;
remote probes use the configured concurrency and deadlines. During refresh,
earlier rows are marked as potentially stale and selection follows the same
target when rows move. Repeated `r` presses request at most one follow-up refresh.
Selecting, retrying, or quitting cancels and joins active discovery before the
next attachment starts. `READY` means the refresh finished, not that every host
or VM is healthy; per-host errors remain visible.

Selection disconnects first, so cancel requires reconnection and can fail.
Failures remain visible with retry, selection, and quit controls. Sessions and
pane processes remain alive while switching. Managed attachment uses tmux `-E`
to keep client metadata out of shared session environment; it does not update
session environment variables. SSH masters are neither reused nor terminated.
Guest tmux is created before marking its attaching client, keeping the marker
out of a newly started guest server and its panes. VM reconnects preserve recorded
guest host keys instead of replacing them on each attachment.

See the [implementation review guide](docs/single-attachment-implementation.md)
for ownership details, tested behavior, and remaining human acceptance checks.

## VMs

Create or enter a task VM for a repo:

```sh
cd ~/Projects/my-repo
argos task create "fix auth"
argos task up "fix auth"
```

Task commands derive a VM id from the repo name and task name. For example,
`argos task create "fix auth" --repo ~/Projects/account` creates a VM named
`account: fix auth` with id `account-fix-auth`. Use `--project` or `--id` when
you want explicit naming.

When `--repo` is omitted, Argos uses the current git repository. Pass `--repo`
explicitly when creating a task VM from outside the repo.

Lower-level VM commands are still available:

```sh
argos vm up "my task" --repo .
```

Useful VM commands:

```sh
argos vm list
argos vm list --detailed
argos vm show my-task
argos vm logs my-task --follow
argos vm shell my-task
argos vm tmux my-task
argos vm restart my-task
argos vm stop my-task
argos vm rm my-task --dry-run
argos vm rm my-task --force
```

`argos vm list` prints one VM id per line. Use `--detailed` for status,
project, repo, package, port, and path details.

`vm up` creates the VM if missing, starts it if needed, then attaches to guest tmux. Use `--no-attach` to only create/start.

VM state is host-local. By default Argos uses:

- `~/.local/state/argos/vms/*.json` for VM records.
- `~/.local/state/argos/instances/<id>/` for generated microVM files and logs.
- `~/.local/state/argos/workdirs/<id>/repo` for cloned repos.

`--state-dir /absolute/path` can isolate test state.

### Repos and git credentials

If `--repo` is a local checkout with an `origin`, Argos clones from that origin into VM-owned storage. The guest repo therefore has a normal remote and can `git fetch`, `git pull`, and `git push`.

Generated VMs mount the VM host's `~/.ssh` at `/root/.ssh` and copy the VM host's git config into `/etc/gitconfig`, so guest git uses the host's existing credentials and identity.

### Repo profile

A repo may include `.argos.toml`:

```toml
[workspace]
workdir = "/workspace/repo"

[vm]
packages = ["go", "nodejs", "postgresql", "redis", "just"]

[[ports]]
name = "api"
guest = 3001

[[ports]]
name = "web"
guest = 5173
```

Argos installs the listed Nixpkgs package attributes in the guest. It maps each declared guest port to a unique host loopback port, so two VMs can both run an app on guest port `3001`.

Global `[vm.defaults]` packages from the Argos config are installed before repo packages, with duplicates removed. Global `[[vm.defaults.files]]` entries are intended for non-secret dotfiles and small helper config. Relative `source_path` values are resolved relative to the Argos config file.

`[vm.defaults].modules` supplies guest NixOS module files or bundles before the
repo profile's `[vm].modules`. Relative module paths resolve from the Argos
config file's directory. Sources are snapshotted into the VM's build inputs,
not loaded from mutable host paths on each boot. Keep these modules non-secret:
their contents enter the Nix store. Use explicit `vm update` to apply changed
defaults to an existing VM, then stop/start a running VM to activate its rebuild.

`argos vm show <id>` prints the assigned mappings, for example:

```text
ports:
  api: guest:3001 -> host:http://127.0.0.1:43000
```

Argos does not run servers. Start those inside the VM using the repo's own tools.

### Updating an existing VM profile

After changing `.argos.toml`:

```sh
argos vm update my-task --dry-run
argos vm update my-task
```

Profile resolution order:

1. Explicit `--profile PATH`.
2. The cloned repo's `.argos.toml`.
3. The previous profile path if it still exists.
4. Defaults.

If the VM is already running, stop and start it to apply changes to the generated microVM config.

## Host helpers

On a VM-capable host:

```sh
argos host status
argos host status --json
```

From a controller with config:

```sh
argos hosts status --config ./config.local.toml
argos hosts status --config ./config.local.toml --json
```

The status helper reports Argos version, state paths, required tools, and `/dev/kvm` access.

## TUI

### Guest Jcode overview

```sh
argos agents list
argos agents list --state-dir /absolute/path/to/state --json
```

This is a passive overview of **client-connected Jcode debug sessions** inside
running, locally recorded Argos VMs. It is not a count of all persisted sessions
or all headless agents. A successful empty array means only that the selected
guest debug server reported no client-connected sessions. Busy sessions may have
null model, provider, working directory, and token-usage fields. Missing metadata
stays unknown. No prompts or transcripts are requested.
`is_processing` is upstream swarm-status metadata, not an independent guarantee
that a model is running or idle. Null/missing state remains unknown.

The version-1 JSON envelope contains `schema_version`, `observed_at_unix_ms`,
`scope = "client_connected_debug_sessions"`, and `vms`. Each VM result includes
`vm_id`, `status`, `scope`, `observed_at_unix_ms`, `sessions`, and a nullable
structured `error` with `code` and `message`. Session records expose the debug
API's session id, friendly name, provider/model, processing state, working
directory, swarm id, status/detail, and token usage. Unknown upstream fields are
ignored. Remote labels are sanitized before display. Existing session-list and
VM-list JSON contracts are unchanged.

Operational/partial probe failure exits 1 while retaining healthy VM results.
Stopped VMs are labeled `stopped`, not probed, and do not themselves cause failure.
Missing Jcode, disabled or unavailable debug control, missing/untrusted host keys,
authentication, unreachable guests, deadlines, output limits, and malformed JSON
remain errors, never successful zero-session counts. Invalid arguments exit 2.

The guest must already have Jcode installed and its local debug control explicitly
enabled by its deployment. Refresh invokes only
`jcode --no-update debug sessions`. It never enables debug control, starts a daemon
or VM, creates a session, keyscans, or rewrites SSH trust. The stable API-bridge
session list is deliberately not used because its retention writes and idle
semantics are unsuitable for passive discovery. Debug socket selection uses
Jcode's default main socket resolution. A debug `--socket`, if added in the future,
must name the **main** socket, not the derived `-debug` socket. Multiple independent
Jcode servers/sockets are not aggregated.

Probes require a recorded IPv4 loopback SSH endpoint and an existing regular
per-guest `known_hosts` file. They ignore user SSH configuration, enforce strict
host-key checking, and disable forwarding, agent forwarding and SSH masters.
CLI probes run at most four at once with three-second deadlines and the shared
four-MiB capture limit. Picker probes use the configured parallelism (capped at
16) and deadline, with the same cancellation and capture limits.

VM picker rows show scoped counts or probe errors. The selected VM detail pane
shows session metadata and observation timestamps. Failed refreshes retain the
last successful data visibly marked **STALE**, alongside the latest error.
Refreshes coalesce and cancel/join before attachment or quit. **Enter still opens
normal guest tmux**, not a Jcode session or control operation. The cache is
in-memory for the picker lifetime, not a persisted inventory.
Discovery runs when the picker opens and on manual **r** refresh. There is no
automatic polling interval. Terminals below 20 rows hide the detail pane to keep
the selected row and navigation controls usable.
The detail preview bounds label lengths and previews at most 20 sessions, clipped
to terminal space. Use `agents list --json` for the complete capped probe result.

```sh
argos tui --config ./config.local.toml
```

Keys:

- `j`, `k`, arrows: move.
- `r`: refresh.
- `Enter`: attach to the selected host session or VM session.
- `q`: quit.

## Shell completion

Completion is dynamic: the shell asks the `argos` binary for candidates on each
Tab, so suggestions always match the installed version.

The Nix package installs zsh, bash and fish completions automatically. For a
non-Nix build, register the completer in your shell rc file:

```sh
# zsh, in ~/.zshrc
eval "$(COMPLETE=zsh argos)"

# bash, in ~/.bashrc
eval "$(COMPLETE=bash argos)"

# fish, in ~/.config/fish/config.fish
COMPLETE=fish argos | source
```

Beyond subcommands and flags, completion fills in live values:

- `--host` offers machine IDs from your private inventory.
- `vm start|stop|show|rm|logs|shell|tmux|restart|update` offer local VM IDs.
- `sessions attach` offers `vm:<id>`, `host:<host>:` and local tmux names.

Completers only read the local inventory file and this host's state directory.
They never open SSH connections, so a slow or unreachable host cannot stall a
Tab press, and remote session names are not enumerated.

## Releases

`Cargo.toml` is the source of truth for the Argos version. The Nix package reads
that version directly, so bumping `package.version` is the release version bump.

Use stable `MAJOR.MINOR.PATCH` versions. Before 1.0, new features and breaking
workflow changes bump MINOR; compatible fixes bump PATCH. Collect changes under
Unreleased in [CHANGELOG.md](CHANGELOG.md), then prepare a release explicitly:

```sh
python3 scripts/release.py bump 0.2.1
python3 scripts/release.py check --base-ref origin/main
```

The helper updates the manifest, root lockfile version, and changelog together.
CI validates every PR/main push and tests a standalone **static x86-64 Linux
binary**, including real SSH/tmux/PTY checks. A successful version bump on `main`
creates an immutable annotated tag and publishes a tarball plus SHA-256 checksum.
Existing versions still get validation, but their published assets are not replaced.

Downloads contain `bin/argos`, shell completions, the optional tmux binding, and
installation notes. The binary runs without Nix; session commands need SSH/tmux
on PATH and VM lifecycle still needs Nix/KVM. See
[binary installation](docs/binary-release.md) and the
[versioning/recovery guide](docs/versioning.md).

## Development checks

```sh
nix develop --command bash -c '
  cargo fmt --check &&
  cargo test --locked &&
  cargo clippy --locked --all-targets -- -D warnings &&
  cargo build --locked &&
  python3 tests/release.py &&
  python3 tests/local_tmux.py &&
  python3 tests/config_cli.py &&
  python3 tests/host_status.py &&
  python3 tests/vm_list.py &&
  python3 tests/vm_resources.py &&
  python3 tests/vm_modules.py &&
  python3 tests/vm_fetch.py &&
  python3 tests/credential_sharing.py &&
  python3 tests/remote_ssh.py &&
  python3 tests/remote_attach.py &&
  python3 tests/attach_tmux.py &&
  python3 tests/connect.py &&
  python3 tests/tui.py &&
  python3 tests/jcode.py &&
  python3 tests/completions.py
'
```

Package check:

```sh
nix build .# --print-build-logs
```

Explicit real-VM connection acceptance (requires local KVM; creates, boots, then
removes one disposable VM using generated credentials and isolated HOME/state):

```sh
nix develop --command python3 tests/vm_connect.py
```
