# Implementation plan

**M0a is in progress**, beginning with a small local-only CLI slice. Other
implementation milestones are not started. The cross-machine acceptance checks
below remain unproven until exercised on the real hosts.

### Slice 1: local listing

Implemented: pinned Rust dev shell, `list`, `list --json`, optional `--socket`,
local hostname identity, versioned snapshots and explicit empty/error behavior.
See README for commands to run this slice and its real-tmux integration checks.
This is a subset of M0a, not completion of A01–A06: remote discovery, timeouts,
attachment and terminal handoff still need implementation and real-host testing.
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

Current limits: local subprocess queries are sequential and do not yet have deadlines,
so a wedged tmux server can stall discovery. Deadlines and partial-host behavior belong
to the next discovery slice. The snapshot is not atomic across concurrent session
changes. A Nix release package is available via `nix build`/`nix run`. Generic
Home Manager installation instructions are in README. System activation is
user-controlled.


Release boundaries: **M0a (CLI) is the first usable release** without VM/browser
features. **M0b adds the full-screen TUI over CLI JSON output.**
**M2 is the initial complete managed-environment workflow**. M1 is its prerequisite
feasibility proof, and M3 is optional polish.

## M0: one interface for existing tmux work

### M0a: CLI foundation (first usable release)

**Deliverable:** Rust `list` and `attach` commands that work across the real hosts.
The TUI is not a prerequisite for using or testing this release.

1. Initialize the Cargo crate and a pinned Nix development shell for Rust/Cargo,
   rustfmt, Clippy and required CLI tools. Do not add a TUI framework yet.
2. Parse the explicit machine inventory using existing SSH aliases. Define stable
   IDs, observed/cached state and the versioned JSON envelope described in architecture.md.
3. Discover sessions concurrently with deadlines and structured per-host errors.
   No required remote installation beyond existing SSH and tmux in this milestone.
4. Implement human-readable `list`, `list --json` and interactive `attach`. Keep JSON
   stdout clean and test exit codes, partial failures and empty results.
5. Implement same-server local switching and remote connection-window handoff inside
   tmux, plus attachment from a plain terminal. Preserve the user's existing bindings.

M0a exit: complete the CLI portions of A01–A06 on two real machines, including the
persistent-process and terminal checks. CLI output must be ready for another program
to consume before starting the TUI.

### M0b: TUI over the working CLI

**Deliverable:** a full-screen Rust TUI using the M0a command/output contract.

1. Choose a Rust TUI framework. Add `cmd-center tui` with keyboard navigation,
   search/filter, selected-session details, host status and contextual help.
2. Invoke CLI read commands asynchronously with `--json`; render structured data and
   errors rather than scraping terminal tables. Keep filtering/selection in the TUI,
   but all host operations in the CLI. Handle schema mismatch and failed subprocesses.
3. Hand the terminal to CLI `attach` and restore/redraw the TUI afterward. Test inside
   and outside tmux; attach must not be treated as captured JSON output.
4. Add an optional user-selected tmux launch binding. Do not overwrite the existing
   prefix, session picker, clipboard setup or navigation bindings.

Acceptance on the user's real machines:

- **A01:** On two real hosts, M0a human and JSON listings represent the same ordinary
  sessions without project/VM registration. Parse the actual CLI stdout, verify stable
  IDs/timestamps/schema version and ensure diagnostics cannot corrupt JSON. In M0b,
  the TUI displays those results, supports keyboard search/navigation/help and remains
  usable when resized. No duplicate host discovery implementation is introduced.
- **A02:** Attach to a disposable test session on each host. Record shell/process PID,
  run a counter, switch away and reconnect. Same remote process remains alive and
  the counter advanced. Removing only the SSH client must not kill the session.
- **A03:** Run CLI attach inside and outside tmux in M0a. Same-server attach does not
  nest; prefix forwarding, resize and terminal cleanup work. Repeat through the TUI
  in M0b, including terminal handoff and return-to-dashboard.
- **A04:** Test an unreachable host, an authentication error, an untrusted host key,
  missing tmux and zero sessions. Each is distinct, and a bad host cannot freeze
  CLI completion or hide successful results from other hosts. JSON partial results
  and exit codes agree. In M0b, these errors do not freeze navigation or redraws.
- **A05:** Existing bindings and sessions remain unchanged. Verify OSC 52 clipboard
  end-to-end through the actual terminal and remote tmux, not just a config flag.
- **A06:** Session labels containing spaces/quotes/metacharacters are safely displayed
  and attached, never evaluated as shell commands. In M0b, canceling selection does nothing.

Initial responsiveness targets to measure: render cached/local results immediately,
finish healthy-host discovery within 2 seconds on the tailnet, and bound an individual
unreachable-host attempt to 3 seconds. These are targets, not performance claims.

Exit: M0a ships independently as the CLI. M0b then demonstrates real task switching
through CLI-backed TUI views across two machines. Record the CLI contract, chosen
TUI framework and attach UX before adding VM management.

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
2. Pin microvm.nix/NixOS/devenv versions and create a QEMU/KVM guest prototype.
   Record any required one-time privileged host configuration before applying it.
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

## M2: manage task environments through cmd-center

**Deliverable:** the same interface lists unmanaged host sessions and managed VM work.

Work:

- Introduce a host-owned environment registry, stable IDs, locks and operation status.
- Wrap the demonstrated M1 lifecycle in `env create/start/stop/inspect`; no arbitrary
  root shell or broad sudo permissions. Use a small SSH-invoked helper only as needed.
- Make long create operations supervised and reconnectable with operation IDs.
- Discover/attach guest tmux sessions using stable guest SSH aliases through the host.
- Implement `open` with owned, persistent client-side tunnels and accurate local URLs.
- Show host reachability, VM power state, stack readiness and session attachment as
  separate states. Logs/errors remain accessible when readiness fails.
- Add narrowly scoped host and Home Manager modules for the user's NixOS repo.
- Add explicit `destroy` only after preservation and confirmation checks exist.

Acceptance:

- **A14:** From client A, create a task on host B, attach, edit/run it and open its app.
  Switch to another task and back without restarting either backend.
- **A15:** From another configured client, find and attach the same environment on B,
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
