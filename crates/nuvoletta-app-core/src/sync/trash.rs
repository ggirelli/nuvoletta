//! "Recently deleted": deleted notes of a synced vault and their restoration.
//!
//! A deletion is a tombstone in the sync log; the engine keeps the last content (the tombstone's
//! parents) under its garbage-collection rules. There is no purge: retention is governed by
//! those rules alone. A vault whose sync was never configured has no tombstones and no kept
//! content (a plain delete unlinks the file), so it has nothing to list.

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
}

impl App {
    /// Deleted notes, most recently deleted first. Empty when sync was never configured.
    /// Deletions made since the last cycle are recorded first, so they show up at once.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle; locked or no vault.
    pub fn recently_deleted(&mut self) -> Result<Vec<DeletedItem>> {
        let s = self.session_mut()?;
        let Some(vault) = s.vault.as_mut() else {
            return Err(Error::SyncBusy);
        };
        let Some(syncer) = s.sync.syncer.as_deref_mut() else {
            return Ok(Vec::new());
        };
        syncer
            .record_local_changes(vault)
            .map_err(|e| Error::Sync(describe_resolution_error(&e)))?;
        Ok(syncer
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
                }
            })
            .collect())
    }

    /// Restore a deleted note. This writes a new version whose parent is the deletion, so every
    /// device ends up with the note.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle; [`Error::Sync`] if the note is not deleted or its
    /// content has not arrived on this device yet (sync, then retry).
    pub fn restore_note(&mut self, note: NoteId) -> Result<()> {
        let now = self.clock_now();
        let s = self.session_mut()?;
        let before = s.list();
        let (Some(vault), Some(syncer)) = (s.vault.as_mut(), s.sync.syncer.as_deref_mut()) else {
            return Err(if s.vault.is_none() {
                Error::SyncBusy
            } else {
                Error::SyncNotConfigured
            });
        };
        let map = |e: nuvoletta_sync::Error| Error::Sync(describe_resolution_error(&e));
        syncer.record_local_changes(vault).map_err(map)?;
        let result = syncer.restore_note(vault, &note);
        s.sync.changed_at = Some(now);
        s.reconcile_with_vault(&before);
        result.map_err(map)
    }
}
