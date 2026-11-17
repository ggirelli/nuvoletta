//! Import of a folder of Markdown files into a vault.

use super::frontmatter::{self, FrontMatter, Split};
use crate::error::{Error, Result};
use crate::fsutil::{ensure_real_dir, open_nofollow};
use crate::unlocked::Unlocked;
use nuvoletta_core::{MAX_BODY, Note, NoteId, NotePath, now_ms};
use std::fs;
use std::io::Read;
use std::path::Path;
use zeroize::Zeroize;

/// Room for the front matter on top of the largest body.
const MAX_FILE: u64 = MAX_BODY as u64 + 1024 * 1024;
/// Deepest directory nesting that is followed.
const MAX_DEPTH: usize = 32;

/// A note that was added to the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedNote {
    /// Source file, relative to the source folder, `/`-separated.
    pub file: String,
    /// Id the note has in the vault (fresh if the file had none, or if it conflicted).
    pub id: NoteId,
    /// Front matter keys that are not part of the format and were ignored.
    pub unknown_keys: Vec<String>,
}

/// A file whose id already exists in the vault with different content. The file was imported as
/// a new note under a fresh id (it is also listed in [`ImportReport::imported`]); the existing
/// note was not touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportConflict {
    /// Source file, relative to the source folder.
    pub file: String,
    /// The id in the file, which belongs to a different note in the vault.
    pub existing_id: NoteId,
    /// The fresh id the imported note received.
    pub new_id: NoteId,
}

/// Why a file or directory was not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Not a `.md` file.
    NotMarkdown,
    /// A directory whose name starts with `.`.
    HiddenDirectory,
    /// A symlink; never followed.
    Symlink,
    /// Neither a regular file nor a directory.
    SpecialFile,
    /// Directories nested deeper than the limit.
    TooDeep,
    /// The file name is not valid UTF-8.
    InvalidFileName,
    /// The contents are not valid UTF-8.
    NotUtf8,
    /// The file is larger than the note size limit allows.
    TooLarge,
    /// The front matter is closed but invalid; the file is left for the user to fix.
    MalformedFrontMatter(&'static str),
    /// The virtual path (from the front matter or the file name) is invalid.
    InvalidPath(&'static str),
    /// The note violates a size limit.
    InvalidNote(&'static str),
    /// The file could not be read.
    Unreadable,
    /// The vault refused to store the note.
    StoreFailed,
}

/// A file or directory that was not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    /// Path relative to the source folder, `/`-separated (lossy for odd names).
    pub file: String,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// Outcome of [`import_markdown`]. Every file under the source folder is accounted for in
/// exactly one of `imported`, `already_present` or `skipped`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Notes added to the vault.
    pub imported: Vec<ImportedNote>,
    /// Files identical to a note already in the vault (same id and content); nothing was done.
    pub already_present: Vec<String>,
    /// Id conflicts; each conflicting file was imported as a new note.
    pub conflicts: Vec<ImportConflict>,
    /// Files and directories that were not imported, with the reason.
    pub skipped: Vec<SkippedFile>,
}

/// Import every `.md` file under `src` (recursively) into `vault`.
///
/// For each file: an optional front matter block (as written by
/// [`export_markdown`](super::export_markdown), also hand-written `title`/`tags`/`created`/
/// `modified`) supplies the metadata; the rest is the body, verbatim.
///
/// - `title` defaults to the file stem, `path` to the relative path without `.md`, `tags` to
///   none, timestamps to now (a missing `modified` takes the value of `created`), `id` to a
///   fresh id.
/// - **Id conflict policy:** if the file's id already exists in the vault with identical content
///   the file is listed in `already_present` (so importing the same export twice is harmless);
///   with different content the file is imported as a *new note with a fresh id*, the existing
///   note is never overwritten, and the file is listed in `conflicts`. Nothing is dropped.
/// - Unknown front matter keys are ignored and listed per note in `unknown_keys`.
/// - Non-UTF-8 files, files too large, invalid paths, malformed front matter, symlinks, hidden
///   directories and non-Markdown files are skipped and reported; one bad file never aborts the
///   import. A file that starts with `---` but has no closing `---` line has no front matter.
///
/// # Errors
/// [`Error::SourceOverlapsVault`] if `src` contains the vault directory or lies inside it.
/// Fails if `src` is not a real directory (a symlink is refused) or cannot be listed. Problems
/// with individual files are reported, not returned.
pub fn import_markdown(vault: &mut Unlocked, src: &Path) -> Result<ImportReport> {
    ensure_real_dir(src, false)?;
    let (s, v) = (
        super::canonical_lenient(src)?,
        super::canonical_lenient(vault.root())?,
    );
    if s.starts_with(&v) || v.starts_with(&s) {
        return Err(Error::SourceOverlapsVault);
    }
    let mut report = ImportReport::default();
    walk(vault, src, &mut Vec::new(), &mut report)?;
    Ok(report)
}

fn walk(
    vault: &mut Unlocked,
    dir: &Path,
    rel: &mut Vec<String>,
    report: &mut ImportReport,
) -> Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let os_name = entry.file_name();
        let lossy = os_name.to_string_lossy().into_owned();
        let display = |rel: &[String]| {
            let mut parts = rel.to_vec();
            parts.push(lossy.clone());
            parts.join("/")
        };
        let mut skip = |rel: &[String], reason| {
            report.skipped.push(SkippedFile {
                file: display(rel),
                reason,
            });
        };
        let Ok(ft) = entry.file_type() else {
            skip(rel, SkipReason::Unreadable);
            continue;
        };
        if ft.is_symlink() {
            skip(rel, SkipReason::Symlink);
        } else if ft.is_dir() {
            if lossy.starts_with('.') {
                skip(rel, SkipReason::HiddenDirectory);
            } else if rel.len() >= MAX_DEPTH {
                skip(rel, SkipReason::TooDeep);
            } else if os_name.to_str().is_none() {
                skip(rel, SkipReason::InvalidFileName);
            } else {
                rel.push(lossy);
                walk(vault, &entry.path(), rel, report)?;
                rel.pop();
            }
        } else if ft.is_file() {
            let Some(name) = os_name.to_str() else {
                skip(rel, SkipReason::InvalidFileName);
                continue;
            };
            let Some(stem) = strip_md(name) else {
                skip(rel, SkipReason::NotMarkdown);
                continue;
            };
            let file = display(rel);
            let mut segs = rel.clone();
            segs.push(stem.to_owned());
            match import_file(vault, &entry.path(), &segs) {
                Ok(Outcome::Imported(n)) => report.imported.push(ImportedNote { file, ..n }),
                Ok(Outcome::Conflict(n, existing_id)) => {
                    report.conflicts.push(ImportConflict {
                        file: file.clone(),
                        existing_id,
                        new_id: n.id,
                    });
                    report.imported.push(ImportedNote { file, ..n });
                }
                Ok(Outcome::AlreadyPresent) => report.already_present.push(file),
                Err(reason) => report.skipped.push(SkippedFile { file, reason }),
            }
        } else {
            skip(rel, SkipReason::SpecialFile);
        }
    }
    Ok(())
}

fn strip_md(name: &str) -> Option<&str> {
    let split = name.len().checked_sub(3)?;
    let (stem, ext) = (name.get(..split)?, name.get(split..)?);
    (ext.eq_ignore_ascii_case(".md") && !stem.is_empty()).then_some(stem)
}

enum Outcome {
    Imported(ImportedNote),
    Conflict(ImportedNote, NoteId),
    AlreadyPresent,
}

fn read_text(path: &Path) -> std::result::Result<String, SkipReason> {
    // Open first (never following a symlink), then judge the handle: no check/use race.
    let f = open_nofollow(path).map_err(|e| match e {
        crate::Error::UnsafeEntry("symlink") => SkipReason::Symlink,
        _ => SkipReason::Unreadable,
    })?;
    let meta = f.metadata().map_err(|_| SkipReason::Unreadable)?;
    if !meta.is_file() {
        return Err(SkipReason::Unreadable);
    }
    if meta.len() > MAX_FILE {
        return Err(SkipReason::TooLarge);
    }
    let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    f.take(MAX_FILE + 1)
        .read_to_end(&mut buf)
        .map_err(|_| SkipReason::Unreadable)?;
    if buf.len() as u64 > MAX_FILE {
        buf.zeroize();
        return Err(SkipReason::TooLarge);
    }
    String::from_utf8(buf).map_err(|e| {
        e.into_bytes().zeroize();
        SkipReason::NotUtf8
    })
}

fn import_file(
    vault: &mut Unlocked,
    path: &Path,
    segs: &[String],
) -> std::result::Result<Outcome, SkipReason> {
    let mut text = read_text(path)?;
    let result = build_note(&text, segs).and_then(|(mut note, unknown_keys)| {
        let outcome = store(vault, &mut note, unknown_keys);
        note.zeroize();
        outcome
    });
    text.zeroize();
    result
}

fn build_note(text: &str, segs: &[String]) -> std::result::Result<(Note, Vec<String>), SkipReason> {
    let (fm, body) = match frontmatter::split(text) {
        Split::None => (FrontMatter::default(), text),
        Split::Block { front, body } => (
            frontmatter::parse(front).map_err(SkipReason::MalformedFrontMatter)?,
            body,
        ),
    };
    let from_file = segs.join("/");
    let path = NotePath::parse(fm.path.as_deref().unwrap_or(&from_file)).map_err(|e| match e {
        nuvoletta_core::Error::InvalidPath(r) => SkipReason::InvalidPath(r),
        _ => SkipReason::InvalidPath("invalid"),
    })?;
    let title = fm
        .title
        .unwrap_or_else(|| segs.last().cloned().unwrap_or_default());
    let now = now_ms();
    let created = fm.created.or(fm.modified).unwrap_or(now);
    let note = Note {
        id: fm
            .id
            .map_or_else(NoteId::generate, Ok)
            .map_err(|_| SkipReason::StoreFailed)?,
        title,
        path,
        tags: fm.tags.unwrap_or_default(),
        created_at: created,
        modified_at: fm.modified.unwrap_or(created),
        body: body.to_owned(),
    };
    note.validate().map_err(|e| match e {
        nuvoletta_core::Error::InvalidNote("body too large") => SkipReason::TooLarge,
        nuvoletta_core::Error::InvalidNote(r) => SkipReason::InvalidNote(r),
        _ => SkipReason::InvalidNote("invalid"),
    })?;
    Ok((note, fm.unknown_keys))
}

pub(super) fn fuzz_build_note(text: &str, segs: &[String]) -> std::result::Result<Note, String> {
    build_note(text, segs)
        .map(|(n, _)| n)
        .map_err(|e| format!("{e:?}"))
}

fn store(
    vault: &mut Unlocked,
    note: &mut Note,
    unknown_keys: Vec<String>,
) -> std::result::Result<Outcome, SkipReason> {
    let mut conflict_with = None;
    if vault.summary(&note.id).is_some() {
        match vault.read_note(&note.id) {
            Ok(existing) if existing == *note => return Ok(Outcome::AlreadyPresent),
            Ok(_) | Err(Error::Crypto(_) | Error::Core(_) | Error::IdMismatch) => {}
            Err(_) => return Err(SkipReason::StoreFailed),
        }
        conflict_with = Some(note.id);
        note.id = NoteId::generate().map_err(|_| SkipReason::StoreFailed)?;
    }
    vault
        .create_note(note)
        .map_err(|_| SkipReason::StoreFailed)?;
    let imported = ImportedNote {
        file: String::new(),
        id: note.id,
        unknown_keys,
    };
    Ok(match conflict_with {
        Some(id) => Outcome::Conflict(imported, id),
        None => Outcome::Imported(imported),
    })
}
