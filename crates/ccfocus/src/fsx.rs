//! Private, symlink-refusing filesystem writes for the state directory.
//!
//! Directories are created `0700` and files `0600` (tightened when found
//! wider). A symlink where a state directory or file should be is an error,
//! never followed. Session files are replaced atomically (temp file + rename),
//! so a reader never sees a half-written file.

use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// Ensure `path` is a real (non-symlink) directory with mode `0700`, creating
/// it — and, when `parents` is set, its missing ancestors — if absent.
pub fn ensure_private_dir(path: &Path, parents: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_dir() {
                bail!(
                    "{} is not a directory (symlinks are not followed)",
                    path.display()
                );
            }
            tighten(path, &meta, DIR_MODE)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(parents);
            #[cfg(unix)]
            builder.mode(DIR_MODE);
            match builder.create(path) {
                Ok(()) => {}
                // A concurrent hook created it first; re-check below.
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("create {}", path.display()));
                }
            }
            let meta =
                fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
            if !meta.file_type().is_dir() {
                bail!("{} is not a directory", path.display());
            }
            tighten(path, &meta, DIR_MODE)
        }
        Err(e) => Err(e).with_context(|| format!("stat {}", path.display())),
    }
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
/// Refuses when `path` is a symlink or other non-regular file.
pub fn append_line(path: &Path, line: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                bail!(
                    "{} is not a regular file (symlinks are not followed)",
                    path.display()
                );
            }
            tighten(path, &meta, FILE_MODE)?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("stat {}", path.display())),
    }
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    opts.mode(FILE_MODE);
    let mut f = opts
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    f.write_all(line.as_bytes())
        .with_context(|| format!("append {}", path.display()))
}

/// Remove `path` (a link itself, never its target). Missing is fine.
pub fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

/// Clear group/other bits on an existing entry we own.
#[cfg(unix)]
fn tighten(path: &Path, meta: &fs::Metadata, mode: u32) -> Result<()> {
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn tighten(_path: &Path, _meta: &fs::Metadata, _mode: u32) -> Result<()> {
    Ok(())
}
