//! An unlocked vault: keys in memory, in-memory index, note CRUD and header operations.

use crate::error::{Error, Result};
use crate::fsutil::{read_regular, sync_dir, write_atomic};
use crate::layout::{
    HEADER_FILE, TMP_PREFIX, header_path, meta_dir, note_file_name, notes_dir, parse_note_file_name,
};
use crate::locked::Locked;
use nuvoletta_core::{Note, NoteId, NotePath};
use nuvoletta_crypto::{
    KdfParams, NotePayload, ObjType, ObjectMeta, RecoveryKey, Vault, VaultHeader, open, seal,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Upper bound for a note file: the largest body, padding overhead and slack.
const MAX_NOTE_FILE: u64 = 32 * 1024 * 1024;

/// Index entry: everything about a note except its body.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct NoteSummary {
    /// Note id.
    pub id: NoteId,
    /// Title.
    pub title: String,
    /// Virtual path.
    pub path: NotePath,
    /// Tags.
    pub tags: Vec<String>,
    /// Creation time (ms, informational).
    pub created_at: i64,
    /// Modification time (ms, informational).
    pub modified_at: i64,
    /// Id of the stored revision (the envelope `object_id`).
    pub revision_id: [u8; 16],
}

impl std::fmt::Debug for NoteSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NoteSummary({}, <redacted>)", self.id)
    }
}

/// What was wrong with a file found in the notes directory at unlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// Name outside the strict grammar, or not a regular file. Ignored, never deleted.
    Stray,
    /// A symlink; never followed.
    Symlink,
    /// Name is valid but the contents failed authentication, parsing or the id check.
    Corrupt,
}

/// A problem found while building the index. Reported, never fatal, never auto-deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Kind of problem.
    pub kind: IssueKind,
    /// File name (lossy), non-secret.
    pub name: String,
}

/// An unlocked vault. Dropping it, or calling [`Unlocked::lock`], zeroizes the keys.
#[derive(Debug)]
pub struct Unlocked {
    root: PathBuf,
    vault: Vault,
    index: HashMap<NoteId, NoteSummary>,
    issues: Vec<Issue>,
}

impl Unlocked {
    pub(crate) fn new(root: PathBuf, vault: Vault) -> Result<Self> {
        let mut u = Self {
            root,
            vault,
            index: HashMap::new(),
            issues: Vec::new(),
        };
        u.rebuild_index()?;
        Ok(u)
    }

    /// Lock the vault: keys and the index are dropped (zeroized).
    #[must_use]
    pub fn lock(self) -> Locked {
        Locked::from_parts(self.root.clone(), self.vault.header().clone())
    }

    /// The vault directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Problems found while scanning the notes directory at unlock.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// All indexed notes, sorted by path then id.
    #[must_use]
    pub fn list(&self) -> Vec<NoteSummary> {
        let mut v: Vec<_> = self.index.values().cloned().collect();
        v.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
        v
    }

    /// Index entry of one note.
    #[must_use]
    pub fn summary(&self, id: &NoteId) -> Option<&NoteSummary> {
        self.index.get(id)
    }

    fn rebuild_index(&mut self) -> Result<()> {
        self.index.clear();
        self.issues.clear();
        for entry in fs::read_dir(notes_dir(&self.root))? {
            let entry = entry?;
            let os_name = entry.file_name();
            let name = os_name.to_string_lossy().into_owned();
            if name.starts_with(TMP_PREFIX) {
                continue; // another writer's in-flight temporary file
            }
            let ft = entry.file_type()?;
            let kind = if ft.is_symlink() {
                Some(IssueKind::Symlink)
            } else if !ft.is_file() {
                Some(IssueKind::Stray)
            } else {
                None
            };
            let id = os_name.to_str().and_then(parse_note_file_name);
            let (Some(id), None) = (id, kind) else {
                self.issues.push(Issue {
                    kind: kind.unwrap_or(IssueKind::Stray),
                    name,
                });
                continue;
            };
            match self.load(&id) {
                Ok((note, revision_id)) => {
                    self.index.insert(id, summary_of(&note, revision_id));
                }
                Err(_) => self.issues.push(Issue {
                    kind: IssueKind::Corrupt,
                    name,
                }),
            }
        }
        Ok(())
    }

    fn note_path(&self, id: &NoteId) -> PathBuf {
        notes_dir(&self.root).join(note_file_name(id))
    }

    fn load(&self, id: &NoteId) -> Result<(Note, [u8; 16])> {
        let bytes = read_regular(&self.note_path(id), MAX_NOTE_FILE)?;
        let opened = open(self.vault.keys(), &bytes)?;
        if opened.meta.obj_type != ObjType::NoteRevision || &opened.meta.scope_id != id.as_bytes() {
            return Err(Error::IdMismatch);
        }
        let mut payload = NotePayload::decode(&opened.plaintext)?;
        payload.check_scope(id.as_bytes())?;
        let note = Note {
            id: *id,
            title: std::mem::take(&mut payload.title),
            path: NotePath::parse(&payload.path)?,
            tags: std::mem::take(&mut payload.tags),
            created_at: payload.created_at,
            modified_at: payload.modified_at,
            body: std::mem::take(&mut payload.body),
        };
        note.validate()?;
        Ok((note, opened.meta.object_id))
    }

    /// Read and decrypt a note from disk (plaintext is not cached).
    ///
    /// # Errors
    /// [`Error::NotFound`] if the id is not indexed; crypto errors if the file was tampered with.
    pub fn read_note(&self, id: &NoteId) -> Result<Note> {
        if !self.index.contains_key(id) {
            return Err(Error::NotFound(*id));
        }
        Ok(self.load(id)?.0)
    }

    fn store(&mut self, note: &Note) -> Result<()> {
        note.validate()?;
        let mut payload = NotePayload {
            schema_version: 1,
            note_id: *note.id.as_bytes(),
            title: note.title.clone(),
            path: note.path.as_str().to_owned(),
            tags: note.tags.clone(),
            created_at: note.created_at,
            modified_at: note.modified_at,
            body: note.body.clone(),
        };
        let encoded = payload.encode();
        payload.zeroize();
        let meta = ObjectMeta::new(ObjType::NoteRevision, *note.id.as_bytes())?;
        let envelope = seal(self.vault.keys(), &meta, &encoded?)?;
        write_atomic(&notes_dir(&self.root), &note_file_name(&note.id), &envelope)?;
        self.index.insert(note.id, summary_of(note, meta.object_id));
        Ok(())
    }

    /// Store a new note.
    ///
    /// # Errors
    /// [`Error::AlreadyExists`] if the id is taken (indexed or any file at that name).
    pub fn create_note(&mut self, note: &Note) -> Result<()> {
        if self.index.contains_key(&note.id)
            || fs::symlink_metadata(self.note_path(&note.id)).is_ok()
        {
            return Err(Error::AlreadyExists(note.id));
        }
        self.store(note)
    }

    /// Replace an existing note with a new revision (fresh `object_id`, fresh nonce).
    ///
    /// # Errors
    /// [`Error::NotFound`] if the id is not indexed.
    pub fn update_note(&mut self, note: &Note) -> Result<()> {
        if !self.index.contains_key(&note.id) {
            return Err(Error::NotFound(note.id));
        }
        self.store(note)
    }

    /// Delete a note file. (Tombstone log entries arrive with sync, Phase 4.)
    ///
    /// # Errors
    /// [`Error::NotFound`] if the id is not indexed.
    pub fn delete_note(&mut self, id: &NoteId) -> Result<()> {
        if !self.index.contains_key(id) {
            return Err(Error::NotFound(*id));
        }
        fs::remove_file(self.note_path(id))?;
        sync_dir(&notes_dir(&self.root))?;
        self.index.remove(id);
        Ok(())
    }

    fn check_header_unchanged(&self) -> Result<()> {
        let on_disk = read_regular(&header_path(&self.root), 4096)?;
        if on_disk == self.vault.header().as_bytes() {
            Ok(())
        } else {
            Err(Error::HeaderChanged)
        }
    }

    fn write_header(&self, header: &VaultHeader) -> Result<()> {
        self.check_header_unchanged()?;
        write_atomic(&meta_dir(&self.root), HEADER_FILE, header.as_bytes())
    }

    /// Change the passphrase (re-wraps the DEK; notes are untouched). Also the way to set a new
    /// passphrase after a recovery-key unlock.
    ///
    /// # Errors
    /// Passphrase too short, KDF parameters out of range, header changed on disk, I/O errors.
    pub fn change_passphrase(&mut self, new_passphrase: &str, params: KdfParams) -> Result<()> {
        let header = self.vault.change_passphrase(new_passphrase, params)?;
        // Prove the new header opens with the new passphrase before replacing the old one.
        let vault = header.unlock_with_passphrase(new_passphrase)?;
        self.write_header(&header)?;
        self.vault = vault;
        Ok(())
    }

    /// Rotate the recovery key; needs the current passphrase. Returns the new key, to be shown
    /// once. The old key stops working on the new header.
    ///
    /// # Errors
    /// Wrong passphrase (unlock error), header changed on disk, I/O errors.
    pub fn rotate_recovery_key(&mut self, passphrase: &str) -> Result<RecoveryKey> {
        let (header, key) = self.vault.rotate_recovery_key(passphrase)?;
        let vault = header.unlock_with_passphrase(passphrase)?;
        self.write_header(&header)?;
        self.vault = vault;
        Ok(key)
    }
}

fn summary_of(note: &Note, revision_id: [u8; 16]) -> NoteSummary {
    NoteSummary {
        id: note.id,
        title: note.title.clone(),
        path: note.path.clone(),
        tags: note.tags.clone(),
        created_at: note.created_at,
        modified_at: note.modified_at,
        revision_id,
    }
}
