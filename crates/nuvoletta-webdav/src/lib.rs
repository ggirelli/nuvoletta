//! WebDAV backend of Nuvoletta (`docs/WEBDAV.md`, `docs/ADR/0007-webdav-backend.md`).
//!
//! [`WebDavProvider`] implements the sync engine's [`nuvoletta_sync::Provider`] over WebDAV with
//! credentials that may read and create files but must not delete or overwrite them. The
//! credential check in [`permissions`] proves that before anything is read or written.
//!
//! Transport is HTTPS only (`ureq` with rustls and the built-in web PKI roots), optionally with one self-signed certificate pinned by SHA-256 fingerprint
//! ([`tls`]).

#![forbid(unsafe_code)]

pub mod client;
pub mod config;
pub mod error;
pub mod permissions;
pub mod provider;
#[cfg(feature = "test-server")]
pub mod testing;
pub mod tls;
pub mod xml;

pub use client::{Client, Options, RetryPolicy};
pub use config::{ConfigError, WebDavConfig, format_fingerprint, parse_fingerprint};
pub use error::{DavError, TlsFailure};
pub use permissions::{Refusal, RefusalReason, Verdict};
pub use provider::WebDavProvider;
pub use tls::fetch_fingerprint;
