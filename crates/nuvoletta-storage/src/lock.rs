//! Single-instance lock of a vault directory (`docs/STORAGE_FORMAT.md` section 5).
//!
//! Two processes on one vault each keep their own in-memory index and sync state, and one
//! autosave can replace a revision file the other never recorded. An advisory exclusive lock on
//! `.nuvoletta/lock`, held for as long as a [`crate::Locked`] or [`crate::Unlocked`] of the vault
//! exists (they share it), makes the second opener fail with [`Error::VaultInUse`].
//!
//! The lock is the operating system's (`flock` / `LockFileEx` through `std::fs::File::try_lock`),
//! not the existence of the file, so a crash cannot leave a stale lock: the kernel drops it when
//! the process dies, and a leftover `lock` file is harmless. File systems without lock support
//! (some network mounts) report `Unsupported`; the vault then opens without the protection
//! rather than not at all. It is advisory: it stops a second Nuvoletta, not a hostile process.

use crate::error::{Error, Result};
use crate::layout::{LOCK_FILE, meta_dir};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::Arc;

/// A held vault lock; clones share it, the last drop releases it.
#[derive(Debug, Clone)]
pub struct VaultLock {
    _file: Option<Arc<File>>,
}

impl VaultLock {
    /// Take the exclusive lock of the vault at `root` (its `.nuvoletta` directory must exist).
    ///
    /// # Errors
    /// [`Error::VaultInUse`] if another handle holds it; I/O errors.
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = meta_dir(root).join(LOCK_FILE);
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = opts.open(&path).map_err(|e| {
            #[cfg(unix)]
            if e.raw_os_error() == Some(libc::ELOOP) {
                return Error::UnsafeEntry("symlink");
            }
            Error::Io(e)
        })?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: Some(Arc::new(file)),
            }),
            Err(TryLockError::WouldBlock) => Err(Error::VaultInUse),
            Err(TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Unsupported => {
                Ok(Self { _file: None })
            }
            Err(TryLockError::Error(e)) => Err(Error::Io(e)),
        }
    }
}
