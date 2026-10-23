# Guest profiles, Android development and review artifacts

Argos owns VM allocation, persistent workspace/storage, declared guest tooling,
loopback ports and file retrieval. NixOS/systemd owns guest boot services. Project
scripts own Metro, builds, backend processes, Maestro flows and phone installs.
A running VM is not a claim that an emulator or application is ready.

## Resources and declarative modules

```toml
[vm]
packages = ["nodejs_24", "jdk21", "maestro"]
modules = ["nix/android.nix", "nix/project-guest"]

[vm.resources]
cpus = 4
memory_mib = 6144
var_disk_mib = 32768
store_disk_mib = 16384

[[ports]]
name = "metro"
guest = 8081
```

Omitted resources retain the old defaults: 4 CPUs, 4096 MiB RAM, 1024 MiB
`/var`, and 8192 MiB writable Nix overlay. Schema-1 VM records without these
fields remain readable by the new CLI. Older Argos binaries reject the newly
written fields because records use strict field validation. Use the new binary
for all commands after upgrading a VM, and retain private state backups before
trying a downgrade. `vm show --json` reports resolved resource intent.
Values must be positive and within the CLI's validation bounds. Start emits
non-blocking capacity warnings on Linux when available memory or recorded
running allocations indicate pressure. These are estimates, not reservations,
and include neither exact QEMU overhead nor workload-specific peaks.

CPU/RAM changes take effect after stop/start. Argos does not resize filesystems
or truncate existing disks. Changing a disk allocation with an existing image
is rejected unless the raw image already has exactly the requested byte size,
which allows declaration of a previously customized allocation without touching
its data. Stop always preserves disks. Plan actual disk growth separately.

`vm.modules` paths are relative to the profile file, not your shell's working
directory. Absolute paths are also accepted. A `.nix` file must be self-contained.
For relative imports and `builtins.readFile`, use a directory bundle with a
`default.nix` entry point. Only explicitly declared files/bundles are copied.

Modules are **trusted executable Nix configuration**, not sandboxed extensions.
They can accept licenses, configure persistent cache paths and define native
systemd services. Never include secrets: module inputs enter the Nix store.
Do not point a module declaration at an entire application repository or home.
Symlinks, sockets, devices and `.git` directories in module bundles are rejected.
The aggregate source limit is 4096 entries, 64 MiB and 32 directory levels, with
at most 32 declared modules.

Create/update validates and snapshots modules into the instance's managed
`guest-modules/` directory. Start uses that snapshot, even if the original source
moves or changes. Explicit `vm update` refreshes modules and regenerates imports;
it no longer loses custom guest configuration. An update does not restart a VM.
To change module policy, edit the declared source, not generated instance files.

## Clean flake sources

Every runner build uses a private temporary source containing top-level `.nix`
files, `flake.lock`, the generated guest tmux config and managed module snapshots.
The runtime directory remains the working directory for QEMU disks and sockets,
but is never passed wholesale to Nix. No `git init` or `git add` workaround is
needed. Legacy top-level `.nix` imports remain supported; undeclared arbitrary
subdirectories are intentionally excluded. Move those into a module bundle.
Generated lock changes are copied back atomically, with a concurrent-change check.
Staging is removed after both successful and failed builds. Existing runtime
files, SSH host trust, disks and legacy instance Git repositories are untouched.

## Android example

`examples/android/profile.toml` opts into API 36 Google APIs x86_64, the headless
Android emulator, JDK 21, Node 24, Maestro and persistent tool caches. Its module
accepts Android licenses and enables a native systemd emulator service. Review
that policy before using it. Nested KVM must be enabled on a capable Linux host.
No Play Store, desktop Android Studio, GPU passthrough or physical USB access is
required. No ADB/emulator ports are forwarded to the host by default.

```sh
argos vm create "Android task" --id android-task --repo /path/to/repository \
  --profile /path/to/argos/examples/android/profile.toml
argos vm start android-task
argos vm shell android-task
# Inside the guest:
systemctl status android-emulator
journalctl -u android-emulator
adb -s emulator-5554 shell getprop sys.boot_completed
maestro --device emulator-5554 test /workspace/repo/path/to/flow.yaml
```

The example keeps AVD, local system image, caches and temporary files on `/var`,
not the small RAM-backed root filesystem. Image staging avoids repeated random
reads over the shared Nix store. Yarn concurrency and JavaScript heap limits are
bounded, and Maestro gets a longer instrumentation startup deadline for nested
virtualization. A task may still need different resources or a smaller image.
Measure two concurrent guests before multiplying heavyweight Android workers.
Cold Android/native/JavaScript initialization can be slow. The example does not
start Metro or a backend, inject application credentials, register authentication
clients, or verify passkeys. Repo tooling owns those choices.

## Fetch a review build

Build the APK using the project's existing native build/signing workflow, then:

```sh
argos vm fetch android-task /workspace/repo/build/review.apk \
  --output ./review.apk --json
adb devices -l
adb -s YOUR_PHONE_SERIAL install -r ./review.apk
```

Fetch requires a running VM and existing verified guest `known_hosts`, performs
batch SSH without accepting or changing host trust, streams the file to a private
temporary file, and checks SHA-256 before/after transfer and against the local
copy. It publishes only a verified file and refuses to overwrite an existing
output. JSON reports retrieval metadata, not source-to-build provenance. A
matching hash proves transfer integrity, not that an APK was built from the
current commit. No VM lifecycle action, app uninstall, device selection, signing
key copying, phone installation or release publication is performed by fetch.

Use a standalone ARM64 build for a typical Android phone, not an x86_64 emulator
APK or a Metro-dependent dev client. Preserve the project's development signing
identity when replacing an existing development app. App/backend environment
values must be baked or configured through the project's own workflow.
