//! Deleted notes and their restoration (`SYNC_PROTOCOL.md` sections 5.3 and 7.3). A deleted note
//! is a note whose only head is a tombstone. Restoring writes a PUT of the last content with the
//! tombstone as its parent, the same mechanism as "keep" in delete-versus-edit, so every device
//! converges on the restored note. Retention is whatever the garbage-collection rules keep
//! (section 9: the direct parents of a tombstone head are always kept).

use crate::engine::Syncer;
use crate::error::{Error, Result};
use crate::ids::{DeviceId, Hash32, Id16, RevId};
use crate::local::{open_revision, seal_revision};
use crate::log::KIND_PUT;
use crate::provider::Provider;
use crate::reconcile::Action;
use nuvoletta_core::NoteId;
use nuvoletta_storage::Unlocked;

/// A note whose only head is a tombstone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedNote {
    /// The note.
    pub note: NoteId,
    /// The tombstone.
    pub tombstone: RevId,
    /// Informational time of the deletion entry (ms).
    pub time: i64,
    /// Devices that introduced the tombstone.
    pub devices: Vec<DeviceId>,
    /// The last content revision (a parent of the tombstone), if one is known.
    pub content: Option<RevId>,
    /// That content is cached locally, so the note can be restored now.
    pub available: bool,
}

impl<P: Provider> Syncer<P> {
    /// Sync has seen this note (it has a history in the log), so a deletion must be undone
    /// through [`Syncer::restore_note`], never by putting a file back.
    #[must_use]
    pub fn tracks(&self, note: &NoteId) -> bool {
        self.state.notes.contains_key(&Id16(*note.as_bytes()))
    }

    /// Deleted notes, newest deletion first. Notes in conflict or needing attention are not
    /// listed (they are handled as conflicts).
    #[must_use]
    pub fn deleted_notes(&self) -> Vec<DeletedNote> {
        let mut out: Vec<DeletedNote> = self
            .state
            .notes
            .iter()
            .filter(|(_, rec)| !rec.needs_attention)
            .filter_map(|(n, rec)| {
                let heads = rec.graph.heads();
                let [tomb] = heads.as_slice() else {
                    return None;
                };
                let v = &rec.graph.vertices[tomb];
                if !v.is_delete() {
                    return None;
                }
                // Prefer a cached parent, then the latest one.
                let content = v
                    .parents
                    .iter()
                    .filter_map(|p| rec.graph.vertices.get(p))
                    .filter(|p| p.is_put())
                    .max_by_key(|p| (self.state.has_rev(&p.rev), p.time, p.rev))
                    .map(|p| p.rev);
                Some(DeletedNote {
                    note: NoteId::from_bytes(n.0),
                    tombstone: *tomb,
                    time: v.time,
                    devices: v.origins.iter().map(|o| o.device).collect(),
                    available: content.is_some_and(|c| self.state.has_rev(&c)),
                    content,
                })
            })
            .collect();
        out.sort_by(|a, b| b.time.cmp(&a.time).then(a.note.cmp(&b.note)));
        out
    }

    /// Restore a deleted note: a new PUT of its last content whose parent is the tombstone.
    ///
    /// # Errors
    /// [`Error::InvalidResolution`] if the note is not currently deleted;
    /// [`Error::RevisionUnavailable`] if its last content is not cached (sync, then retry).
    pub fn restore_note(&mut self, vault: &mut Unlocked, note: &NoteId) -> Result<()> {
        let d = self
            .deleted_notes()
            .into_iter()
            .find(|d| d.note == *note)
            .ok_or(Error::InvalidResolution("the note is not deleted"))?;
        let id = Id16(*note.as_bytes());
        let content = d.content.ok_or(Error::RevisionUnavailable)?;
        let bytes = self
            .state
            .get_rev(&content)?
            .ok_or(Error::RevisionUnavailable)?;
        let payload = open_revision(vault.keys(), &bytes, &id, Some(&content))?;
        let (rev, new_bytes) = seal_revision(vault.keys(), &payload)?;
        self.state.put_rev(&rev, &new_bytes)?;
        let local = self.state.notes[&id].local_rev;
        self.add_intent(
            id,
            KIND_PUT,
            rev,
            Some(Hash32::of(&new_bytes)),
            vec![d.tombstone],
        );
        self.apply_actions(
            vault,
            vec![Action::Install {
                note: id,
                rev,
                old_local: local,
            }],
        )
    }
}
