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

Host sessions are unmanaged. Argos lists and attaches to them, but does not own their lifecycle.

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

```sh
argos tui --config ./config.local.toml
```

Keys:

- `j`, `k`, arrows: move.
- `r`: refresh.
- `Enter`: attach to the selected host session or VM session.
- `q`: quit.

## Releases

`Cargo.toml` is the source of truth for the Argos version. The Nix package reads
that version directly, so bumping `package.version` is the release version bump.

Every push to `main` runs the release workflow. If tag `v<version>` does not
exist, GitHub Actions validates the Rust project, builds the Nix package, creates
an annotated tag, and publishes a GitHub release with an x86_64 Linux tarball and
SHA-256 file. If the tag already exists, the workflow exits successfully without
creating another release.

## Development checks

```sh
nix develop --command bash -lc '
  cargo fmt -- --check &&
  cargo clippy --locked -- -D warnings &&
  cargo test --locked -- --nocapture &&
  cargo build --locked &&
  python3 tests/local_tmux.py &&
  python3 tests/config_cli.py &&
  python3 tests/host_status.py &&
  python3 tests/vm_list.py &&
  python3 tests/remote_ssh.py &&
  python3 tests/remote_attach.py &&
  python3 tests/tui.py
'
```

Package check:

```sh
nix build .# --print-build-logs
```
