//! Application errors. Messages never contain note content or secrets.

/// Errors returned by [`crate::App`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No vault is open.
    #[error("no vault is open")]
    NoVault,
    /// The operation needs an unlocked vault.
    #[error("the vault is locked")]
    Locked,
    /// The operation needs a locked vault.
    #[error("the vault is already unlocked")]
    AlreadyUnlocked,
    /// The note is not the open note (stale editor message) or does not exist.
    #[error("that note is not open")]
    NotOpen,
    /// Unsaved edits could not be written; they are still in the editor buffer.
    #[error("unsaved changes could not be saved: {0}")]
    Unsaved(Box<Error>),
    /// Invalid user input.
    #[error("{0}")]
    Invalid(&'static str),
    /// A sync cycle currently has exclusive use of the vault; retry in a moment.
    #[error("a sync cycle is running; try again in a moment")]
    SyncBusy,
    /// Sync is not set up (no folder, or disabled).
    #[error("sync is not set up for this vault")]
    SyncNotConfigured,
    /// Sync failed.
    #[error("sync: {0}")]
    Sync(String),
    /// Input that cannot be used (a bad web address, a missing password), in words for the user.
    #[error("{0}")]
    Input(String),
    /// Settings could not be read or written.
    #[error("settings: {0}")]
    Settings(String),
    /// Storage error.
    #[error(transparent)]
    Storage(#[from] nuvoletta_storage::Error),
    /// Domain error.
    #[error(transparent)]
    Core(#[from] nuvoletta_core::Error),
    /// Crypto error.
    #[error(transparent)]
    Crypto(#[from] nuvoletta_crypto::Error),
}

/// Result alias.
pub type Result<T> = core::result::Result<T, Error>;

impl Error {
    /// A message suitable for showing to the user.
    #[must_use]
    pub fn user_message(&self) -> String {
        use nuvoletta_storage::Error as S;
        match self {
            Self::Storage(S::Crypto(nuvoletta_crypto::Error::Unlock))
            | Self::Crypto(nuvoletta_crypto::Error::Unlock) => {
                "Wrong passphrase or recovery key, or the vault header is damaged.".into()
            }
            Self::Storage(S::NotAVault) => "That folder does not contain a Nuvoletta vault.".into(),
            Self::Storage(S::VaultExists) => "That folder already contains a vault.".into(),
            other => other.to_string(),
        }
    }
}
