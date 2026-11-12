//! Atomic writes and safe file access.

use crate::error::{Error, Result};
use crate::layout::TMP_PREFIX;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

/// Write `bytes` to `dir/name` atomically: temporary file in the same directory, fsync, rename
/// over the destination, fsync of the directory. On failure the temporary file is removed and the
/// destination is untouched.
pub(crate) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
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

/// fsync a directory so a rename or unlink inside it is durable.
pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
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

/// Read a regular file, refusing symlinks and anything larger than `max` bytes.
pub(crate) fn read_regular(path: &Path, max: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(Error::UnsafeEntry("symlink"));
    }
    if !meta.is_file() {
        return Err(Error::UnsafeEntry("not a regular file"));
    }
    if meta.len() > max {
        return Err(Error::UnsafeEntry("file too large"));
    }
    let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Require that `path` is a real directory (not a symlink); create it if `create` is set.
pub(crate) fn ensure_real_dir(path: &Path, create: bool) -> Result<()> {
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
pub(crate) fn clean_temp_files(dir: &Path) -> Result<()> {
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
