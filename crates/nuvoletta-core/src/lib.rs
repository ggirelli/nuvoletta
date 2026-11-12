//! Core domain types of Nuvoletta: note identifiers, virtual note paths and the note model.
//!
//! This crate is pure domain logic: no filesystem, no UI, no provider code. Persistence lives in
//! `nuvoletta-storage` and the encrypted formats in `nuvoletta-crypto`.

#![forbid(unsafe_code)]

pub mod error;
pub mod id;
pub mod note;
pub mod path;

pub use error::{Error, Result};
pub use id::NoteId;
pub use note::{MAX_BODY, MAX_TAGS, MAX_TITLE, Note, now_ms};
pub use path::NotePath;
