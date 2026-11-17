//! Filesystem vault of Nuvoletta (`docs/STORAGE_FORMAT.md`).
//!
//! A vault is an ordinary directory holding the vault header and one encrypted envelope per note.
//! Nothing but ciphertext is ever written to disk; file names are opaque ids; every write is
//! atomic (temporary file, fsync, rename, fsync of the directory). There is no database: the
//! index lives in memory and is rebuilt at unlock.

#![forbid(unsafe_code)]

pub mod error;
pub mod fsutil;
pub mod header_change;
pub mod layout;
pub mod lock;
pub mod locked;
pub mod portable;
pub mod trash;
pub mod unlocked;

pub use error::{Error, Result};
pub use header_change::{Cleanup, Refusal, Secret, VerifiedHeader, remove_unused_vault};
pub use locked::Locked;
pub use trash::TrashedNote;
pub use unlocked::{Issue, IssueKind, NoteSummary, Unlocked};
