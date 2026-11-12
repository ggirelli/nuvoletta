//! Typed errors. Messages never contain secret material.

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// The OS random number generator failed.
    #[error("operating system random number generator failed")]
    Rng,
    /// The vault header is structurally invalid (checked before any key derivation).
    #[error("malformed vault header: {0}")]
    MalformedHeader(&'static str),
    /// Unknown or unsupported format version. Update Nuvoletta.
    #[error("unsupported format version")]
    UnsupportedVersion,
    /// Unknown or unsupported algorithm identifier.
    #[error("unsupported algorithm identifier")]
    UnsupportedAlgorithm,
    /// Key derivation parameters are outside the accepted range.
    #[error("key derivation parameters out of range")]
    KdfParamsOutOfRange,
    /// Unlock failed: wrong passphrase/recovery key or corrupted header (deliberately indistinguishable).
    #[error("unlock failed: wrong passphrase or recovery key, or corrupted header")]
    Unlock,
    /// The new passphrase is shorter than the minimum.
    #[error("passphrase must be at least {min} characters")]
    PassphraseTooShort {
        /// Minimum number of characters.
        min: usize,
    },
    /// The recovery key text is not well formed (wrong length or characters).
    #[error("recovery key is not well formed")]
    RecoveryKeyFormat,
    /// The recovery key checksum failed: most likely a typo.
    #[error("recovery key checksum mismatch (typo)")]
    RecoveryKeyChecksum,
    /// The object envelope is structurally invalid.
    #[error("malformed object envelope: {0}")]
    MalformedEnvelope(&'static str),
    /// The envelope belongs to a different vault.
    #[error("object belongs to a different vault")]
    VaultMismatch,
    /// Authentication failed; no plaintext is returned.
    #[error("authentication failed")]
    Authentication,
    /// The decrypted padding is invalid.
    #[error("invalid padding")]
    InvalidPadding,
    /// Object id or scope id do not match the expected context.
    #[error("object context mismatch")]
    ContextMismatch,
    /// The payload is too large to encode.
    #[error("payload too large")]
    TooLarge,
    /// The payload is invalid.
    #[error("invalid payload: {0}")]
    Payload(&'static str),
}

/// Result alias.
pub type Result<T> = core::result::Result<T, Error>;
