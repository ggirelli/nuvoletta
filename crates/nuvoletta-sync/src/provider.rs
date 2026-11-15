//! The provider interface (`SYNC_PROTOCOL.md` section 1.6) and its capability model.
//!
//! A provider is a dumb store of immutable files addressed by `/`-separated relative names such
//! as `d/<device>/l/<seq>.nvlog`. The engine only ever lists, reads, creates and deletes; it never
//! overwrites a file.

/// What a provider can guarantee. The engine never relies on more than `put_new` being best
/// effort; capabilities only let it skip defensive work and let the UI explain limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// `put_new` fails with [`ProviderError::AlreadyExists`] atomically if the name exists.
    /// If false, `put_new` is check-then-write and may overwrite under a race.
    pub create_only: bool,
    /// A file appears under its final name only once complete (write to a `.tmp-` name, then
    /// rename or move).
    pub atomic_rename: bool,
    /// `list` returns an etag that changes when a file's content changes.
    pub etags: bool,
    /// `list` returns modification times.
    pub mtimes: bool,
    /// `delete` may be used. False for providers that are operated with credentials that must
    /// not be able to delete (WebDAV, `docs/WEBDAV.md`): garbage collection is unavailable there
    /// and nothing may rely on removing a file (no temporary names, no leftover cleanup).
    pub can_delete: bool,
    /// A file that is listed was written completely by a request that has ended, and there is
    /// no propagation delay: what the store shows is the truth (a server, not a synced folder).
    /// A half-written file under a name this device owns is then permanent rather than "not yet
    /// arrived", which lets the engine move on instead of waiting forever.
    pub authoritative: bool,
}

/// One file in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    /// Name relative to the sync root, `/`-separated.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Provider etag, if the capability exists.
    pub etag: Option<String>,
    /// Modification time (ms since the epoch), if known. Informational only.
    pub mtime_ms: Option<i64>,
}

/// Typed provider failure.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The file does not exist (yet).
    #[error("not found: {0}")]
    NotFound(String),
    /// `put_new` found the name taken. The existing file is never modified.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// A failure that may succeed if retried (network, timeout, busy).
    #[error("transient provider failure: {0}")]
    Transient(String),
    /// The store rejected the credentials.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// The credentials are valid but may not do this (read, create, list).
    #[error("not permitted: {0}")]
    Forbidden(String),
    /// The credentials can do more than the policy allows (they can delete or overwrite), so the
    /// store is not used at all.
    #[error("unsafe credentials: {0}")]
    UnsafeCredentials(String),
    /// Any other failure (invalid name, unsafe entry, corrupt store).
    #[error("provider failure: {0}")]
    Other(String),
}

impl ProviderError {
    /// Whether retrying the same operation later may succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

/// A store of immutable files. Implementations must be safe against hostile content: they may
/// return anything, so the engine verifies every byte it reads.
pub trait Provider {
    /// The provider's guarantees.
    fn capabilities(&self) -> Capabilities;

    /// List all regular files under the directory `prefix` (`""` is the root), recursively, with
    /// names relative to the root. Symlinks and anything that is not a regular file are omitted.
    ///
    /// # Errors
    /// Provider failures; a missing directory is an empty listing, not an error.
    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError>;

    /// The listing a sync cycle needs: like `list("")`, but a provider whose listings cost one
    /// request per directory may leave out directories only used for diagnostics and garbage
    /// collection (the sharded `o/` tree), because a cycle fetches objects by name. The default
    /// is the full listing.
    ///
    /// # Errors
    /// Provider failures.
    fn list_cycle(&self) -> Result<Vec<RemoteEntry>, ProviderError> {
        self.list("")
    }

    /// Read a whole file.
    ///
    /// # Errors
    /// [`ProviderError::NotFound`] if absent.
    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError>;

    /// Create a file that must not exist. Never overwrites when `create_only` is set.
    ///
    /// # Errors
    /// [`ProviderError::AlreadyExists`] if the name is taken.
    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError>;

    /// Delete a file. Deleting a missing file is [`ProviderError::NotFound`].
    ///
    /// # Errors
    /// Provider failures.
    fn delete(&self, name: &str) -> Result<(), ProviderError>;
}

impl<P: Provider + ?Sized> Provider for &P {
    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError> {
        (**self).list(prefix)
    }
    fn list_cycle(&self) -> Result<Vec<RemoteEntry>, ProviderError> {
        (**self).list_cycle()
    }
    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError> {
        (**self).get(name)
    }
    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        (**self).put_new(name, bytes)
    }
    fn delete(&self, name: &str) -> Result<(), ProviderError> {
        (**self).delete(name)
    }
}

impl<P: Provider + ?Sized> Provider for Box<P> {
    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError> {
        (**self).list(prefix)
    }
    fn list_cycle(&self) -> Result<Vec<RemoteEntry>, ProviderError> {
        (**self).list_cycle()
    }
    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError> {
        (**self).get(name)
    }
    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        (**self).put_new(name, bytes)
    }
    fn delete(&self, name: &str) -> Result<(), ProviderError> {
        (**self).delete(name)
    }
}

impl<P: Provider + ?Sized> Provider for std::sync::Arc<P> {
    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError> {
        (**self).list(prefix)
    }
    fn list_cycle(&self) -> Result<Vec<RemoteEntry>, ProviderError> {
        (**self).list_cycle()
    }
    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError> {
        (**self).get(name)
    }
    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        (**self).put_new(name, bytes)
    }
    fn delete(&self, name: &str) -> Result<(), ProviderError> {
        (**self).delete(name)
    }
}
