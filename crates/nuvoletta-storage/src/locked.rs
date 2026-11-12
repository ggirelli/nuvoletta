//! A vault directory that is not unlocked: it holds no keys.

use crate::error::{Error, Result};
use crate::fsutil::{clean_temp_files, ensure_real_dir, read_regular, write_atomic};
use crate::layout::{HEADER_FILE, header_path, meta_dir, notes_dir};
use crate::unlocked::Unlocked;
use nuvoletta_crypto::header::HEADER_LEN;
use nuvoletta_crypto::{KdfParams, RecoveryKey, Vault, VaultHeader};
use std::fs;
use std::path::{Path, PathBuf};

/// A vault on disk whose header has been read and validated but whose keys are not loaded.
#[derive(Debug, Clone)]
pub struct Locked {
    root: PathBuf,
    header: VaultHeader,
}

impl Locked {
    /// Create a new vault in `root` (created if missing). Returns the unlocked vault and the
    /// recovery key, which the caller must show to the user exactly once.
    ///
    /// # Errors
    /// Fails if a vault already exists there, the passphrase is too short, the KDF parameters
    /// are out of range or writing fails.
    pub fn create(
        root: impl AsRef<Path>,
        passphrase: &str,
        params: KdfParams,
    ) -> Result<(Unlocked, RecoveryKey)> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let meta = meta_dir(root);
        ensure_real_dir(&meta, true)?;
        if fs::symlink_metadata(header_path(root)).is_ok() {
            return Err(Error::VaultExists);
        }
        let created = Vault::create(passphrase, params)?;
        ensure_real_dir(&notes_dir(root), true)?;
        write_atomic(&meta, HEADER_FILE, created.vault.header().as_bytes())?;
        let unlocked = Unlocked::new(root.to_path_buf(), created.vault)?;
        Ok((unlocked, created.recovery_key))
    }

    /// Open an existing vault directory: validate the header and remove leftover temporary
    /// files from interrupted writes. No key is derived.
    ///
    /// # Errors
    /// Fails if there is no vault, the header is malformed or unsafe, or cleanup fails.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let meta = meta_dir(&root);
        match ensure_real_dir(&meta, false) {
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotAVault);
            }
            other => other?,
        }
        let bytes = match read_regular(&header_path(&root), HEADER_LEN as u64) {
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotAVault);
            }
            other => other?,
        };
        let header = VaultHeader::parse(&bytes)?;
        clean_temp_files(&meta)?;
        let notes = notes_dir(&root);
        match ensure_real_dir(&notes, false) {
            Ok(()) => clean_temp_files(&notes)?,
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                ensure_real_dir(&notes, true)?;
            }
            Err(e) => return Err(e),
        }
        Ok(Self { root, header })
    }

    pub(crate) fn from_parts(root: PathBuf, header: VaultHeader) -> Self {
        Self { root, header }
    }

    /// The vault directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The validated vault header.
    #[must_use]
    pub fn header(&self) -> &VaultHeader {
        &self.header
    }

    /// Unlock with the passphrase and build the in-memory index.
    ///
    /// # Errors
    /// [`nuvoletta_crypto::Error::Unlock`] (wrapped) for a wrong passphrase or corrupted header.
    pub fn unlock_with_passphrase(&self, passphrase: &str) -> Result<Unlocked> {
        let vault = self.header.unlock_with_passphrase(passphrase)?;
        Unlocked::new(self.root.clone(), vault)
    }

    /// Unlock with the recovery key. The caller should then force a new passphrase with
    /// [`Unlocked::change_passphrase`] and offer [`Unlocked::rotate_recovery_key`].
    ///
    /// # Errors
    /// As [`Locked::unlock_with_passphrase`].
    pub fn unlock_with_recovery_key(&self, key: &RecoveryKey) -> Result<Unlocked> {
        let vault = self.header.unlock_with_recovery_key(key)?;
        Unlocked::new(self.root.clone(), vault)
    }
}
