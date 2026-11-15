//! The local-folder provider: an externally synced directory (Syncthing, Dropbox, a mounted NAS)
//! used as the shared store, and the backend of the engine tests.
//!
//! Safety rules (`SYNC_PROTOCOL.md` section 8): names containing separators of the host, `..`,
//! empty or control characters are rejected; symlinks are never followed (reads refuse them,
//! listings omit them, directories that are symlinks are not entered); writes go to a `.tmp-`
//! file that is hard-linked or renamed to the final name, so a peer never sees a partial file
//! under a final name written by us; only strictly named protocol files may be created.

use crate::names::{Class, classify};
use crate::provider::{Capabilities, Provider, ProviderError, RemoteEntry};
use nuvoletta_storage::fsutil::{read_regular, write_new_atomic};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Largest file the provider will read (a note is at most 16 MiB of body plus overhead).
pub const MAX_REMOTE_FILE: u64 = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 6;

/// A folder used as the sync root.
#[derive(Clone, Debug)]
pub struct FolderProvider {
    root: PathBuf,
}

fn map_io(e: &std::io::Error, what: &str) -> ProviderError {
    match e.kind() {
        ErrorKind::NotFound => ProviderError::NotFound(what.to_owned()),
        ErrorKind::AlreadyExists => ProviderError::AlreadyExists(what.to_owned()),
        ErrorKind::Interrupted | ErrorKind::TimedOut | ErrorKind::WouldBlock => {
            ProviderError::Transient(format!("{what}: {e}"))
        }
        _ => ProviderError::Other(format!("{what}: {e}")),
    }
}

fn map_storage(e: nuvoletta_storage::Error, what: &str) -> ProviderError {
    match e {
        nuvoletta_storage::Error::Io(io) => map_io(&io, what),
        other => ProviderError::Other(format!("{what}: {other}")),
    }
}

/// Split and validate a relative name; every component must be a plain file-system name.
fn components(name: &str) -> Result<Vec<&str>, ProviderError> {
    let bad = || ProviderError::Other(format!("invalid name: {name:?}"));
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&str> = name.split('/').collect();
    for p in &parts {
        if p.is_empty()
            || *p == "."
            || *p == ".."
            || p.contains(['\\', '\0', ':'])
            || p.chars().any(char::is_control)
        {
            return Err(bad());
        }
    }
    Ok(parts)
}

impl FolderProvider {
    /// Use `root` as the sync root, creating it if missing.
    ///
    /// # Errors
    /// Fails if the directory cannot be created or canonicalised.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ProviderError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|e| map_io(&e, "root"))?;
        let root = fs::canonicalize(root).map_err(|e| map_io(&e, "root"))?;
        Ok(Self { root })
    }

    /// The canonical root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Walk existing directory components, refusing symlinks. Returns `None` if a directory is
    /// missing and `create` is not set.
    fn dir_path(&self, dirs: &[&str], create: bool) -> Result<Option<PathBuf>, ProviderError> {
        let mut p = self.root.clone();
        for d in dirs {
            p.push(d);
            match fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(ProviderError::Other(format!("symlinked directory: {d}")));
                }
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(ProviderError::Other(format!("not a directory: {d}"))),
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    if !create {
                        return Ok(None);
                    }
                    fs::create_dir(&p)
                        .or_else(|e| {
                            if e.kind() == ErrorKind::AlreadyExists {
                                Ok(())
                            } else {
                                Err(e)
                            }
                        })
                        .map_err(|e| map_io(&e, d))?;
                }
                Err(e) => return Err(map_io(&e, d)),
            }
        }
        Ok(Some(p))
    }

    fn walk(
        dir: &Path,
        rel: &str,
        depth: usize,
        out: &mut Vec<RemoteEntry>,
    ) -> Result<(), ProviderError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let rd = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(map_io(&e, rel)),
        };
        for entry in rd {
            let entry = entry.map_err(|e| map_io(&e, rel))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue; // non-UTF-8 names can never be protocol files
            };
            let Ok(ft) = entry.file_type() else { continue };
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if ft.is_dir() {
                Self::walk(&entry.path(), &child, depth + 1, out)?;
            } else if ft.is_file() {
                let Ok(meta) = entry.metadata() else { continue };
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| i64::try_from(d.as_millis()).ok());
                out.push(RemoteEntry {
                    name: child,
                    size: meta.len(),
                    etag: None,
                    mtime_ms,
                });
            }
        }
        Ok(())
    }
}

impl Provider for FolderProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            create_only: cfg!(unix),
            atomic_rename: true,
            etags: false,
            mtimes: true,
            can_delete: true,
            authoritative: false,
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError> {
        let parts = components(prefix)?;
        let Some(dir) = self.dir_path(&parts, false)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        Self::walk(&dir, prefix, 0, &mut out)?;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError> {
        let parts = components(name)?;
        let Some((file, dirs)) = parts.split_last() else {
            return Err(ProviderError::Other("empty name".into()));
        };
        let Some(dir) = self.dir_path(dirs, false)? else {
            return Err(ProviderError::NotFound(name.to_owned()));
        };
        read_regular(&dir.join(file), MAX_REMOTE_FILE).map_err(|e| map_storage(e, name))
    }

    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        if !matches!(classify(name), Class::Protocol(_)) {
            return Err(ProviderError::Other(format!(
                "refusing to create non-protocol name: {name:?}"
            )));
        }
        let parts = components(name)?;
        let Some((file, dirs)) = parts.split_last() else {
            return Err(ProviderError::Other("empty name".into()));
        };
        let dir = self
            .dir_path(dirs, true)?
            .ok_or_else(|| ProviderError::Other("directory vanished".into()))?;
        match write_new_atomic(&dir, file, bytes).map_err(|e| map_storage(e, name))? {
            true => Ok(()),
            false => Err(ProviderError::AlreadyExists(name.to_owned())),
        }
    }

    fn delete(&self, name: &str) -> Result<(), ProviderError> {
        let parts = components(name)?;
        let Some((file, dirs)) = parts.split_last() else {
            return Err(ProviderError::Other("empty name".into()));
        };
        let Some(dir) = self.dir_path(dirs, false)? else {
            return Err(ProviderError::NotFound(name.to_owned()));
        };
        let p = dir.join(file);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() => fs::remove_file(&p).map_err(|e| map_io(&e, name)),
            Ok(_) => Err(ProviderError::Other(format!("not a regular file: {name}"))),
            Err(e) => Err(map_io(&e, name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(seq: u64) -> String {
        format!("d/{}/l/{seq:016x}.nvlog", "0a".repeat(16))
    }

    #[test]
    fn put_get_list_delete_and_create_only() {
        let t = tempfile::tempdir().unwrap();
        let p = FolderProvider::new(t.path()).unwrap();
        p.put_new(&seg(1), b"one").unwrap();
        assert!(matches!(
            p.put_new(&seg(1), b"two"),
            Err(ProviderError::AlreadyExists(_))
        ));
        assert_eq!(p.get(&seg(1)).unwrap(), b"one");
        let l = p.list("d").unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, seg(1));
        assert_eq!(l[0].size, 3);
        assert_eq!(p.list("").unwrap().len(), 1);
        assert!(p.list("o").unwrap().is_empty());
        p.delete(&seg(1)).unwrap();
        assert!(matches!(p.get(&seg(1)), Err(ProviderError::NotFound(_))));
        // No temporary file is left behind.
        assert!(p.list("").unwrap().is_empty());
    }

    #[test]
    fn hostile_names_are_rejected() {
        let t = tempfile::tempdir().unwrap();
        let p = FolderProvider::new(t.path()).unwrap();
        for bad in [
            "../x",
            "a/../b",
            "a//b",
            "a\\b",
            "/etc/passwd",
            "a/./b",
            "x\0y",
        ] {
            assert!(p.get(bad).is_err(), "{bad}");
            assert!(p.put_new(bad, b"x").is_err(), "{bad}");
            assert!(p.delete(bad).is_err(), "{bad}");
        }
        // Only protocol names may be created.
        assert!(p.put_new("h/readme.txt", b"x").is_err());
        assert!(
            p.put_new(
                &format!("{} (1).nvlog", seg(1).trim_end_matches(".nvlog")),
                b"x"
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), b"secret").unwrap();
        let t = tempfile::tempdir().unwrap();
        let p = FolderProvider::new(t.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), t.path().join("d")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), t.path().join("h-link")).unwrap();
        // A symlinked directory is refused, a symlinked file is neither read nor listed.
        assert!(p.list("d").is_err());
        assert!(p.put_new(&seg(1), b"x").is_err());
        assert!(p.get("h-link").is_err());
        let names: Vec<_> = p.list("").unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.is_empty(), "{names:?}");
        assert!(!outside.path().join("a").exists());
    }

    #[test]
    fn leftover_temp_files_are_listed_so_the_engine_can_see_them() {
        let t = tempfile::tempdir().unwrap();
        let p = FolderProvider::new(t.path()).unwrap();
        p.put_new(&seg(1), b"x").unwrap();
        let dir = t.path().join("d").join("0a".repeat(16)).join("l");
        fs::write(dir.join(".tmp-deadbeef"), b"half").unwrap();
        assert_eq!(p.list("d").unwrap().len(), 2);
    }
}
