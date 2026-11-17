//! Atomic writes and safe file access (also used by `nuvoletta-sync` for its local state and the
//! synced-folder backend).

use crate::error::{Error, Result};
use crate::layout::TMP_PREFIX;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

/// Write `bytes` to `dir/name` atomically: temporary file in the same directory, fsync, rename
/// over the destination, fsync of the directory. On failure the temporary file is removed and the
/// destination is untouched.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut rnd = [0u8; 8];
    getrandom::fill(&mut rnd).map_err(|_| nuvoletta_crypto::Error::Rng)?;
    let tmp_name: String = rnd.iter().fold(TMP_PREFIX.to_owned(), |mut s, b| {
        s.push_str(&format!("{b:02x}"));
        s
    });
    let tmp = dir.join(&tmp_name);
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, dir.join(name))?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Like [`write_atomic`] but never replaces an existing destination: the temporary file is
/// hard-linked to the final name (atomic, fails if the name exists) and then unlinked. Falls
/// back to "check, then rename" where hard links are unsupported (a small race window, so the
/// caller must treat the result as best effort there). Returns `Ok(false)` if the name exists.
pub fn write_new_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<bool> {
    let mut rnd = [0u8; 8];
    getrandom::fill(&mut rnd).map_err(|_| nuvoletta_crypto::Error::Rng)?;
    let tmp_name: String = rnd.iter().fold(TMP_PREFIX.to_owned(), |mut s, b| {
        s.push_str(&format!("{b:02x}"));
        s
    });
    let tmp = dir.join(&tmp_name);
    let dest = dir.join(name);
    let result = (|| -> Result<bool> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        match fs::hard_link(&tmp, &dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(_) => {
                if fs::symlink_metadata(&dest).is_ok() {
                    return Ok(false);
                }
                fs::rename(&tmp, &dest)?;
            }
        }
        sync_dir(dir)?;
        Ok(true)
    })();
    let _ = fs::remove_file(&tmp);
    result
}

/// fsync a directory so a rename or unlink inside it is durable.
pub fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        // Directories cannot be opened for fsync on this platform.
        let _ = dir;
    }
    Ok(())
}

/// Open `path` for reading, refusing a symlink as the final component. On Unix the check is part
/// of the `open` call itself (`O_NOFOLLOW`), so a file swapped for a symlink between a check and
/// the open cannot be followed; `O_NONBLOCK` keeps a swapped-in FIFO from blocking the open.
/// Elsewhere the check is a separate step and the residual race is accepted (no such platform is
/// a supported target for a synced vault folder).
pub fn open_nofollow(path: &Path) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| {
                if e.raw_os_error() == Some(libc::ELOOP) {
                    Error::UnsafeEntry("symlink")
                } else {
                    Error::Io(e)
                }
            })
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(Error::UnsafeEntry("symlink"));
        }
        Ok(File::open(path)?)
    }
}

/// Read a regular file, refusing symlinks and anything larger than `max` bytes. The type and the
/// size are taken from the opened handle, not from the path, so they describe what is read.
pub fn read_regular(path: &Path, max: u64) -> Result<Vec<u8>> {
    let f = open_nofollow(path)?;
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(Error::UnsafeEntry("not a regular file"));
    }
    if meta.len() > max {
        return Err(Error::UnsafeEntry("file too large"));
    }
    let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    f.take(max.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(Error::UnsafeEntry("file too large"));
    }
    Ok(buf)
}

/// Require that `path` is a real directory (not a symlink); create it if `create` is set.
pub fn ensure_real_dir(path: &Path, create: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(Error::UnsafeEntry("symlinked directory")),
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(Error::UnsafeEntry("not a directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Remove leftover `.tmp-` files (interrupted writes) from `dir`. They only ever hold ciphertext.
pub fn clean_temp_files(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let is_tmp = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(TMP_PREFIX));
        if is_tmp && !entry.file_type()?.is_dir() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn symlinks_are_refused_at_open_time_even_when_valid_or_dangling() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::write(&real, b"x").unwrap();
        let link = tmp.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(matches!(
            read_regular(&link, 10),
            Err(Error::UnsafeEntry("symlink"))
        ));
        let dangling = tmp.path().join("dangling");
        symlink(tmp.path().join("missing"), &dangling).unwrap();
        assert!(matches!(
            read_regular(&dangling, 10),
            Err(Error::UnsafeEntry("symlink"))
        ));
        assert_eq!(read_regular(&real, 10).unwrap(), b"x");
    }

    #[test]
    fn a_fifo_is_refused_without_blocking_and_size_is_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            read_regular(&fifo, 10),
            Err(Error::UnsafeEntry("not a regular file"))
        ));
        let big = tmp.path().join("big");
        fs::write(&big, [0u8; 11]).unwrap();
        assert!(matches!(
            read_regular(&big, 10),
            Err(Error::UnsafeEntry("file too large"))
        ));
    }
}
