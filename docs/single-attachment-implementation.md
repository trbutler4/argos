# Single-attachment switching: review guide

Implementation of [PR #1's design outline](https://github.com/trbutler4/argos/pull/1),
reviewed at `359a0d656d5c49273abc567f990e0d610f02657d`.

## Implemented scope

`argos connect [TARGET]` owns one interactive host-tmux attachment from the
original Linux terminal. It supports local tmux and direct SSH to existing
sessions, with the same inventory and exact target selection as `sessions attach`.
Without a target it opens a host-only picker. Existing direct attach and TUI
entry points retain their native behavior.

| Event | Behavior |
| --- | --- |
| Initial picker: Enter | Attach to selected host session |
| Initial picker: Escape / q | Exit to shell |
| Managed attachment: guarded F12 | Detach invoking client, reap transport, open origin picker |
| Return picker: Enter | Attach selected target |
| Return picker: Escape / Shift-R | Reconnect last Argos-selected target |
| Picker: q / Ctrl-C | Exit (Ctrl-C returns 130) |
| Native tmux detach | Exit managed mode to shell |
| Connection or target failure | Show error, selection, retry with Shift-R, and quit |
| Origin INT / TERM / HUP | Terminate owned attachment, restore terminal, exit 128 + signal |

### Decisions relative to the proposal

- **Cancel reconnects the last Argos-selected target**, using its resolved tmux
  session ID. A native same-server `switch-client` does not update that target.
  The return picker states this explicitly. A fixed exit status cannot convey
  the last visible session, so this version does not claim to track it.
- **The limit covers interactive attachments.** Read-only discovery can use
  bounded concurrent probes between attachments. All probes finish before the
  next interactive child starts; there is no discovery during attachment.
- **Host-only, Linux-only, direct SSH.** VM targets are rejected. ProxyCommand
  and ProxyJump are detected using `ssh -G` and rejected before connection.
  Cleared environment variables cannot prove that an ancestor SSH/tmux chain
  does not exist; detectable nesting is rejected, not adopted.
- **Disconnect before picking.** Escape needs a new connection and may fail.
  Errors return to recovery UI instead of automatically retrying in a loop.

## Code map

- `src/main.rs`: `connect [TARGET]` and inventory-selection flags.
- `src/sessions.rs`: shared exact selection and command construction; managed
  target validation, client marker, transport policy, and reconnect ID.
- `src/connect.rs`: origin loop, native foreground process-group handoff,
  signal handling, child reaping, and terminal restoration.
- `src/tui.rs`: host-only selection using the existing browser model; distinct
  retry/quit outcomes, visible recovery message, no VM state reads in this mode.
- `src/completions.rs`: host-only candidates for `connect`, without remote probes.
- `examples/connect.tmux.conf`: optional guarded F12 binding, also installed at
  `$out/share/argos/connect.tmux.conf` by the Nix package.
- `tests/connect.py`: disposable PTY/tmux/sshd lifecycle and stress checks.

## Client identification and binding

Managed tmux clients receive `ARGOS_CONNECT_CLIENT=1` in their own process
environment. The Linux binding reads `/proc/#{client_pid}/environ` with GNU
`grep -zqx`, using tmux's invoking-client context. A marker on one client does
not authorize detaching another client on the same session. Missing or unreadable
markers show guidance and leave the client attached. This is a UX guard, not an
authentication boundary against processes running as the same user.

Managed attach uses tmux `-E` so the marker is not copied through a customized
`update-environment` list into shared session state. This also means managed
attach does not refresh other session environment variables. No server options,
hooks, or keybindings are installed by `connect` itself.

The opt-in binding runs only the fixed `detach-client -E 'exit 78'` action.
It never puts destination names into shell code or launches another connection
on the destination host. Only managed mode interprets 78 as a picker request.

## Ownership and terminal details

Each attachment inherits native terminal I/O in a new foreground process group.
Argos remains in its original group. `waitid` observes child completion without
reaping the group leader; cleanup can therefore signal the owned group without
PID-reuse ambiguity. The old group is cleaned up and its leader reaped before
the picker can start another attachment. tmux servers and their panes are outside
this group and persist.

Catchable origin termination gets a bounded grace period, then forced cleanup.
The origin restores its foreground group, saved terminal attributes, and native
display modes. `SIGTTOU` is blocked only while handing off the foreground group.
Stopped clients suspend the origin too; `fg` resumes the same attachment. This
uses safe `nix` and `signal-hook` APIs, with no application `unsafe` code.

Managed SSH disables ControlMaster, ControlPath, ControlPersist, agent and port
forwarding, local commands, background-after-authentication, and SSH escape
commands. Strict host-key checking and batch authentication remain enabled.
ConnectTimeout bounds connection setup; keepalives detect dead links without
imposing a duration limit on a healthy attachment. Unrelated masters remain alive.

SIGKILL of the origin cannot run cleanup. Arbitrary daemonizing SSH configuration
helpers and custom wrappers that escape the owned process group are outside the
tested ownership contract. Proxies/jump hosts remain explicitly unsupported.

## Validation and remaining human checks

Validated with the pinned Linux development shell, tmux 3.7c and OpenSSH 10.5p1:

- Formatting, all 35 Rust tests, Clippy with `--all-targets -- -D warnings`,
  locked build, and every contributor-required Python integration script passed.
- `nix build .# --no-link` passed. The packaged binary and installed binding also
  passed `tests/connect.py`, including the full 100-switch run.

- Local and SSH return-status propagation, native detach, and two-client isolation.
- Guarded binding on managed and unmanaged clients sharing a session, including
  a customized tmux environment-update list.
- Initial cancel, reconnect-on-cancel, explicit quit, failed-target retry and
  choosing another target; native same-server switching followed by cancel.
- 100 alternating switches between two logical remote hosts on an isolated
  loopback sshd, with separate tmux sockets. Each iteration checks one owned
  attachment child and disappearance of the previous SSH process before selection.
- Local-to-remote switching, unchanged pane PIDs, and preservation of a second
  attached client and an unrelated fixture-owned SSH master.
- Resize, origin INT/TERM/HUP, local and remote suspend/resume, and real interactive
  Bash job control using tmux's native suspend binding and `fg`.
- Bracketed paste through SSH and Ctrl-C reaching the remote pane without
  terminating the origin-side owner.
- Forced SSH transport loss and recovery; host-key and authentication failures;
  proxy rejection before the configured proxy command executes.

The full automated check command is in README. Physical-host latency and a real
terminal's clipboard integration still need human acceptance. Tests use disposable
host sessions rather than VMs because VM transport is outside this feature's scope.
No physical host inventory was used for connections or configuration activation.

### Human review path

1. Read the cancel rule and ownership limits above; review `src/connect.rs` and
   `examples/connect.tmux.conf` first.
2. From a plain terminal, create disposable tmux sessions on chosen hosts and
   select them with `argos connect --config /path/to/private-inventory.toml`.
3. Review the destination's current F12 binding, then explicitly source the
   example on the disposable tmux server or choose a different unused key.
4. Switch between hosts, press Escape to reconnect, then test normal detach,
   connection failure, resize, paste, clipboard, and native suspend/resume.
5. Confirm working processes survive and another client remains attached.
   Update downstream package pins and live bindings only after accepting this UX.

No downstream configuration is activated by this change.
