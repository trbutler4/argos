# Proposal: single-attachment session switching

Status: original design proposal, retained for context. The host-only implementation
is now included in this branch; see the [implementation review guide](single-attachment-implementation.md)
for current behavior, validation, and decisions that differ from this proposal.

## Goal

Make cross-host session switching feel like one workflow without nesting tmux or
retaining a chain of SSH connections. An Argos process in the original terminal
owns at most one interactive attachment at a time. Existing per-host tmux servers,
sessions, panes, and running processes stay where they are.

## Problem

The current sessions layer replaces Argos with a native tmux or SSH process when
attaching from outside tmux. No origin-side Argos process remains to coordinate
later switches. Inside tmux, local targets switch the current client, while
remote targets open a window running SSH and another tmux client.

A wrapper using `detach-client -E 'argos sessions attach ...'` does not solve
cross-host transport ownership. The replacement runs where that tmux client runs,
not necessarily in the original terminal's host. Switching between hosts can
retain earlier SSH transports even though each tmux client is replaced.

The desired invariant is **one active attachment, not one tmux server**.

## Scope / affected areas

- `src/main.rs`: an explicit managed entry point, provisionally
  `argos connect [TARGET]`.
- `src/sessions.rs`: reuse target resolution, validation, and command construction
  while adding a supervised child lifecycle instead of unconditional `exec`.
- `src/tui.rs`: reuse selection without permanently handing off execution.
- `tests/`: real PTY and isolated SSH lifecycle coverage.
- Downstream tmux configuration: a guarded return-to-picker binding and a
  coordinated package update after the implementation is tested and available.

Host sessions are the initial scope. VM attachment has a separate transport path
and must remain explicitly outside managed switching until separately validated.
Do not silently fall back to nested attachment for unsupported targets.

Non-goals: a central tmux server, remote-session migration, nested SSH windows,
a terminal renderer, a general daemon, or a reverse-forwarded control service.
Existing direct attachment behavior need not be broadly rewritten for this slice.

## Proposed user experience

```text
Original terminal
  Argos connection loop
    one active attachment:
      local tmux client
      OR SSH -> remote tmux client

return-to-picker key -> attachment exits -> local picker -> next attachment
```

- Launch managed mode from the original terminal, outside an existing tmux or
  SSH chain. Reject detectable nested entry and document the boundary rather
  than pretending to adopt or remove ancestor connections.
- The global picker key requests a return to origin-side Argos. Argos waits for
  the old attachment to exit before showing the picker and connecting elsewhere.
- Normal detach exits managed mode to the shell.
- Escape from a picker opened during attachment reconnects to the previous
  target. Escape from the initial picker exits. Failed reconnection remains a
  visible error with retry, choose-another-target, and quit options.
- Keep a separate same-server picker using native `switch-client` for immediate
  local session changes without reconnecting.

The initial global picker is local, not a popup over a still-attached remote
session. Opening it disconnects first, and canceling requires reconnection.
This avoids a remote control protocol but means a redraw and cold reconnect delay.

## Approach

1. Inspect the existing TUI handoff and attach integration tests. Factor only the
   target preparation and command construction needed by the managed path.
2. Add the managed connection loop using native terminal I/O:
   select target, spawn tmux or SSH, wait and reap, then interpret the outcome.
   Never start another attachment before the previous child has exited.
3. Reserve a fixed exit status for return-to-picker, provisionally `78`. The
   tmux action can use `detach-client -E 'exit 78'`: replace only the invoking
   client with a fixed exit command, never with another destination command.
   Preserve remote exit status through the SSH invocation and any wrappers.
4. Treat normal detach, picker requests, network errors, and signals distinctly.
   Only managed attachment mode interprets the reserved status as a request.
   Bound cleanup to owned processes and never kill sessions or unrelated clients.
5. Run inventory discovery from the origin between attachments, then finish all
   probes before connecting. Initially avoid background refresh during attachment.
   Preserve host-key verification, argument quoting, and destination validation.
6. Identify managed clients reliably before enabling the return binding. Scope
   metadata to the invoking client, not a shared tmux session environment.
   Unmanaged attachments should display guidance rather than unexpectedly detach.
7. Document startup, cancel, detach, failure recovery, and unsupported targets.
   Update the downstream package pin and bindings together only after validation.
   Do not automatically activate live configuration or stop existing connections.

### Connection ownership

The primary guarantee is at most one interactive attachment per managed terminal,
not one process or network connection across the user's entire machine. Remote
servers and jobs persist without attached clients.

Discovery may use short-lived SSH probes. If the desired guarantee covers every
Argos SSH transport, serialize probes as well as completing them before attach.
Do not retain old SSH masters to hide reconnection latency. Managed mode must not
reuse or terminate unrelated user-owned masters. Custom proxy commands and jump
hosts need explicit lifecycle testing before stronger process-count claims.

## Why not one tmux server?

A server owns its local PTYs and sessions. Consolidating work on one machine would
make session switching simple, but changes where workloads run. A central server
with plain SSH shells avoids nested tmux yet can retain many SSH connections and
does not provide remote tmux's independently reconnectable sessions if a
hub-to-host link fails.

Per-host servers plus an origin-side attachment manager preserve existing
placement and persistence without needing a distributed tmux server.

## Alternatives considered

- **SSH windows inside tmux:** rejected for this workflow because they introduce
  nested tmux and can accumulate further layers during cross-host switching.
- **Separate terminal tabs per host:** useful when retaining connections is
  acceptable, but does not meet the single-attachment goal.
- **Remote popup with a control channel:** could keep the current attachment
  until selection, then request an origin-side handoff. This requires per-client
  authentication, stale-request rejection, acknowledgment before detach, helper
  compatibility, and transport cleanup. Reverse Unix-socket forwarding is a
  candidate, but the current `ClearAllForwardings=yes` policy must be redesigned
  deliberately. Defer unless the simpler picker UX proves inadequate.
- **tmux control-mode or a PTY-intercepting frontend:** adds terminal integration
  and lifecycle responsibilities unnecessary for the first native-I/O solution.

## Validation

- Unit tests: target validation, reserved status, normal/error outcomes,
  cancellation, and connection-loop transitions.
- Real disposable tmux/PTY tests: reserved exit status, normal detach, unchanged
  pane PIDs, and exact-client behavior with two clients on one session.
- Real isolated SSH tests: remote status propagation, repeated host switching,
  authentication/connection failure, and complete owned-child cleanup.
- Stress test: 100 alternating-host switches with stable process depth and no
  accumulation of owned SSH connections or attached tmux clients.
- Terminal checks: resize, paste and clipboard, interrupts, suspend/resume, and
  restored terminal state after normal exit and failure.
- Existing repository formatting, unit, Clippy, build, and integration checks.
- Downstream Nix evaluation and binding checks before activation.
- Physical-host acceptance only against explicitly approved disposable sessions.

## Feasibility evidence and open decisions

An isolated local macOS tmux/PTY probe confirmed that replacing an attached client
with `exit 78` returned status 78, preserved the pane process, and left no attached
client. This is a mechanism check, not an implementation or end-to-end acceptance
result. SSH handoff and the complete switching loop still need real tests.

A candidate client-environment lookup returned empty during that probe. Reliable
managed-client identification is unresolved and must be verified against the
supported tmux versions rather than assumed from current online documentation.

Decisions before implementation:

1. Confirm disconnect-before-picker and reconnect-on-cancel as the initial UX.
2. Choose command naming and reserve a protocol status without confusing errors.
3. Decide whether the connection limit includes discovery transports.
4. Select and test client-specific managed-mode detection with safe fallback.
5. Define previous-target tracking when native same-server switching has occurred.
   Without tracking, cancel could return to the originally attached session
   rather than the session visible immediately before opening the picker.
6. Decide whether managed mode exits or returns to recovery UI after connection
   loss, while keeping intentional normal detach unambiguous.

## Reference semantics

The relevant contracts are documented in `tmux(1)` under `detach-client` and
session persistence, `ssh(1)` under exit status and remote forwarding, and
`ssh_config(5)` under `ClearAllForwardings` and `ControlPersist`. Those contracts
support the proposal but do not replace tests of the actual installed versions.
