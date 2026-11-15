//! Errors of the WebDAV client, kept apart from the sync engine's [`ProviderError`] so the
//! application can tell, for example, an untrusted certificate (offer pinning) from a network
//! failure (retry later).

use nuvoletta_sync::ProviderError;
use std::time::Duration;

/// Why a TLS connection was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsFailure {
    /// The certificate does not chain to a known authority, has the wrong name or has expired.
    /// The text names the reason. Offer to pin the certificate's fingerprint.
    Untrusted(String),
    /// A fingerprint is pinned and the server presented another certificate.
    PinMismatch,
    /// Any other TLS failure (protocol, handshake).
    Other(String),
}

/// A failed WebDAV request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavError {
    /// The server could not be reached or the connection broke. Retrying may help.
    #[error("network error: {0}")]
    Network(String),
    /// The server did not answer in time. Retrying may help.
    #[error("the server did not answer in time ({0})")]
    Timeout(String),
    /// 429 or 503: the server asks for a pause.
    #[error("the server is busy or limiting requests")]
    Throttled {
        /// The pause the server asked for, if any.
        retry_after: Option<Duration>,
    },
    /// The secure connection was refused.
    #[error("secure connection failed: {0:?}")]
    Tls(TlsFailure),
    /// 401.
    #[error("the user name or password was rejected")]
    Auth,
    /// 403.
    #[error("the server refused this operation for this account")]
    Forbidden,
    /// 404.
    #[error("not found")]
    NotFound,
    /// 409.
    #[error("conflict (a parent folder is missing)")]
    Conflict,
    /// 412.
    #[error("the file already exists")]
    PreconditionFailed,
    /// 423.
    #[error("the file is locked")]
    Locked,
    /// A redirect that was not followed.
    #[error("redirect refused: {0}")]
    Redirect(String),
    /// The server's answer made no sense (bad XML, impossible values).
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
    /// Any other status.
    #[error("the server answered with status {0}")]
    Status(u16),
}

impl DavError {
    /// Whether retrying the same request later may succeed.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout(_) | Self::Throttled { .. } | Self::Locked
        ) || matches!(self, Self::Status(s) if (500..600).contains(s))
    }
}

impl From<DavError> for ProviderError {
    fn from(e: DavError) -> Self {
        match e {
            DavError::NotFound => Self::NotFound(String::from("remote file")),
            DavError::PreconditionFailed => Self::AlreadyExists(String::from("remote file")),
            DavError::Auth => Self::Auth("the user name or password was rejected".to_owned()),
            DavError::Forbidden => {
                Self::Forbidden("the server refused the request for this account".to_owned())
            }
            other if other.is_transient() => Self::Transient(other.to_string()),
            other => Self::Other(other.to_string()),
        }
    }
}
