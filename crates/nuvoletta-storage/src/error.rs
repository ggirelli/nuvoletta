//! Errors of the storage layer. Messages never contain note content or secrets.

use nuvoletta_core::NoteId;

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An I/O operation failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// A cryptographic operation failed (wrong passphrase, tampering, malformed data, ...).
    #[error(transparent)]
    Crypto(#[from] nuvoletta_crypto::Error),
    /// A domain rule was violated.
    #[error(transparent)]
    Core(#[from] nuvoletta_core::Error),
    /// The directory already contains a vault.
    #[error("a vault already exists in this directory")]
    VaultExists,
    /// The directory does not contain a vault.
    #[error("no vault found in this directory")]
    NotAVault,
    /// A vault entry is a symlink, not a regular file or directory, or otherwise unsafe.
    #[error("unsafe vault entry: {0}")]
    UnsafeEntry(&'static str),
    /// A note with this id already exists.
    #[error("note {0} already exists")]
    AlreadyExists(NoteId),
    /// No note with this id exists.
    #[error("note {0} not found")]
    NotFound(NoteId),
    /// The export destination exists and is not an empty directory.
    #[error("export destination exists and is not an empty directory")]
    DestinationNotEmpty,
    /// The export destination is the vault directory or inside it; plaintext must never be
    /// written into a vault (it may be synchronised).
    #[error("export destination is inside the vault directory")]
    DestinationInsideVault,
    /// The export destination is a synchronised folder (or contains or lies inside one); the
    /// plaintext would be uploaded.
    #[error("export destination is inside or contains a synchronised folder")]
    DestinationSynced,
    /// The import source contains the vault directory or lies inside it.
    #[error("import source overlaps the vault directory")]
    SourceOverlapsVault,
    /// The vault header on disk changed underneath this session; reopen the vault.
    #[error("vault header changed on disk since unlock")]
    HeaderChanged,
    /// Another Nuvoletta window or process has this vault open.
    #[error("this vault is already open in another Nuvoletta window or process")]
    VaultInUse,
    /// The note file does not belong at this location (id mismatch between name and contents).
    #[error("note file does not match its id")]
    IdMismatch,
}

/// Result alias.
pub type Result<T> = core::result::Result<T, Error>;
