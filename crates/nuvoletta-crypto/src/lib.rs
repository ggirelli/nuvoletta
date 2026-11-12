//! Cryptographic format and primitives of Nuvoletta, per `docs/CRYPTO_FORMAT.md`.
//!
//! Crypto only: no filesystem, sync or UI. Secrets live in zeroizing types
//! without `Debug` output; production randomness is always the OS CSPRNG.

#![forbid(unsafe_code)]

pub mod envelope;
pub mod error;
pub mod header;
pub mod keys;
pub mod padding;
pub mod payload;
pub mod recovery;
mod rng;

#[cfg(test)]
mod tests;

pub use envelope::{ObjType, ObjectMeta, OpenedObject, open, seal};
pub use error::{Error, Result};
pub use header::{CreatedVault, KdfParams, Vault, VaultHeader};
pub use keys::{Dek, Subkeys};
pub use payload::NotePayload;
pub use recovery::RecoveryKey;
