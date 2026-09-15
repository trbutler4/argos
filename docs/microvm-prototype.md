# MicroVM prototype notes

This is the first Argos VM backend spike. It uses `microvm.nix` in runner-package
mode so the VM can be built and booted from the repository without installing a
host-level `microvm@...` systemd service or rebuilding the NixOS host.

## Current prototype

- Flake input: `github:microvm-nix/microvm.nix` pinned in `flake.lock`.
- Runner package: `.#argos-microvm-prototype` on `x86_64-linux`.
- Guest config: `nix/microvm/prototype.nix`.
- Hypervisor: QEMU through microvm.nix.
- Runtime state: created in the directory where the runner is launched.
- Persistent guest `/var`: `var.img` beside the runner invocation directory.
- Host `/nix/store`: shared read-only through 9p to avoid building a full root
  filesystem image for every spike.
- Included guest tools: `git` and `tmux`.
- Authentication: no SSH yet. The root password is locked. Console autologin is
  used only for this prototype boot path.

## Verified on 2026-09-15

Host checks:

- `/dev/kvm` exists and is usable by the current user.
- The current user is in the `kvm` group.
- No host NixOS module, tap device, bridge, sudo action, or system rebuild was
  required.

Commands exercised:

```sh
nix build .#argos-microvm-prototype
```

Then the runner was launched from a scratch directory so `var.img` and the
control socket did not land in the repository. The VM emitted the readiness
marker on its console:

```text
ARGOS_MICROVM_READY host=argos-microvm-prototype
```

Observed boot-to-marker time was about 15 to 16 seconds in scratch-run tests.

## Important limitations

This proves only that the chosen backend can build and boot a minimal NixOS
MicroVM without host setup. It does **not** yet prove:

- SSH into the guest.
- Stable guest host-key identity.
- Host-loopback port forwarding.
- Separate per-task writable clone/state directories.
- Running a real project or `devenv up` inside the guest.
- Stop/start persistence across a guest reboot.
- Browser access to guest services.

## Next slice

Add an SSH-enabled local-only prototype without committing secrets:

1. Generate runtime-only guest SSH host keys and an authorized client key under an
   ignored/scratch state directory.
2. Add QEMU user networking with host-loopback forwarding for guest port 22.
3. Boot the VM from a managed runtime directory.
4. Verify `ssh -p <forwarded-port> argos@127.0.0.1 true` with strict host-key
   checking and a stable `HostKeyAlias`.
5. Start a guest tmux session and prove Argos can discover or attach through the
   forwarded SSH path.

Only after that should Argos grow `env create/start/status/attach` commands.
