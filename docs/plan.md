# Implementation plan

M0a/M0b have evolved into a VM-first workflow: host tmux can be listed for
situational awareness, but interactive development entry is VM-owned tmux through
`argos vm tmux`. Cross-machine VM placement remains future work.

### Slice 1: local listing

Implemented: pinned Rust dev shell, `list`, `list --json`, optional `--socket`,
local hostname identity, versioned snapshots and explicit empty/error behavior.
See README for commands to run this slice and its real-tmux integration checks.
This was an early subset of M0a, before remote discovery, timeouts and VM-owned
tmux entry were implemented.
Next slice after user feedback: explicit host inventory and SSH-based listing.

Local validation:

- `nix develop` realized the pinned shell. `cargo fmt --check`,
  `cargo test --locked` (7 tests), `cargo clippy --locked --all-targets -- -D warnings`,
  and `cargo build --locked` all passed.
- `python3 tests/local_tmux.py` passed against a real disposable tmux server:
  help/usage exit codes, absent server without starting one, invalid socket, missing
  tmux, JSON schema, exact Unicode/quotes/metacharacters/trailing-space names, numeric
  metadata, escaped human output, repeat snapshots, and unchanged pane process IDs.
  The first run exposed a regular-file socket being misreported as empty. Explicit
  socket validation fixed it and the real test passed on rerun. tmux itself rejected
  control-character names, so the integration fixture uses supported unusual names.
  Control escaping and final-CR preservation have unit coverage only.
- Read-only actual CLI human/JSON runs matched existing sessions against
  native tmux: IDs, exact names, windows and attached-client counts. No existing
  sessions, settings or host configuration were changed.

### Slice 2: private inventory and remote discovery

Implemented: strict schema-versioned private TOML inventory, `--config`, `--host`,
`--local`, concurrent per-host probes, one native tmux query per host, total
per-host deadlines, output caps, and structured partial results. The Nix package
includes both tmux and OpenSSH. Examples separate active machine inventory from
the proposed VM/project schema.

Validation:

- Real CLI configuration checks cover implicit/explicit loading, missing files,
  schema/field/alias/limit rejection, host selection and local-only bypass.
- A real isolated loopback sshd and tmux test covers exact session data with Unicode,
  quotes, metacharacters and pipes, quoted socket paths, unchanged pane PIDs,
  local/remote agreement, filtering, host-key failures, authentication failures,
  refused connections and healthy partial results. It uses disposable keys and
  trust files and a PATH adapter that executes real SSH with a test config.
- Read-only discovery against existing personal tailnet hosts was attempted, but
  SSH authentication was rejected. No credentials, authorized keys or trust files
  were changed. Successful discovery on two physical machines remains an acceptance
  blocker, not a completed check. Hostnames and captured session data are not public.

Discovery limits: no cached or live-streamed state. Discovery is a current snapshot, not a
cache or live event stream. Remote login shells must support the fixed POSIX shell
probe. A timed-out local SSH process is killed and reaped, but custom ProxyCommand
subprocesses that outlive it are not guaranteed to be terminated in this slice.
Reader threads never delay return past the deadline. Proxy cleanup needs a dedicated
process-group follow-up before claiming arbitrary proxy-lifecycle coverage.

M0a has a working CLI path for discovery and a separate sessions layer for terminal entry. Host tmux remains unmanaged, while VM tmux belongs to managed environments. NixOS activation remains user-controlled.


### Slice 3: sessions attach layer

Implemented interface: `argos sessions list` for explicit host-tmux discovery and
`argos sessions attach TARGET` for terminal entry. Targets are typed at the edge:
`host:<host>:<session>` or `SESSION --host HOST` routes to unmanaged host tmux,
while `vm:<id>` routes to guest-owned tmux, equivalent to `argos vm tmux ID`.
`--config` remains Argos config for both target kinds; VM session attach accepts it
and reads only the sections it needs. This keeps VMs as lifecycle-managed
environments and sessions as attachable entry points.

Verified: real PTY local tmux tests exercise exact name/ID selection, same-server
switching, ambiguous/cross-server refusal, detach, terminal restoration and
unchanged panes. Real loopback SSH tests exercise remote host tmux attach from a
plain terminal and from inside local tmux. Unit tests cover target parsing for VM
and host session forms.

### Slice 4: simple TUI with session and VM entry

Implemented interface: `argos tui` with the same config/host/local/socket selection
as `list`. It renders hosts with separate `sessions` and `vms` groups, backed by
the tmux snapshot plus local VM state. It supports `j`/`k` and arrow navigation,
`r` refresh, `q`/Esc/Ctrl-C quit, and Enter attach handoff through the sessions
layer. Host tmux rows attach as host sessions; VM rows attach as `vm:<id>`.

Verified: Rust tests, fmt, Clippy, local/config/real-SSH integration checks,
`tests/tui.py` against a real PTY, and packaged Nix TUI smoke checks all passed.
The TUI PTY test verifies visible rendered host sessions and VMs, navigation,
refresh/quit input, non-TTY refusal, attach handoff, and terminal-mode restoration.

Boundaries: this is still not a rich VM dashboard. It does not start/stop VMs or
show service readiness.

### Slice 5: microvm.nix runner-package spike

Implemented interface: a flake-level `.#argos-microvm-prototype` runner package
for `x86_64-linux`, backed by `nix/microvm/prototype.nix`. It uses microvm.nix
with QEMU, a 9p read-only `/nix/store` share, and a small `/var` image created in
the runner's working directory. The VM includes `git` and `tmux`, locks the root
password, and emits `ARGOS_MICROVM_READY` on the console for boot validation.

Verified: `/dev/kvm` exists and the current user is in `kvm`; `nix build
.#argos-microvm-prototype` succeeded; the runner booted from a scratch directory
without a host NixOS rebuild, sudo, TAP/bridge setup, or repo-local runtime state;
readiness appeared on the console after about 15 to 16 seconds. No QEMU process was
left running after the test harness terminated it.

Boundaries: this proves only build and boot of a minimal MicroVM. It does not yet
prove SSH into the guest, guest host-key identity, forwarded ports, separate clone
state, `devenv up`, reboot persistence, or browser access. Those belong to the
next slice before any managed environment lifecycle command is added.

### Slice 6: installed-host status helper

Implemented interface: `argos host status [--json]`, intended to be run on each
VM-capable host where Argos is installed. It is read-only and reports schema
version, host name, Argos version, state/runtime roots, required tool availability
and `/dev/kvm` accessibility. This creates the first stable SSH-invoked host
helper contract without copying scripts to hosts or starting/stopping VMs yet.

Verified: 20 Rust tests, fmt, Clippy and `tests/host_status.py` passed. On the
current host, the helper reported `microvms: available` with `nix`, `tmux`, `ssh`
and accessible KVM. Packaged validation is required before updating the NixOS pin.

Boundaries: no lifecycle mutation, host registry, SSH-to-guest, or long-running
supervised operation exists yet. The command only proves the installed binary can
report whether the host is ready for those future steps.

### Slice 8: local VM create/list/start/shell/tmux/stop scaffold

Implemented interface: `argos vm list [--json] [--state-dir PATH]`,
`argos vm create NAME [--repo REPO] [--dry-run]`, `argos vm start ID`,
`argos vm shell ID`, `argos vm tmux ID`, and `argos vm stop ID`. `create` and `start` accept `--config` and use
`[vm.guest_tmux]` `config_text` or `config_path` to install a guest tmux config.
This is conservative host-local VM lifecycle scaffolding. `list` reads versioned JSON VM records from the host Argos state directory under
`vms/*.json`, returns a stable schema versioned snapshot, sorts records by ID,
treats missing state as an empty list, and rejects malformed state. `create`
writes one VM record, creates per-VM instance/work directories, optionally
performs a separate `git clone`, and renders a microVM config plus instance
flake. `start` builds the runner from that instance flake, launches it in a
dedicated host tmux console session, waits for the guest readiness marker, and
records PID/log/session/SSH metadata and rewrites the generated microVM config
from the selected Argos config so guest tmux settings can change between starts.
`shell` SSHes into the guest over the recorded local forwarded port, `tmux`
SSHes into `tmux new -A -s main` inside the guest, and `stop` terminates the
recorded local process while keeping persistent instance data.

Boundaries: no remote VM placement, no systemd user units, no port-collision
recovery, no richer guest project setup, and no state migration exist in this
slice. Future lifecycle commands should consume this state shape after they have
successfully completed their host-side action.

Release boundaries: **M0a (CLI) is the first usable release** without VM/browser
features. **M0b adds the full-screen TUI over CLI JSON output.**
**M2 is the initial complete managed-environment workflow**. M1 is its prerequisite
feasibility proof, and M3 is optional polish.

## M0: one interface for existing tmux work

### M0a: CLI foundation (first usable release)

**Deliverable:** Rust `list`, `sessions`, host status, VM lifecycle, and first read-only `tui` commands that work across the real hosts.
Interactive entry goes through the sessions layer: unmanaged host tmux for host sessions and guest-owned tmux for VM sessions.

1. Initialize the Cargo crate and a pinned Nix development shell for Rust/Cargo,
   rustfmt, Clippy and required CLI tools.
2. Parse the explicit machine inventory using existing SSH aliases. Define stable
   IDs, observed/cached state and the versioned JSON envelope described in architecture.md.
3. Discover sessions concurrently with deadlines and structured per-host errors.
   No required remote installation beyond existing SSH and tmux in this milestone.
4. Implement human-readable `list`, `list --json`, `sessions list`, host status,
   and VM lifecycle commands. Keep JSON stdout clean and test exit codes, partial failures and empty results.
5. Route all interactive terminal entry through `sessions attach`: host targets enter
   unmanaged host tmux, and VM targets enter guest-owned tmux. Preserve the user's existing tmux bindings.

M0a exit: complete the CLI portions on the real machines, including VM-owned
persistent-process and terminal checks. CLI output must remain ready for another
program to consume.

### M0b: TUI over the working CLI

**Deliverable:** a full-screen Rust TUI using the M0a command/output contract.

1. Choose a Rust TUI framework. Add `argos tui` with keyboard navigation,
   host status, separate session/VM groups and contextual help.
2. Invoke CLI read commands asynchronously with `--json`; render structured data and
   errors rather than scraping terminal tables. Keep filtering/selection in the TUI,
   but all host operations in the CLI. Handle schema mismatch and failed subprocesses.
3. TUI entry should hand the terminal to `argos sessions attach` and
   restore/redraw afterward. Test inside and outside tmux; guest tmux must not be treated as captured JSON output.
4. Add optional launch bindings only when they preserve the existing prefix, session
   picker, clipboard setup and navigation bindings.

Acceptance on the user's real machines:

- **A01:** On two real hosts, M0a human and JSON listings represent the same ordinary
  sessions without project/VM registration. Parse the actual CLI stdout, verify stable
  IDs/timestamps/schema version and ensure diagnostics cannot corrupt JSON. In M0b,
  the TUI displays those results, supports keyboard search/navigation/help and remains
  usable when resized. No duplicate host discovery implementation is introduced.
- **A02:** Enter a disposable session on each relevant host, including host tmux and VM-owned tmux. Record shell/process PID,
  run a counter, switch away and reconnect. Same remote process remains alive and
  the counter advanced. Removing only the SSH client must not kill the session.
- **A03:** Run `sessions attach` inside and outside host tmux. Host targets connect
  to unmanaged host tmux; VM targets connect to guest-owned tmux. Prefix forwarding, resize and terminal
  cleanup work. Repeat through the TUI once TUI launch is added.
- **A04:** Test an unreachable host, an authentication error, an untrusted host key,
  missing tmux and zero sessions. Each is distinct, and a bad host cannot freeze
  CLI completion or hide successful results from other hosts. JSON partial results
  and exit codes agree. In M0b, these errors do not freeze navigation or redraws.
- **A05:** Existing bindings and sessions remain unchanged. Verify OSC 52 clipboard
  end-to-end through the actual terminal and remote tmux, not just a config flag.
- **A06:** Session labels containing spaces/quotes/metacharacters are safely displayed
  and displayed, never evaluated as shell commands. In M0b, canceling selection does nothing.

Initial responsiveness targets to measure: render cached/local results immediately,
finish healthy-host discovery within 2 seconds on the tailnet, and bound an individual
unreachable-host attempt to 3 seconds. These are targets, not performance claims.

Exit: M0a ships independently as the CLI. M0b then demonstrates real task switching
through CLI-backed TUI views across two machines. Record the CLI contract, chosen
TUI framework decisions before expanding VM management.

## M1: prove two isolated project stacks

**Deliverable:** a reproducible NixOS VM prototype on one host, not a VM fleet manager.
This is an early feasibility gate, not an invitation to build every hypervisor adapter.

Before starting the VM proof, write a short prototype decision record naming:

- The actual project, base ref, seed data, service ports and expected browser workflow.
  Decide whether fixed HTTP/HTTPS origins are required before running A07; required
  origins belong in M1, not in a hoped-for later proxy feature.
- The pinned devenv release and chosen supervisor mode. For a repo without devenv,
  either add a minimal project definition with approval or explicitly revise this
  milestone to use another tool. An existing-shell adapter is a bootstrap aid, not
  evidence of completed devenv integration.
- The VM runner's execution user, service/registry/storage locations and authority
  boundary. Prove the specific start/stop/create mechanism without a host rebuild;
  do not implement a generic privileged helper before choosing that mechanism.
- Required Git access, LLM authentication and app secrets. State their runtime-only
  delivery (for example, user-provided mode-0600 files over SSH), refresh and revocation
  procedures. Provider revocation is separate from deleting a guest or its local files.
- Guest egress expectations: repo endpoints, Nix caches and LLM APIs, plus any explicit
  host/tailnet access. Document what the chosen network mode actually permits and any
  required firewall rules. Do not label NAT alone as a restrictive egress policy.

These are intentionally unresolved at scoping time. They are required before M1
acceptance, not optional decisions to defer until after the manager ships.

Work:

1. Confirm the first project and host. `example-app` is a candidate only. Check host KVM,
   architecture, memory/disk headroom, repo authentication and project service needs.
   Initial local KVM availability is confirmed on the current machine.
2. Pin microvm.nix/NixOS/devenv versions and create a QEMU/KVM guest prototype.
   The first runner-package prototype builds and boots without a host rebuild; SSH,
   persistence and project tooling are still pending.
3. Prove private guest connectivity through the host, guest host-key verification,
   and client loopback forwarding to services bound to guest loopback.
4. Give each guest independent writable storage for clone, project state, Nix store
   and Nix database. Prefer correctness over shared-store optimization.
5. Clone separately, select explicit branches, and bootstrap the project using its
   locked devenv configuration or a narrowly defined existing-shell adapter.
6. Establish guest systemd ownership of the devenv stack. Readiness and shutdown
   must reflect actual application state, not just a launcher exit code.
7. Write a manual runbook proving create/start/stop without rebuilding the entire
   host for each new task. Choose the simplest working lifecycle/privilege mechanism.

Acceptance:

- **A07:** Two VMs run the same real stack simultaneously on the same internal ports.
  Both are accessible from the client's browser, including WebSockets/HMR as applicable.
- **A08:** Write different marker rows into the two databases and edit/commit different
  source changes. Neither environment sees the other's data or Git metadata.
- **A09:** Close SSH and dashboard connections. Both stacks stay healthy. Kill/restart
  one backend via its process manager and verify the other environment is unaffected.
- **A10:** Gracefully stop and start one VM. Uncommitted files, commits, database rows,
  guest SSH identity and Nix package database state persist. Build/use a dependency
  installed before restart and verify Nix store consistency. Do not claim live tmux
  processes or LLM requests survive a powered-off guest.
- **A11:** Simulate a failed clone, bad service healthcheck and lack of VM privileges.
  Failures are diagnosable and do not report ready, expose secrets, or destroy data.
- **A12:** Measure boot-to-ready time, memory per VM and writable disk growth with both
  stacks running. Record measurements and adjust defaults instead of assuming 4 GiB fits.
- **A13:** Check host/guest forwarding listeners bind as intended. No unintended LAN,
  tailnet or public app ingress. Document/test guest egress and credential boundaries.

Exit: retain a working prototype/runbook and a short decision record on hypervisor,
networking, persistent Nix layout and devenv supervision. If microvm.nix persistence
is awkward in the pinned version, choose a full QEMU guest and document the tradeoff.
Do not postpone the persistence problem until after building the UI around it.
For the proposed devenv path, completion means the selected real stack runs through
the pinned devenv supervisor with working readiness, shutdown and reboot behavior.
If another tooling layer is chosen, update this scope and its checks explicitly.

## M2: manage task environments through argos

**Deliverable:** the same interface lists unmanaged host sessions and managed VM work.

Work:

- Introduce a host-owned environment registry, stable IDs, locks and operation status.
- Use installed Argos on VM-capable hosts as the initial SSH-invoked helper surface;
  start with read-only `argos host status --json` before lifecycle mutations.
- Wrap the demonstrated M1 lifecycle in `env create/start/stop/inspect`; no arbitrary
  root shell or broad sudo permissions. Use a small SSH-invoked helper only as needed.
- Make long create operations supervised and reconnectable with operation IDs.
- Enter guest tmux sessions using stable guest SSH aliases through the host.
- Implement `open` with owned, persistent client-side tunnels and accurate local URLs.
- Show host reachability, VM power state, stack readiness and session entry as
  separate states. Logs/errors remain accessible when readiness fails.
- Add narrowly scoped host and Home Manager modules for the user's NixOS repo.
- Add explicit `destroy` only after preservation and confirmation checks exist.

Acceptance:

- **A14:** From client A, create a task on host B, enter it, edit/run it and open its app.
  Switch to another task and back without restarting either backend.
- **A15:** From another configured client, find and enter the same environment on B,
  with no file copying or shared client state database. It stays on B throughout.
- **A16:** Repeat start/stop and race two clients' operations. No duplicate disk,
  registry entry, guest port or unintended process termination. A lost connection
  during create is recoverable by operation ID rather than repeating allocation.
- **A17:** An occupied local port is handled without killing its owner. A broken SSH
  tunnel is distinguished from an unhealthy backend; reconnect repairs only owned
  access paths. Switching picker selections does not tear down other task tunnels.
- **A18:** A pending/unhealthy devenv stack is not labeled ready. Existing unregistered
  tmux sessions still work and cannot be targeted by VM stop/destroy commands.
- **A19:** Stop preserves disks and warns about active work. Destroy cancellation leaves
  everything intact. Dirty/unpushed work or an uninspectable stopped VM blocks normal
  deletion; a disposable clean test environment can be explicitly deleted safely.
- **A20:** Full app workflow works in the local browser, including login redirects,
  cookies, frontend-to-backend URLs and WebSockets relevant to the chosen project.
  If fixed HTTPS origins are required, implement those here before declaring success.

Exit: complete a real task from two different clients with the environment stationary
on its original host, then deliberately stop it and recover its files/data later.

## M3: personal daily-use polish

Only after the core loop works:

- Richer terminal status/log views, favorites, recency and project filters.
- Optional Hyprland launcher/workspace association without making headless use depend on it.
- Stable local service hostnames/HTTPS when useful, not a public ingress platform.
- Explicit agent status hooks for supported tools, if desired. Use unknown when no
  signal exists. Process existence is not proof an agent is working or needs review.
- Resource summaries and manual cleanup suggestions. Never auto-delete inactive work.

## Requirement-to-check map

| User requirement | Acceptance evidence |
| --- | --- |
| Personal NixOS workflow, not distribution packaging | M0 local dev entrypoint; M1 pinned guest; M2 small user-specific modules, no installer matrix |
| Existing machines connected by Tailscale | A01, A04, A14, A15 using configured SSH aliases on the actual tailnet |
| One interface for existing persistent tmux work | A01–A06; existing sessions stay unmanaged and usable |
| Independent complete running environments | A07–A10, A14; two real stacks, independent data and continued processes |
| Separate clones accepted instead of worktrees | A08, A10; independent Git metadata and persistent local work |
| No moving VMs between machines or syncing checkouts | A15; access the original host from a second client |
| devenv for tooling and processes inside guests | A09–A11; actual supervisor, readiness and Nix persistence behavior |
| Easy browser access to each backend/frontend | A07, A17, A20 including app-specific origins and WebSockets |
| Keep work safe when switching or stopping | A02, A09, A10, A16, A19 |

## Inputs needed when their milestone begins

Not blockers for writing this scope:

- **M0a:** which two SSH aliases to test first and whether a custom tmux socket is in use.
- **M0b:** preferred TUI keys/launch binding and window versus popup presentation.
  These do not block the CLI milestone.
- **M1:** first real project (confirm rather than assume `example-app`), clone URL/base branch,
  required credentials, host allocation limits, database seed strategy and service ports.
- **M1/M2:** guest egress policy, explicit repo/LLM credential provisioning, stable
  browser-origin requirements, and whether stacks should autostart after VM boot.

## Evidence discipline

Unit tests may check parsers, quoting, operation transitions and port selection, but
must not substitute for the real SSH/tmux/VM/browser checks above. Use disposable test
sessions and data, never kill existing user work for a test. Record commands, observed
results and remaining blockers beside each milestone. Do not add code or provision
machines in a scoping-only change.


### Slice 7: controller host status aggregation

Add `argos hosts status` so the controller can call installed Argos helpers on each configured machine with bounded SSH, report helper versions, state/runtime paths and microVM capability, and filter to one host with `--host`.
