//! Joining a device to an existing vault from a header found on a provider
//! (`docs/ADR/0008-device-join-and-header-adoption.md`).
//!
//! No new cryptography: the header is opened with the existing unlock functions (so the KDF
//! parameters are range-checked, the wrap authenticates its AAD and `header_mac` verifies under
//! the key derived from the unwrapped DEK). A joining device has no local vault to compare with,
//! so the user's secret opening the header is the only authentication; the caller shows the vault
//! fingerprint and generation before anything is written. Nothing touches the disk until
//! [`Locked::join`], which writes the exact bytes that verified.
//!
//! A join that fails or is cancelled is undone with [`remove_unused_vault`], which refuses to
//! remove anything that holds notes.

use crate::error::{Error, Result};
use crate::fsutil::{ensure_real_dir, write_atomic};
use crate::layout::{HEADER_FILE, header_path, meta_dir, notes_dir};
use crate::lock::VaultLock;
use crate::locked::Locked;
use crate::unlocked::Unlocked;
use nuvoletta_crypto::{RecoveryKey, Vault, VaultHeader};
use std::fs;
use std::path::Path;

/// The secret the user typed to open a header.
pub enum Secret<'a> {
    /// The passphrase of that header.
    Passphrase(&'a str),
    /// The recovery key.
    RecoveryKey(&'a RecoveryKey),
}

impl Secret<'_> {
    /// Open `header` with this secret: KDF range check, unwrap, `header_mac`.
    ///
    /// # Errors
    /// [`nuvoletta_crypto::Error::Unlock`] (wrapped) for a wrong secret or a tampered header.
    pub fn open(&self, header: &VaultHeader) -> Result<Vault> {
        Ok(match self {
            Self::Passphrase(p) => header.unlock_with_passphrase(p)?,
            Self::RecoveryKey(k) => header.unlock_with_recovery_key(k)?,
        })
    }
}

impl Locked {
    /// Create the vault directory of a joining device from a header that was opened with the
    /// user's secret ([`Secret::open`]), and unlock it. The header bytes written are exactly the
    /// ones that verified; the notes directory is created empty.
    ///
    /// # Errors
    /// [`Error::VaultExists`] if the directory already has a vault; I/O errors.
    pub fn join(root: impl AsRef<Path>, vault: Vault) -> Result<Unlocked> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let meta = meta_dir(root);
        ensure_real_dir(&meta, true)?;
        let lock = VaultLock::acquire(root)?;
        if fs::symlink_metadata(header_path(root)).is_ok() {
            return Err(Error::VaultExists);
        }
        ensure_real_dir(&notes_dir(root), true)?;
        write_atomic(&meta, HEADER_FILE, vault.header().as_bytes())?;
        Unlocked::new(root.to_path_buf(), vault, lock)
    }
}

/// What [`remove_unused_vault`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    /// The vault's own files were removed (and the directory itself, if nothing else was in it).
    Removed,
    /// The vault holds notes (or something unexpected), so nothing was removed.
    Kept,
}

/// Remove the vault directory of a join that failed or was cancelled, but only if it holds no
/// notes: no note file, nothing in the local trash and no unexpected entry. Never follows a
/// symlink. Only the vault's own `.nuvoletta` and `notes` directories are removed; the directory
/// itself goes only if that leaves it empty.
///
/// Nothing may hold the vault open (drop the session first).
///
/// # Errors
/// I/O errors while removing; the vault is then left partly removed but never with notes lost.
pub fn remove_unused_vault(root: &Path) -> Result<Cleanup> {
    let meta = meta_dir(root);
    let notes = notes_dir(root);
    let is_real_dir = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    if !is_real_dir(&meta) || fs::symlink_metadata(header_path(root)).is_err() {
        return Ok(Cleanup::Kept); // not a vault: not ours to remove
    }
    if fs::symlink_metadata(&notes).is_ok()
        && (!is_real_dir(&notes) || fs::read_dir(&notes)?.next().is_some())
    {
        return Ok(Cleanup::Kept);
    }
    let trash = meta.join("trash");
    if fs::symlink_metadata(&trash).is_ok()
        && (!is_real_dir(&trash)
            || fs::read_dir(&trash)?.any(|e| e.is_ok_and(|e| e.file_name() != "index")))
    {
        return Ok(Cleanup::Kept);
    }
    fs::remove_dir_all(&meta)?;
    if fs::symlink_metadata(&notes).is_ok() {
        fs::remove_dir(&notes)?;
    }
    // Fails (and is ignored) when the user's directory holds other files.
    let _ = fs::remove_dir(root);
    Ok(Cleanup::Removed)
}
