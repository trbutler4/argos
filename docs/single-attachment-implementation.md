# Single-attachment switching: review guide

Implements [PR #1's outline](https://github.com/trbutler4/argos/pull/1), with the
updated requirement that **Argos always owns interactive connections**. The
original proposal remains historical context, not the current behavior contract.

## Default behavior

`sessions attach`, `tui`, and `connect` share one Linux origin-side supervisor
and host/VM picker. `vm tmux`, `vm shell`, and the attachment phase of `vm up` /
`task up` also keep that owner. The direct-exec attachment, implicit same-server
switch, and remote SSH-window fallback have been removed. Native tmux commands
can still switch sessions on the current server.

Connection ownership is mandatory. Installing the guarded return-to-picker
binding is optional and never happens automatically.

| Event | Behavior |
| --- | --- |
| Initial picker: Enter | Attach to selected host or VM session |
| Initial picker: Escape / q | Exit to shell |
| Guarded F12 in tmux | Detach invoking client, reap transport, open origin picker |
| Return picker: Enter | Attach selected target |
| Return picker: Escape / Shift-R | Reconnect last Argos-selected target |
| Picker: q / Ctrl-C | Exit (Ctrl-C returns 130) |
| Native tmux detach | Exit Argos to shell |
| Connection or target failure | Show error, selection, retry, and quit |
| Origin INT / TERM / HUP | Clean up owned attachment and exit 128 + signal |
| Detectable nested tmux/SSH entry | Reject before opening another connection |
| Nested VM/task up | Reject before mutation unless --no-attach / --json |

Escape reconnects the last **Argos-selected** target, using the resolved host
tmux session ID or VM ID. Native same-server switches do not update that target;
the picker states this. Selection disconnects first, so cancellation needs a
new connection and can fail.

VM shell is supervised plain SSH, not tmux. It has no return-binding marker;
a normal shell exit returns to the caller. A shell exit of 78 is not interpreted
as a picker request.

## Code map

- `src/main.rs`: command aliases and CLI argument dispatch.
- `src/sessions.rs`: `BrowseOptions`, typed `SessionTarget` / `GuestMode`, and
  native command preparation. `PreparedAttachment` returns the command and
  canonical reconnect target without modifying the caller's selection.
- `src/connect.rs`: selection/retry policy and interpretation of process outcomes.
- `src/terminal.rs`: Linux process groups, signals, cleanup, reaping, and terminal
  restoration. It reports `ProcessOutcome` and knows nothing about picker status 78.
- `src/ssh.rs`: shared interactive SSH policy and effective-config validation.
- `src/process.rs`: bounded captured-process runner shared by discovery helpers.
- `src/tui.rs`: host/VM rendering and typed selection/retry/quit outcomes. It
  receives the discovery request from the controller instead of doing SSH preflight.
- `src/vm.rs`: guest SSH preparation, unmarked guest tmux bootstrap, and the
  origin check before VM/task up mutation.
- `examples/connect.tmux.conf`: optional guarded F12 binding, also installed at
  `$out/share/argos/connect.tmux.conf` by the Nix package.
- `tests/connect.py`: default-TUI switching stress and PTY/SSH lifecycle checks.
- `tests/vm_connect.py`: opt-in real QEMU/KVM guest connection acceptance.

### Rust boundaries

```text
CLI text -> SessionTarget + BrowseOptions
                 |
            connect controller
              /      |       \
      typed picker   |    terminal owner -> ProcessOutcome
                     |
          sessions::prepare -> PreparedAttachment
                     |
             host / VM command builders -> shared SSH policy
```

Target kinds use enum variants, so host targets cannot carry VM shell mode.
Browser filters and VM state roots are separate from the selected target. CLI
text is parsed at entry; the TUI and VM commands construct typed targets directly.
Preparing a target performs discovery/preflight I/O, but returns its result
explicitly instead of rewriting an options bag. The owner is borrowed by the
picker while idle and supervises one owned child while attached.

Resource cleanup uses concrete RAII guards. The UI screen guard is acquired
before fallible backend initialization, so partial setup also restores terminal
modes. The process owner preserves typed exit/signal/spawn outcomes until the
controller applies connection policy; a shell's exit 78 cannot accidentally
become a tmux picker request.

Packaged interrupt testing exposed transient `WouldBlock` while flushing terminal
reset sequences. Cleanup now retries the buffered flush with a deadline rather
than re-emitting partially written commands, and interrupted wait calls resume
the signal loop. PTY checks also assert restoration of descriptor flags.

Remaining architecture work is incremental: move blocking discovery off the UI
thread (joining/canceling probes before attachment), retain structured domain
errors longer, and separate the large VM module's lifecycle/state/configuration
responsibilities. These do not require a generic transport framework.

## Client identification

Only an attaching tmux client receives `ARGOS_CONNECT_CLIENT=1`. The Linux
binding reads `/proc/#{client_pid}/environ` using GNU `grep -zqx` and tmux's
invoking-client context. Missing/unreadable markers show guidance without
detaching. This distinguishes clients sharing a session, not users or trust
boundaries: processes running as the same user can forge the marker.

Attach uses tmux `-E` to keep the marker out of a customized `update-environment`
list. Consequently, other session environment variables are not refreshed on
attach. Guest tmux is created with the marker unset before starting its marked
client, so a new guest server and its panes do not inherit the marker.

The binding runs the fixed action `detach-client -E 'exit 78'`. Destination
names never become executable handoff commands. Only tmux attachments interpret
78 as a picker request.

## Ownership and limits

Each attachment runs with inherited terminal I/O in its own foreground process
group. Argos stays in its original group. `waitid` observes completion without
reaping the group leader, preventing PID reuse while signalling that group.
The old group is cleaned up and reaped before the next picker or attachment.
tmux servers and panes remain outside the owned attachment group.

Catchable termination has a bounded grace period before forced cleanup. Argos
restores foreground ownership, terminal attributes, and display modes. TTOU is
blocked only during foreground handoff. Stopped clients suspend the origin;
`fg` resumes the same attachment. All application code uses safe Rust APIs.

All interactive SSH disables connection sharing/persistence, agent and port
forwarding, local commands, background authentication, and SSH escape commands.
Strict host-key checking and batch authentication stay enabled. Setup deadlines
and keepalives do not limit the lifetime of a healthy attachment. VM trust retains
the existing first-use enrollment model but never overwrites a recorded key
during reconnect; changed or invalid recorded keys fail SSH verification.

Supported: Linux origins and destinations, direct SSH host sessions, local VM
tmux and shells. ProxyCommand/ProxyJump configurations are explicitly rejected.
Read-only discovery may probe hosts concurrently between attachments; probes
finish before attaching. Clearing nesting variables cannot establish that no
ancestor transport exists. SIGKILL cannot run origin cleanup, and arbitrary
daemonizing custom SSH helpers are outside the tested ownership contract.

## Validation

Pinned Linux development shell: tmux 3.7c, OpenSSH 10.5p1.

All checks below passed, including 33 Rust tests, the Nix package build, the
packaged binary/binding running the full 100-switch TUI test, and real VM acceptance.

- Rust formatting, tests, Clippy, locked build, and contributor Python scripts.
- Real local/SSH PTYs: default `sessions attach` retains its owner, nested pane
  entry fails without opening windows, and unrelated clients/panes survive.
- 100 switches through normal `tui` between two logical loopback SSH hosts;
  each iteration checks one attachment child, reaping before selection, and
  preservation of an unrelated fixture-owned SSH master.
- Cancel/retry/quit, failed targets, host-key/authentication failures, proxy
  rejection, transport loss, resize, bracketed paste, Ctrl-C, signals, and
  real Bash suspend/resume job control.
- A disposable QEMU/KVM VM: guest bootstrap marker isolation, VM tmux/shell and
  sessions entry, host/VM switching through TUI, reconnect, pane persistence,
  explicit VM state roots without inventory, pinned guest trust, and cleanup.
  Generated credentials and isolated
  HOME/state/tmux servers prevent using personal credentials or sessions.

Commands live in README. `tests/vm_connect.py` boots a VM and must only be run
with permission; it stops and removes its own fixture. Physical-host latency,
real-terminal clipboard integration, and daily-use UX remain human acceptance.

## Human review path

1. Review `src/connect.rs`, the session/VM command builders, and the binding.
2. Start `argos tui --config /path/to/private-inventory.toml` in a plain terminal.
   Try both host and VM rows and the `sessions attach` / `vm tmux` aliases.
3. Review existing F12 bindings before explicitly sourcing the example on
   disposable destination servers, or select a different unused key.
4. Switch, cancel, detach, resize, paste, test clipboard, and suspend/resume.
5. Confirm work and other clients survive. Update downstream pins and live
   bindings only after accepting the UX. This change performs no activation.
