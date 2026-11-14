//! Export of a vault to a folder of Markdown files.

use super::filename::{escape_segment, fold};
use super::frontmatter;
use crate::error::{Error, Result};
use crate::unlocked::Unlocked;
use nuvoletta_core::NoteId;
use std::collections::HashSet;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

/// One note that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedNote {
    /// Id of the note.
    pub id: NoteId,
    /// File written, relative to the destination, `/`-separated.
    pub file: String,
    /// True if the file name differs from the escaped virtual path because of a name collision
    /// (a short id was appended).
    pub disambiguated: bool,
}

/// A note that could not be read from the vault (for example because its file is corrupt) and
/// was therefore not exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedNote {
    /// Id of the note.
    pub id: NoteId,
    /// Why, without note content.
    pub reason: String,
}

/// Outcome of [`export_markdown`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Notes written.
    pub exported: Vec<ExportedNote>,
    /// Notes that could not be read and were not written.
    pub failed: Vec<FailedNote>,
}

/// Export every note of `vault` as plaintext Markdown under `dest`.
///
/// **This writes plaintext.** The files are not encrypted and outlive the unlocked session; the
/// caller (the UI) must make the user aware of that. On Unix the files are created readable by
/// the owner only.
///
/// Layout: the note at virtual path `a/b/c` becomes `<dest>/a/b/c.md` (see
/// `docs/EXPORT_FORMAT.md`), starting with a front matter block (`id`, `title`, `path`, `tags`,
/// `created`, `modified`) followed by the body verbatim. Path segments are escaped so the names
/// are valid on Linux, macOS and Windows; notes whose names would collide (also case-insensitively)
/// get `~<first 8 hex digits of the id>` appended to the file stem.
///
/// `dest` must not be the vault directory or inside it (checked after resolving symlinks).
/// It must not exist, or be an empty real directory; existing files are never overwritten.
/// Notes that cannot be read are listed in [`ExportReport::failed`] and the rest is exported.
///
/// # Errors
/// [`Error::DestinationInsideVault`] if `dest` is within the vault, [`Error::DestinationNotEmpty`] if `dest` has content, [`Error::UnsafeEntry`] if it is a symlink
/// or not a directory, or an I/O error while writing (files already written stay).
pub fn export_markdown(vault: &Unlocked, dest: &Path) -> Result<ExportReport> {
    if super::canonical_lenient(dest)?.starts_with(super::canonical_lenient(vault.root())?) {
        return Err(Error::DestinationInsideVault);
    }
    prepare_destination(dest)?;
    let summaries = vault.list();

    let split: Vec<(Vec<String>, String)> = summaries
        .iter()
        .map(|s| {
            let mut segs: Vec<String> = s.path.segments().map(escape_segment).collect();
            let stem = segs.pop().unwrap_or_default();
            (segs, stem)
        })
        .collect();

    // Claim every directory first, so a file never takes the name of a directory.
    let mut dirs: HashSet<String> = HashSet::new();
    for (segs, _) in &split {
        for n in 1..=segs.len() {
            dirs.insert(fold(&segs[..n].join("/")));
        }
    }
    let mut files: HashSet<String> = HashSet::new();
    let mut report = ExportReport::default();

    for (summary, (dir_segs, stem)) in summaries.iter().zip(&split) {
        let id_hex = summary.id.to_string();
        let candidates = [
            stem.clone(),
            format!("{stem}~{}", &id_hex[..8]),
            format!("{stem}~{id_hex}"),
        ];
        let mut chosen = None;
        for (i, c) in candidates.iter().enumerate() {
            let mut rel = dir_segs.clone();
            rel.push(format!("{c}.md"));
            let key = fold(&rel.join("/"));
            if !files.contains(&key) && !dirs.contains(&key) {
                files.insert(key);
                chosen = Some((rel, i > 0));
                break;
            }
        }
        let (rel, disambiguated) = chosen.ok_or(Error::UnsafeEntry("export name collision"))?;

        let mut note = match vault.read_note(&summary.id) {
            Ok(n) => n,
            Err(e) => {
                report.failed.push(FailedNote {
                    id: summary.id,
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let mut content = frontmatter::emit(&note);
        content.push_str(&note.body);
        let written = write_new_file(dest, &rel, content.as_bytes());
        content.zeroize();
        note.zeroize();
        written?;
        report.exported.push(ExportedNote {
            id: summary.id,
            file: rel.join("/"),
            disambiguated,
        });
    }
    Ok(report)
}

fn prepare_destination(dest: &Path) -> Result<()> {
    match fs::symlink_metadata(dest) {
        Ok(m) if m.file_type().is_symlink() => Err(Error::UnsafeEntry("symlinked destination")),
        Ok(m) if m.is_dir() => {
            if fs::read_dir(dest)?.next().is_some() {
                Err(Error::DestinationNotEmpty)
            } else {
                Ok(())
            }
        }
        Ok(_) => Err(Error::UnsafeEntry("destination is not a directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            private_dir_builder().create(dest)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn private_dir_builder() -> DirBuilder {
    let mut b = DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b
}

/// Create `dest/rel...` exclusively (never overwriting), after checking that every component is
/// a plain name so the result cannot leave `dest`.
fn write_new_file(dest: &Path, rel: &[String], bytes: &[u8]) -> Result<()> {
    let mut path: PathBuf = dest.to_path_buf();
    for seg in rel {
        let plain = !seg.is_empty()
            && seg != "."
            && seg != ".."
            && !seg.contains(['/', '\\', '\0'])
            && !seg.contains(':');
        if !plain {
            return Err(Error::UnsafeEntry("unsafe export file name"));
        }
        path.push(seg);
    }
    if let Some(parent) = path.parent() {
        private_dir_builder().create(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
