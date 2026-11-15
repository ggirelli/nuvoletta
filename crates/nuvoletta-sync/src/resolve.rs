//! Conflict presentation and user resolution (section 7.3). Conflict is state, not extra files:
//! the heads of a note's DAG. Every action writes a new revision whose parents are all current
//! heads, so the conflict disappears on every device without discarding any head's content.

use crate::engine::Syncer;
use crate::error::{Error, Result};
use crate::ids::{DeviceId, Hash32, Id16, RevId};
use crate::local::{open_revision, seal_revision};
use crate::log::{KIND_DELETE, KIND_PUT};
use crate::provider::Provider;
use crate::reconcile::Action;
use nuvoletta_core::{MAX_TITLE, Note, NoteId, NotePath, now_ms};
use nuvoletta_storage::Unlocked;

/// One head of a note, for display and resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadInfo {
    /// Revision id (object id, or tombstone id for a deletion).
    pub rev: RevId,
    /// The head is a deletion.
    pub is_delete: bool,
    /// Devices that introduced it.
    pub devices: Vec<DeviceId>,
    /// Informational time of the introducing entry (ms).
    pub time: i64,
    /// The content is available locally (always true for a deletion).
    pub available: bool,
}

/// A note with two or more heads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The note.
    pub note: NoteId,
    /// Its heads in the cosmetic display order `(time, device, rev)`.
    pub heads: Vec<HeadInfo>,
}

/// Where a note stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteStatus {
    /// Sync knows nothing about this note yet.
    Untracked,
    /// One head, equal to the local revision.
    Synced,
    /// The local revision is not yet confirmed on the remote.
    Pending,
    /// Two or more heads.
    Conflict(Vec<HeadInfo>),
    /// A fork freeze removed the entries behind the local content.
    NeedsAttention,
    /// The only head's content has not arrived (or the vault lags behind it).
    Dangling,
}

fn note_of(payload: &nuvoletta_crypto::NotePayload) -> Result<Note> {
    Ok(Note {
        id: NoteId::from_bytes(payload.note_id),
        title: payload.title.clone(),
        path: NotePath::parse(&payload.path).map_err(nuvoletta_storage::Error::from)?,
        tags: payload.tags.clone(),
        created_at: payload.created_at,
        modified_at: payload.modified_at,
        body: payload.body.clone(),
    })
}

impl<P: Provider> Syncer<P> {
    fn head_infos(&self, note: &Id16) -> Vec<HeadInfo> {
        let Some(rec) = self.state.notes.get(note) else {
            return Vec::new();
        };
        rec.graph
            .heads_display_order()
            .into_iter()
            .map(|rev| {
                let v = &rec.graph.vertices[&rev];
                HeadInfo {
                    rev,
                    is_delete: v.is_delete(),
                    devices: v.origins.iter().map(|o| o.device).collect(),
                    time: v.time,
                    available: v.is_delete() || self.state.has_rev(&rev),
                }
            })
            .collect()
    }

    /// The current heads of a note (empty if unknown), in revision-id order.
    #[must_use]
    pub fn heads(&self, note: &NoteId) -> Vec<RevId> {
        self.state
            .notes
            .get(&Id16(*note.as_bytes()))
            .map(|r| r.graph.heads())
            .unwrap_or_default()
    }

    /// Every revision (content or tombstone) in the version graphs, as `(note, rev, is_delete)`.
    /// Diagnostics and tests: nothing in here is ever removed by the engine.
    #[must_use]
    pub fn known_revisions(&self) -> Vec<(NoteId, RevId, bool)> {
        self.state
            .notes
            .iter()
            .flat_map(|(n, rec)| {
                rec.graph
                    .vertices
                    .values()
                    .map(move |v| (NoteId::from_bytes(n.0), v.rev, v.is_delete()))
            })
            .collect()
    }

    /// Whether the content of a revision is cached locally (`revs/`).
    #[must_use]
    pub fn has_cached_revision(&self, rev: &RevId) -> bool {
        self.state.has_rev(rev)
    }

    /// Per-device log knowledge: `(device, verified seq, frozen at)` (diagnostics).
    #[must_use]
    pub fn peers(&self) -> Vec<(DeviceId, u64, Option<u64>)> {
        self.state
            .peers
            .iter()
            .map(|(d, p)| (*d, p.seq(), p.fork.as_ref().map(|f| f.seq)))
            .collect()
    }

    /// All notes with two or more heads.
    #[must_use]
    pub fn conflicts(&self) -> Vec<Conflict> {
        self.state
            .notes
            .iter()
            .filter(|(_, rec)| rec.graph.heads().len() > 1)
            .map(|(n, _)| Conflict {
                note: NoteId::from_bytes(n.0),
                heads: self.head_infos(n),
            })
            .collect()
    }

    /// Where a note stands.
    #[must_use]
    pub fn note_status(&self, note: &NoteId) -> NoteStatus {
        let id = Id16(*note.as_bytes());
        let Some(rec) = self.state.notes.get(&id) else {
            return NoteStatus::Untracked;
        };
        if rec.needs_attention {
            return NoteStatus::NeedsAttention;
        }
        let heads = rec.graph.heads();
        match heads.as_slice() {
            [] => NoteStatus::Untracked,
            [h] => {
                if rec.local_rev != Some(*h) {
                    NoteStatus::Dangling
                } else if self.state.journal.intents.iter().any(|i| i.rev == *h)
                    || self.state.journal.sealed.is_some() && rec.base != Some(*h)
                {
                    NoteStatus::Pending
                } else {
                    NoteStatus::Synced
                }
            }
            _ => NoteStatus::Conflict(self.head_infos(&id)),
        }
    }

    /// Decrypt a head (or any cached revision) of a note for viewing and diffing.
    ///
    /// # Errors
    /// [`Error::RevisionUnavailable`] if the content is not cached (not yet fetched).
    pub fn read_revision(&self, vault: &Unlocked, note: &NoteId, rev: &RevId) -> Result<Note> {
        let id = Id16(*note.as_bytes());
        let bytes = self.state.get_rev(rev)?.ok_or(Error::RevisionUnavailable)?;
        note_of(&open_revision(vault.keys(), &bytes, &id, Some(rev))?)
    }

    fn conflict_heads(&self, note: &NoteId) -> Result<(Id16, Vec<RevId>)> {
        let id = Id16(*note.as_bytes());
        let rec = self.state.notes.get(&id).ok_or(Error::NoteNotFound)?;
        let heads = rec.graph.heads();
        if heads.len() < 2 {
            return Err(Error::InvalidResolution("the note is not in conflict"));
        }
        Ok((id, heads))
    }

    /// Plan the resolution that makes `chosen` win: a PUT with the chosen content (a new
    /// revision) or, for a tombstone, a DELETE, with all current heads as parents.
    fn plan_choose(&mut self, vault: &Unlocked, note: &NoteId, chosen: &RevId) -> Result<Action> {
        let (id, heads) = self.conflict_heads(note)?;
        if !heads.contains(chosen) {
            return Err(Error::InvalidResolution("not a current head"));
        }
        let local = self.state.notes[&id].local_rev;
        let is_delete = self.state.notes[&id].graph.vertices[chosen].is_delete();
        if is_delete {
            let tomb = Id16::random()?;
            self.add_intent(id, KIND_DELETE, tomb, None, heads);
            return Ok(Action::Remove {
                note: id,
                rev: tomb,
                old_local: local,
            });
        }
        let bytes = self
            .state
            .get_rev(chosen)?
            .ok_or(Error::RevisionUnavailable)?;
        let payload = open_revision(vault.keys(), &bytes, &id, Some(chosen))?;
        let (rev, new_bytes) = seal_revision(vault.keys(), &payload)?;
        self.state.put_rev(&rev, &new_bytes)?;
        self.add_intent(id, KIND_PUT, rev, Some(Hash32::of(&new_bytes)), heads);
        Ok(Action::Install {
            note: id,
            rev,
            old_local: local,
        })
    }

    /// Resolve by choosing a head: the note becomes that head's content (or deleted, if the
    /// chosen head is a tombstone). All heads stay retained as parents.
    ///
    /// # Errors
    /// [`Error::InvalidResolution`] if the note is not in conflict or `chosen` is not a head;
    /// [`Error::RevisionUnavailable`] if its content has not arrived.
    pub fn resolve_choose_head(
        &mut self,
        vault: &mut Unlocked,
        note: &NoteId,
        chosen: &RevId,
    ) -> Result<()> {
        let action = self.plan_choose(vault, note, chosen)?;
        self.apply_actions(vault, vec![action])
    }

    /// Delete-versus-edit, "keep": the single content head wins over the tombstone(s).
    ///
    /// # Errors
    /// [`Error::InvalidResolution`] unless there is exactly one content head and a tombstone.
    pub fn resolve_keep(&mut self, vault: &mut Unlocked, note: &NoteId) -> Result<()> {
        let (id, heads) = self.conflict_heads(note)?;
        let g = &self.state.notes[&id].graph;
        let puts: Vec<RevId> = heads
            .iter()
            .copied()
            .filter(|h| g.vertices[h].is_put())
            .collect();
        if puts.len() != 1 || puts.len() == heads.len() {
            return Err(Error::InvalidResolution(
                "keep needs exactly one content head and a deletion",
            ));
        }
        self.resolve_choose_head(vault, note, &puts[0])
    }

    /// Delete-versus-edit, "delete": a new tombstone with all heads as parents. The edited
    /// revision stays retained (it is a parent), so the user can still undo.
    ///
    /// # Errors
    /// [`Error::InvalidResolution`] unless one head is a tombstone.
    pub fn resolve_delete(&mut self, vault: &mut Unlocked, note: &NoteId) -> Result<()> {
        let (id, heads) = self.conflict_heads(note)?;
        let g = &self.state.notes[&id].graph;
        let Some(tomb) = heads.iter().copied().find(|h| g.vertices[h].is_delete()) else {
            return Err(Error::InvalidResolution("no deletion among the heads"));
        };
        self.resolve_choose_head(vault, note, &tomb)
    }

    /// "Keep both": the original note continues with `keep`, and `copy`'s content becomes a new
    /// note titled "... (conflict copy)". Only the resolving device creates the copy. Returns
    /// the new note's id.
    ///
    /// # Errors
    /// [`Error::InvalidResolution`] unless both are distinct content heads;
    /// [`Error::RevisionUnavailable`] if either has not arrived.
    pub fn resolve_keep_both(
        &mut self,
        vault: &mut Unlocked,
        note: &NoteId,
        keep: &RevId,
        copy: &RevId,
    ) -> Result<NoteId> {
        let (id, heads) = self.conflict_heads(note)?;
        let g = &self.state.notes[&id].graph;
        let content = |r: &RevId| heads.contains(r) && g.vertices[r].is_put();
        if keep == copy || !content(keep) || !content(copy) {
            return Err(Error::InvalidResolution(
                "keep and copy must be distinct content heads",
            ));
        }
        let bytes = self
            .state
            .get_rev(copy)?
            .ok_or(Error::RevisionUnavailable)?;
        let mut payload = open_revision(vault.keys(), &bytes, &id, Some(copy))?;
        if self.state.get_rev(keep)?.is_none() {
            return Err(Error::RevisionUnavailable);
        }
        let new_note = NoteId::generate().map_err(nuvoletta_storage::Error::from)?;
        payload.note_id = *new_note.as_bytes();
        let suffix = " (conflict copy)";
        let mut title = payload.title.clone();
        while title.len() + suffix.len() > MAX_TITLE {
            title.pop();
        }
        payload.title = format!("{title}{suffix}");
        payload.modified_at = now_ms();
        let (rev, new_bytes) = seal_revision(vault.keys(), &payload)?;
        let new_id = Id16(*new_note.as_bytes());
        self.state.put_rev(&rev, &new_bytes)?;
        let keep_action = self.plan_choose(vault, note, keep)?;
        self.add_intent(
            new_id,
            KIND_PUT,
            rev,
            Some(Hash32::of(&new_bytes)),
            Vec::new(),
        );
        let copy_action = Action::Install {
            note: new_id,
            rev,
            old_local: None,
        };
        self.apply_actions(vault, vec![keep_action, copy_action])?;
        Ok(new_note)
    }
}
