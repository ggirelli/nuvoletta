//! Filesystem vault of Nuvoletta (`docs/STORAGE_FORMAT.md`).
//!
//! A vault is an ordinary directory holding the vault header and one encrypted envelope per note.
//! Nothing but ciphertext is ever written to disk; file names are opaque ids; every write is
//! atomic (temporary file, fsync, rename, fsync of the directory). There is no database: the
//! index lives in memory and is rebuilt at unlock.

#![forbid(unsafe_code)]

pub mod error;
mod fsutil;
pub mod layout;
pub mod locked;
pub mod unlocked;

pub use error::{Error, Result};
pub use locked::Locked;
pub use unlocked::{Issue, IssueKind, NoteSummary, Unlocked};
