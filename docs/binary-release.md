# Argos standalone binary

This archive targets **x86-64 Linux**. `bin/argos` is a static musl executable;
running it does not require Nix or a particular glibc version.

## Verify and install

Download the `.tar.gz` and matching `.tar.gz.sha256` from the same GitHub release.
From the download directory, replace `<version>` below with that release version:

```sh
sha256sum --check argos-v<version>-x86_64-linux.tar.gz.sha256
tar -xzf argos-v<version>-x86_64-linux.tar.gz
install -Dm755 argos-v<version>-x86_64-linux/bin/argos "$HOME/.local/bin/argos"
argos --version
```

Ensure `$HOME/.local/bin` is on PATH. Replacing this executable upgrades Argos;
it does not modify inventory, VM data, SSH configuration, or running sessions.

## Runtime tools

- Session discovery/attachment needs `tmux` and OpenSSH (`ssh`) on PATH.
- VM lifecycle also needs Nix, local KVM access, and the existing VM host setup.
- Git operations need `git`. The binary does not bundle these native tools.

For a Nix-managed installation with runtime tools, prefer the tagged flake:

```sh
nix run github:trbutler4/argos/v<version> -- --version
```

## Optional shell and tmux integration

Shell completion scripts are under `share/` in the usual bash/zsh/fish layout.
Alternatively register dynamic completion directly:

```sh
# zsh
eval "$(COMPLETE=zsh argos)"
# bash
eval "$(COMPLETE=bash argos)"
# fish
COMPLETE=fish argos | source
```

`share/argos/connect.tmux.conf` contains the optional F12 return-to-picker binding.
Review the destination server's existing F12 binding before sourcing it. It
requires Linux `/proc` and GNU grep. Nothing in this archive activates bindings
automatically; Argos connection ownership is enabled regardless.

Start `argos tui` or `argos sessions attach ...` in the original terminal,
outside tmux and SSH. See `CHANGELOG.md` for compatibility changes.
