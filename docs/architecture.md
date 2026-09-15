# Architecture and decisions

The broader architecture remains a design proposal. The implemented local-list
contract and its current limits are documented in [README](../README.md).
The VM details must pass
[the prototype gates](plan.md) before they become defaults.

## 1. Keep each layer small

| Layer | Owns | Does not own |
| --- | --- | --- |
| Tailscale | Reachability and tailnet access policy between hosts | Git synchronization, VM migration, terminal persistence |
| OpenSSH | Authentication, remote commands, PTYs, file transfer and tunnels | Application lifecycle |
| tmux | Persistent interactive sessions, panes, scrollback and manual agent terminals | VM or database supervision |
| NixOS/systemd | Host capabilities, VM lifecycle and guest boot services | Project-specific tooling definitions |
| devenv | Pinned project tooling, development processes, readiness and logs | VM creation or cross-machine discovery |
| argos | Inventory, selection, environment operations, connection and status | A new terminal emulator, process manager, VPN or Git hosting service |

**Build the Rust CLI first (M0a), then the full-screen TUI (M0b).** Rust is chosen
for learning, not a demonstrated performance need. The CLI is the canonical interface
for discovery, attachment and later lifecycle operations. The TUI invokes those
commands and renders their structured results; it does not parse pretty tables or
implement a second SSH/VM control path. No local HTTP service is needed between them.

### CLI output contract

- Human-readable output is the default. Read commands such as `list --json` produce
  a versioned JSON snapshot on stdout, with stable machine/session/environment IDs,
  observation timestamps, data and structured per-host errors.
- Keep diagnostics on stderr and omit ANSI styling/progress chatter from JSON stdout.
  Partial host failure must retain healthy-host data and identify failed/unknown hosts.
- For valid read requests: exit 0 when discovery succeeds (including zero sessions),
  exit 1 for operational/partial failures while still returning the JSON envelope.
  Invalid arguments/configuration exit 2 with a diagnostic. The TUI handles missing
  or malformed output and rejects unsupported schema versions visibly.
- The TUI launches the installed CLI via an explicit executable path and argument
  array, using the same config as the user. Never interpolate labels into shell code.
  Begin with bounded refresh/polling; a live event stream is not required for M0.
- Interactive `attach` is not a JSON operation. The TUI restores/releases its terminal
  before handing control to the CLI/SSH/tmux PTY, then redraws on return. Do not capture
  and render a remote terminal session as if it were a JSON result.
- Later mutations stay CLI-owned, including validation and destructive confirmation.
  A canceled TUI must not kill remote services or abandon a supervised host operation.

Keep UI state separate from the Rust command implementation. Use cancellable, bounded
concurrency for probes and subprocesses so a network failure does not stall input.
Choose a Rust TUI framework for M0b instead of building a terminal renderer. No
browser/desktop management UI is planned; opening a project app in a browser remains
a separate action. Machine/session access works over SSH without Hyprland.

Proposed TUI layout: machine/session list, selected-item details, search/filter and a
contextual action/help bar. M0b needs keyboard navigation, live reachability/attachment
status, readable errors and a reliable attach/return flow. Support terminal resizing
and a usable compact layout. Specific widgets and keybindings remain design choices.
A slow host probe must not block input or redraws. The TUI may run in a tmux window
or be launched in a sufficiently large popup, but the popup is not the product itself.

## 2. Machine inventory and remote discovery

Use an explicit, small user inventory with stable machine IDs and existing SSH
aliases. The aliases resolve to Tailscale addresses or MagicDNS names through the
user's SSH config. Do not scan or enroll every tailnet member automatically.

- Read-only discovery executes bounded SSH commands as the configured normal user.
- MVP queries the user's default tmux server. Explicit named sockets can be added
  when needed; do not enumerate other users' servers.
- Discover ordinary sessions without requiring a argos registry or repo marker.
- Resolve the current machine locally, so attaching its local sessions need not use SSH.
- Probe hosts concurrently, with bounded fan-out and a per-host connection timeout.
- Distinguish offline, authentication failure, untrusted host key, missing tmux,
  no tmux server/sessions, and a failed command. A failed probe is not an empty list.
- Show last-observed data with its timestamp; never claim cached state is current.
- Preserve host-key verification. Background discovery never blocks on a password or
  accepts an unknown key. Offer an explicit foreground SSH check to resolve access.

Use native command output formats, validate parsing, and keep remote command
construction separate from display labels. Session names, paths, and branch names
must not become unescaped shell code. No executable snippets in untrusted discovery data.

## 3. tmux connection UX

Do not embed tmux rendering in a homegrown terminal widget. Let the user's terminal
and SSH PTY carry the interactive session.

- Outside tmux: selecting an item can hand the terminal to an SSH/tmux attachment;
  detaching returns to argos with the terminal restored.
- Inside local tmux: switch-client for a session on that same server, avoiding nesting.
- For a remote session: use/reuse a dedicated local connection window. A short-lived
  picker popup must not own the remote process lifetime or become its only access path.
- Keep an easy route back to the hub/picker and label connections with their host.
- Disconnecting or closing a connection window must only detach. Do not use tmux
  attach flags that detach other clients by default, kill sessions, or restart shells.

Preserve the user's existing tmux prefix, pane navigation and clipboard bindings.
Remote tmux may be nested under local tmux, so prefix forwarding, returning to the
hub, OSC 52 clipboard and resize need real tests rather than assumptions.

If two clients attach to the same session, accept tmux's native behavior and make
it visible. Do not mistake multiple clients for duplicate environments.

## 4. A managed task environment

Each environment has a stable ID, a display name, a home machine, a project URL,
branch/ref, persistent storage, VM allocation, service descriptors and tmux sessions.
The global identity is `(machine ID, environment ID)`, not just a task name.

**A full clone inside the VM** keeps `.git`, source and runtime state independent.
Create an explicit task branch from the requested base ref. Existing host sessions
remain a separate unmanaged resource type and cannot be stopped/destroyed by VM actions.

Store persistent state for:

- clone, uncommitted files, commits and build outputs that the user chooses to retain;
- project database data and devenv state (not just the source tree);
- the guest's writable Nix store **and its matching Nix database/metadata**;
- guest SSH host identity and explicitly provisioned user configuration.

Read-only guest OS images can be replaced independently of this data. Do not treat
an overlay alone as sufficient Nix persistence. Do not share writable stores or a
host repository's `.git` directory across guests.

### VM candidate and feasibility gates

Prototype **microvm.nix with QEMU/KVM** first, on one NixOS host. Pin versions once
the prototype is implemented. Initial resource hypothesis: 2 vCPU and 4 GiB per VM,
with configurable limits. Measure two copies of the selected project before choosing
host defaults. Android/emulator/GPU-heavy workflows are not part of this first proof.

Prefer a straightforward per-guest writable storage layout over premature shared
store optimization. Verify guest Nix builds and store/database consistency after
reboot. The microvm.nix shared-store documentation explicitly warns about overlay
and Nix database persistence. If the pinned version cannot support the needed layout
cleanly, use a fuller QEMU NixOS guest rather than introduce a fragile cache workaround.

Host root access may be needed once to configure KVM permissions, networks, allowed
VM units and storage. That setup is explicit, reviewed NixOS configuration, separate
from normal dashboard use. Never assume an SSH login implies permission to provision VMs.

Avoid a whole-host NixOS rebuild for every task creation. The VM spike must choose
and demonstrate either user-owned runner services or a narrowly scoped host lifecycle
helper. Do not promise a particular microvm.nix dynamic-creation API before testing it.

## 5. Process and session lifetime

Inside a guest, devenv defines runtimes, services, dependencies and readiness probes.
Pin the devenv version and use the process manager it actually supports; current
upstream documents a native manager, but older versions use different backends.
Do not assume process-compose is mandatory.

Guest systemd should own a development-stack service whose lifetime is independent
of SSH, tmux clients, and the dashboard. Integrate the pinned devenv command with
correct process supervision and shutdown. A detached launcher that exits immediately
must not be mistaken for a running stack by systemd. Validate foreground/detached
semantics in the prototype instead of shipping a guessed unit.

- tmux hosts interactive shells, editor/agent terminals and optional log views.
- devenv owns the project's long-lived backend/database processes.
- Guest boot may start the stack according to explicit project policy, not by every
  tmux shell's startup hooks.
- Authentication or project bootstrap failure is visible and retryable. Never report
  `ready` merely because the VM is running or the devenv process exists.
- Dependency updates are explicit. Creating a task uses the project's committed lockfiles.
- Projects without devenv can first use an explicit command adapter to an existing
  Nix shell. Do not silently rewrite their toolchain or migrate all repos at once.

A stopped VM loses live processes, SSH connections and in-memory tmux sessions.
Starting it restores persistent files/data and configured services, **not RAM or the
same running LLM request**. Any saved tmux layout restoration is a convenience, not
process resurrection. Detach is the operation for keeping work running.

## 6. Networking and browser access

Only the host needs Tailscale initially. Guests use private host-local networking.
For the first QEMU spike, try user-mode networking with a unique **host-loopback-only**
SSH forward per guest. If a host bridge/TAP is needed, configure it explicitly through
NixOS. Validate the actual backend, rather than assume user-mode networking is supported
by every microvm.nix hypervisor.

```text
local terminal/browser
  -> SSH over Tailscale to host
     -> guest SSH through host-local routing/forward
        -> guest loopback frontend/API/database
```

Use an SSH jump connection to the guest so a local forward terminates at the guest's
loopback service. This lets two guests both use, for example, frontend 3000, backend
8080 and PostgreSQL 5432 without changing their projects. Authenticate and verify
host keys for both hops. Give each guest a stable SSH HostKeyAlias so reuse of a host
forwarded port does not confuse identities.

`open` creates or reuses a **client-owned** loopback tunnel, verifies the port is bound,
and opens the browser on that same client. Reserve/select a free local port and report
it accurately. Reuse a stable port per client/environment when available, and handle
collisions rather than stealing or killing unrelated listeners. Track tunnels outside
the transient picker so switching tasks does not close them. Clean up only owned tunnels.

Here, client means the machine running argos. A argos process reached by
SSH on a headless machine cannot silently open the original laptop's browser. Run
the browser-access client on the workstation, or print connection/tunnel instructions
when no local graphical browser is available.

MVP URLs are explicit `http://127.0.0.1:<port>`. Friendly names, local reverse proxy and
HTTPS can come later. `.localhost` resolves on the browser's machine, not a remote host;
Tailscale DNS alone does not configure wildcard routes, certificates or application
origins. Test WebSockets/HMR, absolute API URLs and CORS in the actual selected project.
If authentication requires a fixed HTTPS origin, elevate stable origin support into the
VM integration milestone rather than pretending arbitrary ports work.

Do not publish guest apps to the whole tailnet by default. Direct tailnet exposure is
an explicit later option with firewall/ACL decisions.

## 7. State, operations and safety

Static machine inventory lives at `$XDG_CONFIG_HOME/argos/config.toml`
(default `~/.config/argos/config.toml`), outside the public repository and
optionally rendered by Home Manager. Active machine schema is in `examples/config.toml`.
Future project/VM defaults remain proposed in `examples/environment-proposal.toml`.
The host owns managed environment state; clients cache observations, not authority.
Two dashboards must be able to inspect the same environment without registry sync.

The example inventory deliberately does not declare individual task VMs, their clone
paths or database processes. `env create --host` creates a host-owned runtime record
containing the environment/project IDs, chosen branch, managed disk/clone paths,
guest SSH identity, VM allocation, service endpoints and operation status. Project
devenv definitions own database processes/data locations; access descriptors do not
duplicate those definitions. Credential provisioning is an explicit M1 design input,
not a secret field to improvise in the sample TOML.

Start with a small host-local registry with atomic updates and per-environment locks,
not a distributed database. Operations reconcile recorded intent with real systemd,
VM and tmux state. Machine-local ID allocation must also be serialized.

Once remote create/start operations exist, a long operation must survive the initiating
SSH disconnect. Use a supervised host job with an operation ID and durable status.
After reconnect, inspect that operation rather than blindly cloning/allocating again.
Keep any helper local or SSH-invoked with versioned structured input/output. No always-on
network management API is required. Do not add a general remote agent in the tmux-only MVP.

- Repeat start/stop is idempotent; concurrent create/stop has a defined conflict result.
- Validate environment IDs and confine storage paths to a managed root. Root-capable
  helpers must not execute caller-supplied arbitrary commands, paths or Nix expressions.
- Default views and discovery are read-only. No automatic host setup or lifecycle changes.
- Stop warns about live agents/processes and leaves every persistent disk intact.
- Destroy names the host, environment and data being removed, requires explicit
  confirmation, and checks dirty/unpushed work where possible. If the VM cannot be
  inspected, say preservation is unknown and block ordinary deletion. Defer forced
  destructive recovery until a deliberate design exists.
- Do not push/merge branches or delete task disks automatically. A snapshot is not a
  backup; backups are a separate existing-user responsibility in v1.
- No mounting the whole host home or forwarding the SSH agent into guests by default.
  Provision minimum Git and LLM credentials explicitly, scoped where possible.
  Never put secrets in Nix store paths, committed config, process arguments or UI logs.
- VMs are an isolation boundary, not a claim of complete hostile-code containment.
  Specify guest egress policy; NAT by itself does not prevent access to host/tailnet
  services. Do not hand an LLM agent host-management credentials for convenience.

## 8. Explicit non-goals

No VM migration, live-memory suspend/resume, shared writable checkouts, automatic file
sync, multi-user tenancy, Kubernetes, cloud provisioning, public service ingress,
Windows/macOS support, cross-architecture emulation, universal IDE integration,
agent transcript scraping, or a new LLM provider abstraction in v1.

Nix flake/dev tooling and a small NixOS/Home Manager module are legitimate parts of
making this work on the user's own machines. Installer matrices, release automation,
public packaging and a generic provider/plugin framework are not.

## Sources checked while scoping (2026-09-15)

- [microvm.nix overview](https://microvm-nix.github.io/microvm.nix/)
- [Guest declarations](https://microvm-nix.github.io/microvm.nix/declaring.html)
- [Shares and writable-store caveats](https://microvm-nix.github.io/microvm.nix/shares.html)
- [devenv processes and lifecycle](https://devenv.sh/processes/)

These are design inputs, not proof that the combined VM/devenv workflow works yet.
