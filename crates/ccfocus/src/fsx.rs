//! Private, symlink-refusing filesystem writes for the state directory.
//!
//! Directories are created `0700` and files `0600` (ours are tightened when
//! found wider). Only the last path component is checked: a symlink where the
//! state root, `sessions/`, `events/`, or an event log should be is an error,
//! never followed, while ancestors above the state root (e.g. a symlinked
//! `~/.claude`) are resolved normally. Session files are replaced atomically
//! (temp file + rename), which replaces a symlink at the target instead of
//! writing through it, so a reader never sees a half-written file. The
//! directory checks are check-then-act; the state dir is `0700`, so only the
//! same user could race them.

use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

#[cfg_attr(not(unix), allow(dead_code))]
const DIR_MODE: u32 = 0o700;
#[cfg_attr(not(unix), allow(dead_code))]
const FILE_MODE: u32 = 0o600;

/// How [`ensure_private_dir`] treats a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Create {
    /// The state root: create it and missing ancestors `0700`, but leave an
    /// existing one's mode alone — the user may have pointed us at it.
    WithParents,
    /// A directory only ccfocus uses: its parent must exist, and an existing
    /// one is tightened to `0700`.
    Owned,
}

/// Ensure `path` is a real (non-symlink) directory, creating it `0700` if
/// absent (see [`Create`] for ancestors and existing modes).
pub fn ensure_private_dir(path: &Path, how: Create) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(how == Create::WithParents);
            #[cfg(unix)]
            builder.mode(DIR_MODE);
            match builder.create(path) {
                Ok(()) => {}
                // A concurrent hook created it first; checked below.
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("create {}", path.display()));
                }
            }
            fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?
        }
        Err(e) => return Err(e).with_context(|| format!("stat {}", path.display())),
    };
    if !meta.file_type().is_dir() {
        bail!(
            "{} is not a directory (symlinks are not followed)",
            path.display()
        );
    }
    if how == Create::Owned {
        tighten_dir(path, &meta)?;
    }
    Ok(())
}

/// True when `path` exists as a real directory; false when missing. Errors
/// when something else (a symlink, a file) sits there.
pub fn is_real_dir(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(true),
        Ok(_) => bail!(
            "{} is not a directory (symlinks are not followed)",
            path.display()
        ),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("stat {}", path.display())),
    }
}

/// Atomically replace `dir/name` with `bytes`: write a `0600` temp file in
/// `dir` (created exclusively, so it never follows a planted link), then
/// rename it over the target. Renaming replaces a symlink at the target
/// rather than writing through it.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let target = dir.join(name);

    let result = (|| -> Result<()> {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(FILE_MODE);
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        drop(f);
        fs::rename(&tmp, &target).with_context(|| format!("rename to {}", target.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Append `line` to `path` in one `O_APPEND` write, creating the file `0600`.
/// Refuses when `path` is a symlink or other non-regular file — also when one
/// is swapped in between the check and the open: a new file is created
/// exclusively, and an existing one is re-checked against the opened handle
/// before anything is written.
pub fn append_line(path: &Path, line: &str) -> Result<()> {
    let existed = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => true,
        Ok(_) => bail!(
            "{} is not a regular file (symlinks are not followed)",
            path.display()
        ),
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => return Err(e).with_context(|| format!("stat {}", path.display())),
    };
    let open_existing = || OpenOptions::new().append(true).open(path);
    let opened = if existed {
        open_existing()
    } else {
        let mut opts = OpenOptions::new();
        opts.append(true).create_new(true);
        #[cfg(unix)]
        opts.mode(FILE_MODE);
        match opts.open(path) {
            // Another hook created it between the check and the open.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => open_existing(),
            other => other,
        }
    };
    let mut f = opened.with_context(|| format!("open {}", path.display()))?;
    ensure_same_file(path, &f)?;
    tighten_file(path, &f)?;
    f.write_all(line.as_bytes())
        .with_context(|| format!("append {}", path.display()))
}

/// Error unless `path` is still the regular file `f` was opened from.
#[cfg(unix)]
fn ensure_same_file(path: &Path, f: &fs::File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let opened = f
        .metadata()
        .with_context(|| format!("stat {}", path.display()))?;
    let on_disk = fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !on_disk.file_type().is_file()
        || (opened.dev(), opened.ino()) != (on_disk.dev(), on_disk.ino())
    {
        bail!(
            "{} changed while opening (symlinks are not followed)",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn ensure_same_file(_path: &Path, _f: &fs::File) -> Result<()> {
    Ok(())
}

/// Clear group/other bits on an opened file (through the handle, so no path
/// is re-resolved).
#[cfg(unix)]
fn tighten_file(path: &Path, f: &fs::File) -> Result<()> {
    let mode = f
        .metadata()
        .with_context(|| format!("stat {}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        f.set_permissions(fs::Permissions::from_mode(FILE_MODE))
            .with_context(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn tighten_file(_path: &Path, _f: &fs::File) -> Result<()> {
    Ok(())
}

/// Remove `path` (a link itself, never its target). Missing is fine.
pub fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

/// Clear group/other bits on a directory we own (just checked to be a real
/// directory).
#[cfg(unix)]
fn tighten_dir(path: &Path, meta: &fs::Metadata) -> Result<()> {
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(DIR_MODE))
            .with_context(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn tighten_dir(_path: &Path, _meta: &fs::Metadata) -> Result<()> {
    Ok(())
}
