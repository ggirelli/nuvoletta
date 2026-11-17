//! Plaintext Markdown export and import (`docs/EXPORT_FORMAT.md`).
//!
//! Export and import are the portability mechanism of Nuvoletta: a vault can be written out as a
//! folder of ordinary `.md` files and such a folder can be read back into a vault.
//!
//! **Export writes plaintext.** Everything in the destination is readable by anyone with access
//! to it, and the files are not erased by locking the vault. This is a user-initiated operation;
//! the UI is responsible for warning the user and for choosing the destination.
//!
//! Nothing here changes the encrypted vault format: import goes through
//! [`Unlocked::create_note`](crate::Unlocked::create_note).

mod export;
mod filename;
mod frontmatter;
mod import;
mod timestamp;

/// Entry points for the fuzz targets; not part of the API.
#[doc(hidden)]
pub mod fuzzing {
    use nuvoletta_core::Note;

    /// Parse one Markdown file as import does (`segs`: the path segments of the file name).
    ///
    /// # Errors
    /// The skip reason, as text.
    pub fn parse_note(text: &str, segs: &[String]) -> std::result::Result<Note, String> {
        super::import::fuzz_build_note(text, segs)
    }

    /// The front matter of `note` followed by its body, as export writes the file.
    #[must_use]
    pub fn emit_file(note: &Note) -> String {
        format!("{}{}", super::frontmatter::emit(note), note.body)
    }

    /// Escape a virtual path segment into a file name component.
    #[must_use]
    pub fn escape_segment(seg: &str) -> String {
        super::filename::escape_segment(seg)
    }
}

use std::path::{Path, PathBuf};

/// Canonical form of `path` even if it does not exist yet: the nearest existing ancestor is
/// canonicalised (resolving symlinks) and the missing components are appended.
pub(crate) fn canonical_lenient(path: &Path) -> std::io::Result<PathBuf> {
    let abs = std::path::absolute(path)?;
    let mut missing = Vec::new();
    let mut cur = abs.as_path();
    loop {
        match cur.canonicalize() {
            Ok(mut c) => {
                c.extend(missing.iter().rev());
                return Ok(c);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let (Some(name), Some(parent)) = (cur.file_name(), cur.parent()) else {
                    return Err(e);
                };
                missing.push(name.to_owned());
                cur = parent;
            }
            Err(e) => return Err(e),
        }
    }
}

pub use export::{
    ExportReport, ExportedNote, FailedNote, export_markdown, export_markdown_avoiding,
};
pub use import::{
    ImportConflict, ImportReport, ImportedNote, SkipReason, SkippedFile, import_markdown,
};
