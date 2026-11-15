//! Errors of the sync engine. Messages never contain note content or secrets.

use crate::provider::ProviderError;

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The storage layer failed.
    #[error(transparent)]
    Storage(#[from] nuvoletta_storage::Error),
    /// A cryptographic operation failed.
    #[error(transparent)]
    Crypto(#[from] nuvoletta_crypto::Error),
    /// The remote store failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// An I/O operation on local state failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The OS random number generator failed.
    #[error("random number generator failure")]
    Rng,
    /// A local state file is malformed or belongs to another vault.
    #[error("sync state is corrupt: {0}")]
    State(&'static str),
    /// A note was not found in the sync state or the vault.
    #[error("note not found")]
    NoteNotFound,
    /// The requested resolution does not apply to the note's current state.
    #[error("invalid resolution: {0}")]
    InvalidResolution(&'static str),
    /// A required revision object is not available locally (not yet fetched).
    #[error("revision content is not available locally")]
    RevisionUnavailable,
    /// A test-only crash point fired (see [`crate::CrashPoint`]).
    #[error("simulated crash at {0:?}")]
    Crash(crate::CrashPoint),
}

/// Result alias.
pub type Result<T> = core::result::Result<T, Error>;
