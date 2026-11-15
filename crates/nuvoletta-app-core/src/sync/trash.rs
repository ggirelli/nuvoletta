//! "Recently deleted": deleted notes of this vault and their restoration.
//!
//! Two sources are merged, deduplicated by note id. Notes sync has seen are deleted through a
//! tombstone in the sync log and come back through the engine (a PUT with the tombstone as
//! parent), so every device converges. Notes sync has not seen (no sync, or never scanned) are
//! in the vault's local trash and come back by moving the file back. Retention is automatic
//! (30 days locally, the cleanup rules for synced content); there is no purge action.

use super::{App, Error, Result};
use crate::sync_messages::{describe_resolution_error, short};
use nuvoletta_core::NoteId;

/// A deleted note, for the "Recently deleted" view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedItem {
    /// The note.
    pub note: NoteId,
    /// Title of the last version, if its content is available on this device.
    pub title: Option<String>,
    /// Path of the last version, if available.
    pub path: Option<String>,
    /// Time of the deletion (ms since the epoch, as recorded by the deleting device).
    pub deleted_ms: i64,
    /// Abbreviated id of the device that deleted it.
    pub device: Option<String>,
    /// The content is on this device, so Restore works now; otherwise sync first.
    pub can_restore: bool,
    /// Restore goes through sync (the note is tracked by it) rather than the local trash.
    pub synced: bool,
}

impl App {
    /// Deleted notes, most recently deleted first.
    /// Deletions made since the last cycle are recorded first, so they show up at once.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle; locked or no vault.
    pub fn recently_deleted(&mut self) -> Result<Vec<DeletedItem>> {
        let s = self.session_mut()?;
        let Some(vault) = s.vault.as_mut() else {
            return Err(Error::SyncBusy);
        };
        let _ = vault.purge_expired();
        let mut out: Vec<DeletedItem> = Vec::new();
        if let Some(syncer) = s.sync.syncer.as_deref_mut() {
            syncer
                .record_local_changes(vault)
                .map_err(|e| Error::Sync(describe_resolution_error(&e)))?;
            out = syncer
                .deleted_notes()
                .into_iter()
                .map(|d| {
                    let last = d
                        .content
                        .filter(|_| d.available)
                        .and_then(|c| syncer.read_revision(vault, &d.note, &c).ok());
                    DeletedItem {
                        note: d.note,
                        title: last.as_ref().map(|n| n.title.clone()),
                        path: last.as_ref().map(|n| n.path.as_str().to_owned()),
                        deleted_ms: d.time,
                        device: d.devices.first().map(short),
                        can_restore: d.available,
                        synced: true,
                    }
                })
                .collect();
        }
        for t in vault.list_trash() {
            let tracked = s.sync.syncer.as_deref().is_some_and(|y| y.tracks(&t.id));
            if tracked || out.iter().any(|d| d.note == t.id) {
                continue; // handled (or deliberately hidden) by the sync side
            }
            out.push(DeletedItem {
                note: t.id,
                title: Some(t.summary.title.clone()),
                path: Some(t.summary.path.as_str().to_owned()),
                deleted_ms: t.deleted_ms,
                device: None,
                can_restore: true,
                synced: false,
            });
        }
        out.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms).then(a.note.cmp(&b.note)));
        Ok(out)
    }

    /// Restore a deleted note and return its id. A note sync tracks gets a new version whose
    /// parent is the deletion, so every device ends up with it. Any other note is moved back from
    /// the local trash; if its id is taken by a live note it comes back as a separate copy
    /// (new id, title suffixed " (restored)"), never overwriting anything.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle; [`Error::Sync`] if a tracked note is not deleted or
    /// its content has not arrived on this device yet (sync, then retry); storage errors
    /// (not found) for a note that is not in the trash.
    pub fn restore_note(&mut self, note: NoteId) -> Result<NoteId> {
        let now = self.clock_now();
        let s = self.session_mut()?;
        let before = s.list();
        let Some(vault) = s.vault.as_mut() else {
            return Err(Error::SyncBusy);
        };
        let map = |e: nuvoletta_sync::Error| Error::Sync(describe_resolution_error(&e));
        let mut tracked = false;
        if let Some(syncer) = s.sync.syncer.as_deref_mut() {
            syncer.record_local_changes(vault).map_err(map)?;
            tracked = syncer.tracks(&note);
        }
        let result = if tracked {
            let syncer = s
                .sync
                .syncer
                .as_deref_mut()
                .expect("tracked implies syncer");
            syncer
                .restore_note(vault, &note)
                .map(|()| note)
                .map_err(map)
        } else {
            vault.restore_from_trash(&note).map_err(Error::from)
        };
        s.sync.changed_at = Some(now);
        s.reconcile_with_vault(&before);
        result
    }
}
