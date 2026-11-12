//! The note model.

use crate::error::{Error, Result};
use crate::id::NoteId;
use crate::path::NotePath;
use core::fmt;
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Maximum body size in bytes (`docs/CRYPTO_FORMAT.md` section 6.2).
pub const MAX_BODY: usize = 16 * 1024 * 1024;
/// Maximum title size in bytes.
pub const MAX_TITLE: usize = 1024;
/// Maximum number of tags.
pub const MAX_TAGS: usize = 256;

/// Milliseconds since the Unix epoch, UTC. Informational only: never used for ordering or
/// conflict resolution.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A note: identity plus the title, virtual path, tags, Markdown body and timestamps that make up
/// one revision. Plaintext is zeroized on drop (best effort).
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Note {
    /// Stable identity, independent of title, path and file name.
    pub id: NoteId,
    /// Display title.
    pub title: String,
    /// Virtual path.
    pub path: NotePath,
    /// Tags.
    pub tags: Vec<String>,
    /// Creation time in ms since the Unix epoch (informational).
    pub created_at: i64,
    /// Last modification time in ms (informational).
    pub modified_at: i64,
    /// Markdown body.
    pub body: String,
}

impl fmt::Debug for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Note({}, <redacted>)", self.id)
    }
}

impl Note {
    /// Create a new note with a fresh id and both timestamps set to now.
    ///
    /// # Errors
    /// Fails if the path is invalid, a limit is exceeded or the RNG fails.
    pub fn new(title: &str, path: &str, tags: Vec<String>, body: &str) -> Result<Self> {
        let now = now_ms();
        let note = Self {
            id: NoteId::generate()?,
            title: title.to_owned(),
            path: NotePath::parse(path)?,
            tags,
            created_at: now,
            modified_at: now,
            body: body.to_owned(),
        };
        note.validate()?;
        Ok(note)
    }

    /// Check the size limits. The path is valid by construction.
    ///
    /// # Errors
    /// Returns [`Error::InvalidNote`] if the title, tags or body exceed their limits.
    pub fn validate(&self) -> Result<()> {
        if self.title.len() > MAX_TITLE {
            return Err(Error::InvalidNote("title too long"));
        }
        if self.tags.len() > MAX_TAGS {
            return Err(Error::InvalidNote("too many tags"));
        }
        if self.body.len() > MAX_BODY {
            return Err(Error::InvalidNote("body too large"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_note_has_matching_timestamps_and_unique_id() {
        let a = Note::new("t", "a/b", vec!["x".into()], "body").unwrap();
        let b = Note::new("t", "a/b", vec![], "body").unwrap();
        assert_eq!(a.created_at, a.modified_at);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn invalid_path_is_rejected() {
        assert!(Note::new("t", "../x", vec![], "b").is_err());
    }

    #[test]
    fn limits_are_enforced() {
        assert!(Note::new(&"t".repeat(MAX_TITLE + 1), "a", vec![], "").is_err());
        assert!(Note::new("t", "a", vec![String::new(); MAX_TAGS + 1], "").is_err());
        assert!(Note::new("t", "a", vec![String::new(); MAX_TAGS], "").is_ok());
        assert!(Note::new("t", "a", vec![], &"b".repeat(MAX_BODY + 1)).is_err());
    }

    #[test]
    fn debug_output_hides_content() {
        let n = Note::new("secret title", "secret/path", vec![], "secret body").unwrap();
        let s = format!("{n:?}");
        assert!(!s.contains("secret"));
    }
}
