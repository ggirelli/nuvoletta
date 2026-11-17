//! Taking a vault header found elsewhere: joining a device to an existing vault and adopting a
//! newer header after a passphrase or recovery-key change on another device
//! (`docs/ADR/0008-device-join-and-header-adoption.md`).
//!
//! No new cryptography: a candidate header is opened with the existing unlock functions (so the
//! KDF parameters are range-checked, the wrap authenticates its AAD and `header_mac` verifies
//! under the key derived from the unwrapped DEK) and is then bound to the vault this device
//! already holds by comparing derived keys. Nothing is written until every check passed, and the
//! header file is replaced atomically.
//!
//! * **Join** ([`Locked::join`]): there is no local vault yet, so a user secret opening the header
//!   is the only authentication. The caller shows the fingerprint and generation first.
//! * **Adoption on an unlocked device** ([`Unlocked::verify_header`]): the candidate must unwrap to
//!   the DEK the session already holds.
//! * **Adoption on a locked device** ([`Locked::verify_header`]): the DEK is not in memory, so the
//!   keys the candidate unlocks are checked by opening existing sealed local data (a note, or a
//!   state file sealed with `K_local`). With nothing to open the header is refused.

use crate::error::{Error, Result};
use crate::fsutil::{ensure_real_dir, read_regular, write_atomic};
use crate::layout::{HEADER_FILE, header_path, meta_dir, notes_dir};
use crate::lock::VaultLock;
use crate::locked::Locked;
use crate::unlocked::Unlocked;
use nuvoletta_crypto::header::HEADER_LEN;
use nuvoletta_crypto::{KdfParams, RecoveryKey, Subkeys, Vault, VaultHeader, open};
use std::fs;
use std::path::{Path, PathBuf};

/// Largest sealed local file looked at when checking a key (state files and notes are bounded).
const MAX_SEALED_FILE: u64 = 32 * 1024 * 1024;
/// How many local files are tried at most when checking a key.
const MAX_FILES_TRIED: usize = 64;
/// Directories of `.nuvoletta` holding files sealed with `K_local`.
const SEALED_DIRS: [&str; 3] = ["sync-state", "device-state", "trash"];

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

/// Why a candidate header was refused (after or before it was opened).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// It belongs to another vault.
    OtherVault,
    /// Its generation is lower than the header this device holds (a replay or a rollback).
    Older {
        /// The generation this device holds.
        local: u32,
        /// The generation of the candidate.
        candidate: u32,
    },
    /// It is the header this device already holds.
    AlreadyCurrent,
    /// It opened, but its keys are not the keys of this device's notes and state.
    DifferentKeys,
    /// A locked device has no sealed local data to check the keys against.
    NothingToCheck,
}

/// A candidate header that was opened with the user's secret and bound to this vault. Holds keys:
/// dropping it zeroizes them.
#[derive(Debug)]
pub struct VerifiedHeader {
    vault: Vault,
}

impl VerifiedHeader {
    /// The header that verified.
    #[must_use]
    pub fn header(&self) -> &VaultHeader {
        self.vault.header()
    }

    /// Its generation.
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.vault.header().generation()
    }

    /// Its KDF parameters (authenticated: the wrap covers them).
    #[must_use]
    pub fn kdf_params(&self) -> KdfParams {
        self.vault.header().kdf_params()
    }

    /// The keys it unlocked (the same as the vault's own, by construction).
    #[must_use]
    pub fn keys(&self) -> &Subkeys {
        self.vault.keys()
    }
}

/// Checks that need no secret and no Argon2, so a bad candidate is refused cheaply.
fn precheck(local: &VaultHeader, candidate: &VaultHeader) -> Result<()> {
    if candidate.vault_id() != local.vault_id() {
        return Err(Error::HeaderRefused(Refusal::OtherVault));
    }
    let (l, c) = (local.generation(), candidate.generation());
    if c < l {
        return Err(Error::HeaderRefused(Refusal::Older {
            local: l,
            candidate: c,
        }));
    }
    // The same generation is only acceptable as a *different* header: two devices changed the
    // passphrase at once and this one lost the race.
    if c == l && candidate.as_bytes() == local.as_bytes() {
        return Err(Error::HeaderRefused(Refusal::AlreadyCurrent));
    }
    Ok(())
}

impl Unlocked {
    /// Open `candidate` with `secret` and check it against this session: same vault, generation
    /// not lower, and the unwrapped DEK equals the one held here.
    ///
    /// # Errors
    /// [`Error::HeaderRefused`], or the generic unlock error for a wrong secret.
    pub fn verify_header(
        &self,
        candidate: &VaultHeader,
        secret: &Secret<'_>,
    ) -> Result<VerifiedHeader> {
        precheck(self.header(), candidate)?;
        let vault = secret.open(candidate)?;
        if !vault.keys().same_keys(self.keys()) {
            return Err(Error::HeaderRefused(Refusal::DifferentKeys));
        }
        Ok(VerifiedHeader { vault })
    }

    /// Replace this vault's header with a verified one. The session keeps its keys (the DEK did
    /// not change), so no other file is touched.
    ///
    /// # Errors
    /// [`Error::HeaderRefused`] if it is no longer acceptable, [`Error::HeaderChanged`] if the
    /// header on disk is not the one of this session, I/O errors.
    pub fn adopt_header(&mut self, verified: VerifiedHeader) -> Result<()> {
        precheck(self.header(), verified.header())?;
        if !verified.keys().same_keys(self.keys()) {
            return Err(Error::HeaderRefused(Refusal::DifferentKeys));
        }
        self.replace_header(verified.vault)
    }
}

impl Locked {
    /// Open `candidate` with `secret` on a locked device and check it: same vault, generation not
    /// lower, and the keys it yields open at least one sealed file of this vault. Files are
    /// tried in a fixed order (notes first); one that authenticates is enough, because every key
    /// is derived from the DEK.
    ///
    /// # Errors
    /// [`Error::HeaderRefused`] (including [`Refusal::NothingToCheck`]), or the generic unlock
    /// error for a wrong secret.
    pub fn verify_header(
        &self,
        candidate: &VaultHeader,
        secret: &Secret<'_>,
    ) -> Result<VerifiedHeader> {
        precheck(self.header(), candidate)?;
        let vault = secret.open(candidate)?;
        match opens_local_data(self.root(), vault.keys())? {
            Some(true) => Ok(VerifiedHeader { vault }),
            Some(false) => Err(Error::HeaderRefused(Refusal::DifferentKeys)),
            None => Err(Error::HeaderRefused(Refusal::NothingToCheck)),
        }
    }

    /// Replace this vault's header with a verified one.
    ///
    /// # Errors
    /// [`Error::HeaderRefused`], [`Error::HeaderChanged`], I/O errors.
    pub fn adopt_header(&mut self, verified: VerifiedHeader) -> Result<()> {
        precheck(self.header(), verified.header())?;
        let on_disk = read_regular(&header_path(self.root()), HEADER_LEN as u64)?;
        if on_disk != self.header().as_bytes() {
            return Err(Error::HeaderChanged);
        }
        write_atomic(
            &meta_dir(self.root()),
            HEADER_FILE,
            verified.header().as_bytes(),
        )?;
        self.set_header(verified.header().clone());
        Ok(())
    }

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

/// Whether the keys open any sealed local file: `Some(true)` if one authenticates, `Some(false)`
/// if there were files and none did, `None` if there was nothing to try.
fn opens_local_data(root: &Path, keys: &Subkeys) -> Result<Option<bool>> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    collect_files(&notes_dir(root), &mut candidates)?;
    for d in SEALED_DIRS {
        collect_files(&meta_dir(root).join(d), &mut candidates)?;
    }
    let mut tried = false;
    for path in candidates.into_iter().take(MAX_FILES_TRIED) {
        let Ok(bytes) = read_regular(&path, MAX_SEALED_FILE) else {
            continue;
        };
        tried = true;
        if open(keys, &bytes).is_ok() {
            return Ok(Some(true));
        }
    }
    Ok(tried.then_some(false))
}

/// Regular files directly inside `dir` (never symlinks), in name order; missing is empty.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut files = Vec::new();
    for e in entries {
        let e = e?;
        if e.file_type()?.is_file() {
            files.push(e.path());
        }
    }
    files.sort();
    out.extend(files);
    Ok(())
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
