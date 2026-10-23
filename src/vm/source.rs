//! Explicit, bounded Nix inputs. Runtime disks, sockets and SSH keys never enter
//! the flake source, and module snapshots do not depend on Git tracking tricks.
use super::{VmError, now_ms, state_error};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;

const MAX_FILES: usize = 4096;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
struct Budget {
    files: usize,
    bytes: u64,
}

impl Budget {
    fn copy(&mut self, source: &Path, destination: &Path) -> Result<(), VmError> {
        self.visit(source, Some(destination), 0)
    }
    fn visit(
        &mut self,
        source: &Path,
        destination: Option<&Path>,
        depth: usize,
    ) -> Result<(), VmError> {
        self.files += 1;
        if depth > 32 || self.files > MAX_FILES {
            return Err(state_error(
                "Nix inputs exceed the 4096 entry / 32 directory depth limit",
            ));
        }
        let metadata = fs::symlink_metadata(source).map_err(|e| input_error(source, e))?;
        if metadata.file_type().is_symlink() {
            return Err(state_error(format!(
                "Nix module inputs must not be symlinks: {}",
                source.display()
            )));
        }
        if metadata.is_dir() {
            if let Some(destination) = destination {
                fs::create_dir(destination).map_err(|e| input_error(destination, e))?;
            }
            let mut entries = fs::read_dir(source)
                .map_err(|e| input_error(source, e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| input_error(source, e))?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                // A declared bundle is source, not a repository or cache snapshot.
                if entry.file_name() == ".git" {
                    return Err(state_error(format!(
                        "Nix module bundle must not contain .git: {}",
                        source.display()
                    )));
                }
                let target = destination.map(|dir| dir.join(entry.file_name()));
                self.visit(&entry.path(), target.as_deref(), depth + 1)?;
            }
        } else if metadata.is_file() {
            self.bytes = self.bytes.saturating_add(metadata.len());
            if self.files > MAX_FILES || self.bytes > MAX_BYTES {
                return Err(state_error(
                    "Nix inputs exceed the 4096 file / 64 MiB source limit",
                ));
            }
            if let Some(destination) = destination {
                fs::copy(source, destination).map_err(|e| input_error(source, e))?;
            }
        } else {
            return Err(state_error(format!(
                "Nix inputs must be regular files or directories: {}",
                source.display()
            )));
        }
        Ok(())
    }
}

fn input_error(path: &Path, error: std::io::Error) -> VmError {
    state_error(format!(
        "cannot prepare Nix input {}: {error}",
        path.display()
    ))
}

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn new(parent: &Path, kind: &str) -> Result<Self, VmError> {
        let path = parent.join(format!(
            ".argos-{kind}-{}-{}-{}",
            std::process::id(),
            now_ms(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&path).map_err(|e| input_error(&path, e))?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Validates declared sources before mutations, including dry-run. Single files
/// must be self-contained .nix modules. Bundles must have a default.nix entry.
pub(super) fn resolve_modules(profile: &Path, modules: &[String]) -> Result<Vec<String>, VmError> {
    if modules.len() > 32 {
        return Err(state_error("at most 32 guest modules may be declared"));
    }
    let base = profile.parent().unwrap_or(Path::new("."));
    let mut budget = Budget::default();
    modules
        .iter()
        .map(|module| {
            if module.is_empty() || module.chars().any(char::is_control) {
                return Err(state_error(
                    "vm.modules must contain nonempty module paths without control characters",
                ));
            }
            let path = base.join(module);
            let metadata = fs::symlink_metadata(&path).map_err(|e| input_error(&path, e))?;
            if metadata.file_type().is_symlink() {
                return Err(state_error(format!(
                    "guest module source must not be a symlink: {}",
                    path.display()
                )));
            }
            let path = fs::canonicalize(&path).map_err(|e| input_error(&path, e))?;
            if metadata.is_dir() {
                let entry = path.join("default.nix");
                let entry_metadata =
                    fs::symlink_metadata(&entry).map_err(|e| input_error(&entry, e))?;
                if !entry_metadata.is_file() || entry_metadata.file_type().is_symlink() {
                    return Err(state_error(
                        "guest module bundles require a regular default.nix",
                    ));
                }
            } else if !metadata.is_file() || path.extension().is_none_or(|ext| ext != "nix") {
                return Err(state_error(
                    "guest modules must be .nix files or directories with default.nix",
                ));
            }
            budget.visit(&path, None, 0)?;
            Ok(path.to_string_lossy().into_owned())
        })
        .collect()
}

/// Capture explicit module sources at create/update, not at every start. Nix
/// modules are trusted code and these inputs are public to the Nix store.
pub(super) fn snapshot_modules(instance: &Path, sources: &[String]) -> Result<(), VmError> {
    let temporary = TemporaryDirectory::new(instance, "modules")?;
    let mut budget = Budget::default();
    for (index, source) in sources.iter().enumerate() {
        let source = Path::new(source);
        let target = temporary.0.join(index.to_string());
        if source.is_dir() {
            budget.copy(source, &target)?;
        } else {
            fs::create_dir(&target).map_err(|e| input_error(&target, e))?;
            budget.copy(source, &target.join("default.nix"))?;
        }
    }
    let destination = instance.join("guest-modules");
    let backup = TemporaryDirectory::new(instance, "previous-modules")?;
    let previous = backup.0.join("modules");
    if let Ok(metadata) = destination.symlink_metadata() {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(state_error("managed guest-modules path is not a directory"));
        }
        fs::rename(&destination, &previous).map_err(|e| input_error(&destination, e))?;
    }
    if let Err(error) = fs::rename(&temporary.0, &destination) {
        if previous.exists()
            && let Err(restore) = fs::rename(&previous, &destination)
        {
            let preserved = previous.display().to_string();
            std::mem::forget(backup);
            return Err(state_error(format!(
                "cannot publish guest modules: {error}; rollback failed: {restore}; previous modules preserved at {preserved}"
            )));
        }
        return Err(input_error(&destination, error));
    }
    Ok(())
}

pub(super) struct BuildSource {
    directory: TemporaryDirectory,
    instance: PathBuf,
    original_lock: Option<Vec<u8>>,
}
impl BuildSource {
    pub(super) fn new(instance: &Path) -> Result<Self, VmError> {
        let original_lock = read_optional(&instance.join("flake.lock"))?;
        let directory = TemporaryDirectory::new(instance, "nix-source")?;
        let mut budget = Budget::default();
        for entry in fs::read_dir(instance).map_err(|e| input_error(instance, e))? {
            let entry = entry.map_err(|e| input_error(instance, e))?;
            let name = entry.file_name();
            let path = entry.path();
            // Top-level .nix supports legacy hand-written module imports. Only
            // the explicitly managed bundle directory is copied recursively.
            if path.extension().is_some_and(|ext| ext == "nix")
                || name == "flake.lock"
                || name == "guest-tmux.conf"
                || name == "guest-modules"
            {
                budget.copy(&path, &directory.0.join(name))?;
            }
        }
        if !directory.0.join("flake.nix").is_file() {
            return Err(state_error("VM instance has no regular flake.nix"));
        }
        if read_optional(&directory.0.join("flake.lock"))? != original_lock {
            return Err(state_error("flake.lock changed while preparing Nix inputs"));
        }
        Ok(Self {
            directory,
            instance: instance.to_path_buf(),
            original_lock,
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.directory.0
    }
    pub(super) fn publish_lock(&self) -> Result<(), VmError> {
        let Some(lock) = read_optional(&self.path().join("flake.lock"))? else {
            return Ok(());
        };
        let destination = self.instance.join("flake.lock");
        if read_optional(&destination)? != self.original_lock {
            return Err(state_error(
                "flake.lock changed concurrently; refusing to overwrite it",
            ));
        }
        if self.original_lock.as_ref() != Some(&lock) {
            let temporary = self.directory.0.join("lock-to-publish");
            fs::write(&temporary, lock).map_err(|e| input_error(&temporary, e))?;
            fs::rename(&temporary, &destination).map_err(|e| input_error(&destination, e))?;
        }
        Ok(())
    }
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, VmError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_BYTES =>
        {
            return Err(state_error(format!(
                "Nix lock must be a regular file of at most 64 MiB: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(input_error(path, error)),
        _ => {}
    }
    match fs::read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(input_error(path, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> TemporaryDirectory {
        TemporaryDirectory::new(&std::env::temp_dir(), "source-test").unwrap()
    }
    #[test]
    fn excludes_disks_keys_git_outputs_and_runtime_sockets() {
        let root = temp();
        fs::write(root.0.join("flake.nix"), "{}").unwrap();
        fs::write(root.0.join("microvm.nix"), "{}").unwrap();
        fs::write(root.0.join("var.img"), b"disk").unwrap();
        fs::write(root.0.join("known_hosts"), b"key").unwrap();
        fs::create_dir(root.0.join(".git")).unwrap();
        #[cfg(unix)]
        let _socket =
            std::os::unix::net::UnixListener::bind(root.0.join("control.socket")).unwrap();
        let source = BuildSource::new(&root.0).unwrap();
        let mut names: Vec<_> = fs::read_dir(source.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["flake.nix", "microvm.nix"]);
        let path = source.path().to_path_buf();
        drop(source);
        assert!(!path.exists());
        assert!(root.0.join("var.img").exists());
    }
    #[test]
    fn snapshots_bundle_and_keeps_it_when_source_changes() {
        let root = temp();
        let bundle = root.0.join("bundle");
        fs::create_dir(&bundle).unwrap();
        fs::write(bundle.join("default.nix"), "{ imports = [ ./extra.nix ]; }").unwrap();
        fs::write(bundle.join("extra.nix"), "{}").unwrap();
        let paths = resolve_modules(&root.0.join("profile.toml"), &["bundle".into()]).unwrap();
        snapshot_modules(&root.0, &paths).unwrap();
        fs::write(bundle.join("extra.nix"), "changed").unwrap();
        assert_eq!(
            fs::read_to_string(root.0.join("guest-modules/0/extra.nix")).unwrap(),
            "{}"
        );
        snapshot_modules(&root.0, &[]).unwrap();
        assert!(
            fs::read_dir(root.0.join("guest-modules"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    fn failed_module_copy_preserves_previous_snapshot() {
        let root = temp();
        fs::write(root.0.join("a.nix"), "{}").unwrap();
        snapshot_modules(&root.0, &[root.0.join("a.nix").display().to_string()]).unwrap();
        assert!(
            snapshot_modules(&root.0, &[root.0.join("missing.nix").display().to_string()]).is_err()
        );
        assert!(root.0.join("guest-modules/0/default.nix").exists());
    }
    #[test]
    fn generated_lock_is_persisted_but_concurrent_lock_not_overwritten() {
        let root = temp();
        fs::write(root.0.join("flake.nix"), "{}").unwrap();
        let source = BuildSource::new(&root.0).unwrap();
        fs::write(source.path().join("flake.lock"), "locked").unwrap();
        source.publish_lock().unwrap();
        assert_eq!(
            fs::read_to_string(root.0.join("flake.lock")).unwrap(),
            "locked"
        );
        assert!(source.publish_lock().is_err());
    }
    #[test]
    #[cfg(unix)]
    fn rejects_symlinked_inputs_and_bundle_members() {
        let root = temp();
        fs::write(root.0.join("real.nix"), "{}").unwrap();
        std::os::unix::fs::symlink(root.0.join("real.nix"), root.0.join("link.nix")).unwrap();
        assert!(resolve_modules(&root.0.join("profile.toml"), &["link.nix".into()]).is_err());
        assert!(BuildSource::new(&root.0).is_err());
    }
}
