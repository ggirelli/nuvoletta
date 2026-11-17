//! Local trash: deleted notes kept for 30 days (`docs/STORAGE_FORMAT.md` section 3.1).
//!
//! Deleting a note moves its envelope file, byte for byte, from `notes/` to
//! `.nuvoletta/trash/<note_id>.nvnote`. The deletion times live in one sealed type 4 object,
//! `.nuvoletta/trash/index`. Nothing here is synced or exported. Entries expire automatically
//! after [`RETENTION_MS`]; there is deliberately no way to empty the trash or to delete one item.
//!
//! Crash safety: the index is written first, then the file is moved. A crash leaves the note in
//! `notes/` (index entry without a file, pruned later) or in `trash/`, never in neither.

use crate::error::{Error, Result};
use crate::fsutil::{ensure_real_dir, read_regular, sync_dir, write_atomic};
use crate::layout::{TMP_PREFIX, meta_dir, note_file_name, notes_dir, parse_note_file_name};
use crate::unlocked::{NoteSummary, Unlocked};
use nuvoletta_core::{NoteId, now_ms};
use nuvoletta_crypto::{ObjType, ObjectMeta, open, seal};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// How long a deleted note stays in the local trash: 30 days, the sync GC grace period.
pub const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

const TRASH_DIR: &str = "trash";
const INDEX_FILE: &str = "index";
const MAGIC: &[u8; 6] = b"trash1";
const MAX_FILE: u64 = 32 * 1024 * 1024;

/// A note in the local trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedNote {
    /// Note id.
    pub id: NoteId,
    /// Deletion time (ms since the epoch, local clock).
    pub deleted_ms: i64,
    /// Title, path and so on of the deleted version.
    pub summary: NoteSummary,
}

impl Unlocked {
    fn trash_dir(&self) -> PathBuf {
        meta_dir(self.root()).join(TRASH_DIR)
    }

    fn read_trash_index(&self) -> BTreeMap<NoteId, i64> {
        let mut out = BTreeMap::new();
        let Ok(bytes) = read_regular(&self.trash_dir().join(INDEX_FILE), MAX_FILE) else {
            return out;
        };
        let Ok(opened) = open(self.keys(), &bytes) else {
            return out; // tampered or foreign: times are re-stamped, never made earlier
        };
        let p = &opened.plaintext;
        if opened.meta.obj_type != ObjType::LocalState || !p.starts_with(MAGIC) {
            return out;
        }
        for c in p[MAGIC.len()..].as_chunks::<24>().0 {
            let id = NoteId::from_bytes(c[..16].try_into().expect("16 bytes"));
            let ms = i64::from_be_bytes(c[16..].try_into().expect("8 bytes"));
            out.insert(id, ms);
        }
        out
    }

    fn write_trash_index(&self, index: &BTreeMap<NoteId, i64>) -> Result<()> {
        let mut buf = MAGIC.to_vec();
        for (id, ms) in index {
            buf.extend_from_slice(id.as_bytes());
            buf.extend_from_slice(&ms.to_be_bytes());
        }
        let meta = ObjectMeta::new(ObjType::LocalState, [0; 16])?;
        let sealed = seal(self.keys(), &meta, &buf)?;
        write_atomic(&self.trash_dir(), INDEX_FILE, &sealed)
    }

    fn ensure_trash_dir(&self) -> Result<()> {
        ensure_real_dir(&meta_dir(self.root()), false)?;
        ensure_real_dir(&self.trash_dir(), true)
    }

    /// Ids with a regular file in the trash directory.
    fn trash_files(&self) -> Vec<NoteId> {
        let Ok(rd) = fs::read_dir(self.trash_dir()) else {
            return Vec::new();
        };
        rd.filter_map(|e| {
            let e = e.ok()?;
            if !e.file_type().ok()?.is_file() {
                return None;
            }
            parse_note_file_name(e.file_name().to_str()?)
        })
        .collect()
    }

    /// Move a note to the trash. Called by [`Unlocked::delete_note`].
    ///
    /// If the trash already holds a *different* version under this id (the id came back to life
    /// and is deleted again), the older version is kept: it is re-sealed under a fresh id with
    /// the title suffixed " (earlier version)" and its original deletion time, so both can be
    /// restored. An identical copy is simply not duplicated.
    pub(crate) fn move_to_trash(&self, id: &NoteId) -> Result<()> {
        self.ensure_trash_dir()?;
        let mut index = self.read_trash_index();
        let name = note_file_name(id);
        let from = notes_dir(self.root()).join(&name);
        let to = self.trash_dir().join(&name);
        if fs::symlink_metadata(&to).is_ok() {
            let older = read_regular(&to, MAX_FILE)?;
            if older == read_regular(&from, MAX_FILE)? {
                // Same bytes already in the trash: keep the older stamp, drop the live file.
                fs::remove_file(&from)?;
                sync_dir(&notes_dir(self.root()))?;
                return Ok(());
            }
            self.rehome_older_trash_copy(id, &older, index.get(id).copied(), &mut index)?;
        }
        index.insert(*id, now_ms());
        self.write_trash_index(&index)?;
        fs::rename(&from, &to)?;
        sync_dir(&self.trash_dir())?;
        sync_dir(&notes_dir(self.root()))
    }

    /// Re-seal an older trashed version under a new id so a newer deletion of the same id does
    /// not replace it. An older copy that cannot be decoded is never destroyed: the move is
    /// refused with the decoding error.
    fn rehome_older_trash_copy(
        &self,
        id: &NoteId,
        older: &[u8],
        stamp: Option<i64>,
        index: &mut BTreeMap<NoteId, i64>,
    ) -> Result<()> {
        let (mut note, _) = self.decode_note(id, older)?;
        note.id = NoteId::generate()?;
        let plain = note.title.clone();
        note.title.push_str(" (earlier version)");
        let (envelope, _) = self.seal_note(&note).or_else(|_| {
            note.title = plain; // suffix made the title too long
            self.seal_note(&note)
        })?;
        write_atomic(&self.trash_dir(), &note_file_name(&note.id), &envelope)?;
        index.insert(note.id, stamp.unwrap_or_else(now_ms));
        Ok(())
    }

    /// Notes in the trash, newest deletion first. A note that is also live (for example after a
    /// sync restore) is not listed. Corrupt entries are skipped, never deleted.
    #[must_use]
    pub fn list_trash(&self) -> Vec<TrashedNote> {
        let index = self.read_trash_index();
        let mut out: Vec<TrashedNote> = self
            .trash_files()
            .into_iter()
            .filter(|id| self.summary(id).is_none())
            .filter_map(|id| {
                let note = self.load_trashed(&id).ok()?;
                let deleted_ms = index.get(&id).copied().unwrap_or_else(|| {
                    fs::metadata(self.trash_dir().join(note_file_name(&id)))
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
                });
                Some(TrashedNote {
                    id,
                    deleted_ms,
                    summary: note,
                })
            })
            .collect();
        out.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms).then(a.id.cmp(&b.id)));
        out
    }

    fn load_trashed(&self, id: &NoteId) -> Result<NoteSummary> {
        let bytes = read_regular(&self.trash_dir().join(note_file_name(id)), MAX_FILE)?;
        let (note, rev) = self.decode_note(id, &bytes)?;
        Ok(crate::unlocked::summary_of(&note, rev))
    }

    /// Restore a trashed note. Without a live note of the same id the file is moved back
    /// unchanged. If that id is taken, the trashed version is stored as a new note (new id, title
    /// suffixed " (restored)") so nothing is overwritten. Returns the id of the restored note.
    ///
    /// # Errors
    /// [`Error::NotFound`] if the note is not in the trash; crypto and I/O errors.
    pub fn restore_from_trash(&mut self, id: &NoteId) -> Result<NoteId> {
        let from = self.trash_dir().join(note_file_name(id));
        let bytes = match read_regular(&from, MAX_FILE) {
            Ok(b) => b,
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(*id));
            }
            Err(e) => return Err(e),
        };
        let (mut note, _) = self.decode_note(id, &bytes)?;
        let to = notes_dir(self.root()).join(note_file_name(id));
        let restored = if self.summary(id).is_none() && fs::symlink_metadata(&to).is_err() {
            fs::rename(&from, &to)?;
            sync_dir(&notes_dir(self.root()))?;
            self.reindex(id)?;
            *id
        } else {
            note.id = NoteId::generate()?;
            note.title.push_str(" (restored)");
            self.create_note(&note)?;
            fs::remove_file(&from)?;
            note.id
        };
        sync_dir(&self.trash_dir())?;
        let mut index = self.read_trash_index();
        if index.remove(id).is_some() {
            self.write_trash_index(&index)?;
        }
        Ok(restored)
    }

    /// Housekeeping that runs at unlock and from the app: removes entries older than
    /// [`RETENTION_MS`], copies of notes that are live with identical bytes, and index entries
    /// without a file; stamps files that have no time with the current time. Never touches a
    /// note in `notes/`.
    ///
    /// # Errors
    /// I/O errors.
    pub fn purge_expired(&mut self) -> Result<()> {
        self.purge_expired_at(now_ms())
    }

    /// [`Unlocked::purge_expired`] with an explicit clock, for tests.
    #[doc(hidden)]
    pub fn purge_expired_at(&mut self, now: i64) -> Result<()> {
        if fs::symlink_metadata(self.trash_dir()).is_err() {
            return Ok(());
        }
        let old = self.read_trash_index();
        let mut index = BTreeMap::new();
        for id in self.trash_files() {
            let path = self.trash_dir().join(note_file_name(&id));
            let stamped = old.get(&id).copied().unwrap_or(now);
            let live = notes_dir(self.root()).join(note_file_name(&id));
            let identical = read_regular(&live, MAX_FILE)
                .ok()
                .is_some_and(|l| read_regular(&path, MAX_FILE).ok().as_ref() == Some(&l));
            if identical || now.saturating_sub(stamped) > RETENTION_MS {
                fs::remove_file(&path)?;
            } else {
                index.insert(id, stamped);
            }
        }
        if index != old {
            self.write_trash_index(&index)?;
        }
        // Leftover temporary files of interrupted index writes.
        for e in fs::read_dir(self.trash_dir())? {
            let e = e?;
            if e.file_name().to_string_lossy().starts_with(TMP_PREFIX) && e.file_type()?.is_file() {
                fs::remove_file(e.path())?;
            }
        }
        sync_dir(&self.trash_dir())
    }
}
