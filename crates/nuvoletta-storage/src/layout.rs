//! On-disk layout and the strict file-name grammar.
//!
//! ```text
//! <vault>/
//!   .nuvoletta/vault-header        236-byte vault header
//!   notes/<note_id 32 hex>.nvnote  latest revision of a note, one envelope (type 1)
//! ```
//!
//! `.nuvoletta/device-state/`, `sync-state/` and `cache/` are reserved for later phases and are
//! created only when first needed. Any file whose name is not in this grammar is "stray":
//! reported, never read as truth, never deleted (except our own `.tmp-` files).

use nuvoletta_core::NoteId;
use std::path::{Path, PathBuf};

/// Metadata directory.
pub const META_DIR: &str = ".nuvoletta";
/// Header file name inside [`META_DIR`].
pub const HEADER_FILE: &str = "vault-header";
/// Notes directory.
pub const NOTES_DIR: &str = "notes";
/// Note file extension.
pub const NOTE_EXT: &str = ".nvnote";
/// Prefix of temporary files created by atomic writes.
pub const TMP_PREFIX: &str = ".tmp-";

pub(crate) fn meta_dir(root: &Path) -> PathBuf {
    root.join(META_DIR)
}

pub(crate) fn header_path(root: &Path) -> PathBuf {
    meta_dir(root).join(HEADER_FILE)
}

pub(crate) fn notes_dir(root: &Path) -> PathBuf {
    root.join(NOTES_DIR)
}

/// File name of a note: `<32 lower-case hex>.nvnote`.
#[must_use]
pub fn note_file_name(id: &NoteId) -> String {
    format!("{id}{NOTE_EXT}")
}

/// Parse a note file name under the strict grammar (lower-case hex only, so case-insensitive
/// filesystems cannot make two names equal). Returns `None` for anything else.
#[must_use]
pub fn parse_note_file_name(name: &str) -> Option<NoteId> {
    let stem = name.strip_suffix(NOTE_EXT)?;
    if stem.len() != 32 || !stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    stem.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_round_trips() {
        let id = NoteId::from_bytes([7; 16]);
        assert_eq!(parse_note_file_name(&note_file_name(&id)), Some(id));
    }

    #[test]
    fn non_grammar_names_are_rejected() {
        let ok = "07".repeat(16);
        for bad in [
            format!("{}.nvnote", ok.to_uppercase().replace('0', "A")),
            format!("{ok} (1).nvnote"),
            format!("{ok}.nvnote.tmp"),
            format!("../{ok}.nvnote"),
            format!("{ok}/x.nvnote"),
            ".tmp-abc".to_owned(),
            "x.nvnote".to_owned(),
        ] {
            assert_eq!(parse_note_file_name(&bad), None, "{bad}");
        }
    }
}
