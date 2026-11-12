//! Errors of the domain layer. Messages never contain note content.

/// Errors returned by this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The OS random number generator failed.
    #[error("operating system random number generator failed")]
    Rng,
    /// A note id is not 32 hexadecimal characters.
    #[error("invalid note id")]
    InvalidId,
    /// A note path violates the rules of `docs/CRYPTO_FORMAT.md` section 6.2.
    #[error("invalid note path: {0}")]
    InvalidPath(&'static str),
    /// A note violates a size limit.
    #[error("invalid note: {0}")]
    InvalidNote(&'static str),
}

/// Result alias.
pub type Result<T> = core::result::Result<T, Error>;
