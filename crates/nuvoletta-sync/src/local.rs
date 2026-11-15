//! Local edits: turning changes in the vault into journal intents (section 6.2), and the
//! helpers that read and build revision envelopes.
//!
//! The vault (`nuvoletta-storage`) knows nothing about sync. Instead of hooking every write, the
//! engine scans the vault: a note whose stored revision id is not the one sync last saw is a
//! local edit; a synced note that disappeared is a local delete. This also repairs everything
//! after a crash, as section 6.1 prescribes ("the next scan finds notes whose local revision is
//! not in the own log and re-emits entries").

use crate::engine::Syncer;
use crate::error::{Error, Result};
use crate::graph::{Insert, Origin, Vertex};
use crate::ids::{Hash32, Id16, RevId};
use crate::log::{KIND_DELETE, KIND_PUT};
use crate::provider::Provider;
use crate::state::{Intent, NoteRec};
use nuvoletta_core::{NoteId, now_ms};
use nuvoletta_crypto::{NotePayload, ObjType, ObjectMeta, Subkeys, open, seal};
use nuvoletta_storage::layout::parse_note_file_name;
use nuvoletta_storage::{IssueKind, Unlocked};
use std::collections::BTreeSet;

/// Decrypt a type 1 envelope and check it belongs to `note` and (if given) is revision `rev`.
pub(crate) fn open_revision(
    keys: &Subkeys,
    bytes: &[u8],
    note: &Id16,
    rev: Option<&RevId>,
) -> Result<NotePayload> {
    let opened = open(keys, bytes)?;
    if opened.meta.obj_type != ObjType::NoteRevision
        || opened.meta.scope_id != note.0
        || rev.is_some_and(|r| opened.meta.object_id != r.0)
    {
        return Err(Error::Crypto(nuvoletta_crypto::Error::ContextMismatch));
    }
    let payload = NotePayload::decode(&opened.plaintext)?;
    payload.check_scope(&note.0)?;
    Ok(payload)
}

/// Seal a payload as a new revision of its note: returns the new revision id and envelope.
pub(crate) fn seal_revision(keys: &Subkeys, payload: &NotePayload) -> Result<(RevId, Vec<u8>)> {
    let meta = ObjectMeta::new(ObjType::NoteRevision, payload.note_id)?;
    let encoded = payload.encode()?;
    let bytes = seal(keys, &meta, &encoded)?;
    Ok((Id16(meta.object_id), bytes))
}

impl<P: Provider> Syncer<P> {
    /// Re-insert the vertices of own journal intents (after the notes state was rebuilt).
    fn ensure_intent_vertices(&mut self) {
        let me = self.state.device_id;
        let intents = self.state.journal.intents.clone();
        for i in intents {
            if self.rev_note.contains_key(&i.rev) {
                continue;
            }
            let rec = self.state.notes.entry(i.note).or_default();
            rec.graph.insert(Vertex {
                rev: i.rev,
                kind: i.kind,
                parents: i.parents.clone(),
                ct_hash: i.ct_hash,
                origins: vec![Origin { device: me, seq: 0 }],
                time: i.time,
            });
            self.rev_note.insert(i.rev, i.note);
            self.state.touch_notes();
        }
    }

    /// Scan the vault and queue journal entries for local creates, edits and deletes. Returns
    /// the number of notes that produced an entry. Idempotent: a second scan finds nothing.
    ///
    /// # Errors
    /// Storage or I/O failures.
    pub fn record_local_changes(&mut self, vault: &Unlocked) -> Result<usize> {
        self.ensure_intent_vertices();
        let corrupt: BTreeSet<Id16> = vault
            .issues()
            .iter()
            .filter(|i| matches!(i.kind, IssueKind::Corrupt | IssueKind::Symlink))
            .filter_map(|i| parse_note_file_name(&i.name))
            .map(|n| Id16(*n.as_bytes()))
            .collect();
        let mut created = 0;
        let mut present = BTreeSet::new();
        for s in vault.list() {
            let note = Id16(*s.id.as_bytes());
            present.insert(note);
            let rev = Id16(s.revision_id);
            let known = self.state.notes.get(&note);
            let adopt = match known {
                Some(rec) if rec.local_rev == Some(rev) => continue,
                Some(rec) => rec.graph.vertices.contains_key(&rev),
                None => false,
            };
            if adopt {
                let rec = self.state.notes.get_mut(&note).expect("checked above");
                rec.local_rev = Some(rev);
                rec.base = Some(rev);
                rec.edit_base = None;
                if rec.applying == Some(rev) {
                    rec.applying = None;
                }
                self.state.touch_notes();
                continue;
            }
            self.queue_local_put(vault, &s.id, rev)?;
            created += 1;
        }
        let gone: Vec<Id16> = self
            .state
            .notes
            .iter()
            .filter(|(n, rec)| {
                !present.contains(*n)
                    && !corrupt.contains(*n)
                    && rec.applying.is_none()
                    && rec
                        .local_rev
                        .is_some_and(|l| rec.graph.vertices.get(&l).is_some_and(|v| v.is_put()))
            })
            .map(|(n, _)| *n)
            .collect();
        for note in gone {
            self.queue_local_delete(note)?;
            created += 1;
        }
        self.report.intents_created += created;
        if created > 0 {
            self.state.touch_journal();
            self.state.touch_notes();
        }
        Ok(created)
    }

    fn has_vertex(&self, note: &Id16, rev: &RevId) -> bool {
        self.state
            .notes
            .get(note)
            .is_some_and(|r| r.graph.vertices.contains_key(rev))
    }

    /// Tell the engine that the next local save of `note` was typed on top of revision `base`,
    /// which a sync has since replaced in the vault. Call after the save; the new revision is
    /// then queued as a sibling of the synced one and shows as a conflict. Unknown or current
    /// revisions are ignored.
    ///
    /// # Errors
    /// Failure to persist the hint.
    pub fn note_edited_on(
        &mut self,
        vault: &Unlocked,
        note: &NoteId,
        base: [u8; 16],
    ) -> Result<()> {
        let nid = Id16(*note.as_bytes());
        let base = Id16(base);
        if !self.has_vertex(&nid, &base) {
            return Ok(());
        }
        let Some(rec) = self.state.notes.get_mut(&nid) else {
            return Ok(());
        };
        if rec.local_rev == Some(base) {
            return Ok(());
        }
        rec.edit_base = Some(base);
        self.state.touch_notes();
        self.state.save(vault.keys())
    }

    /// Position and remove an unsealed own intent of `note`, with its vertex and cached object.
    ///
    /// Only the intent that is the note's current local revision is collapsed: other intents of
    /// the same note (re-emitted history after a fork) are older revisions peers may build on.
    fn take_unsealed_intent(&mut self, note: &Id16) -> Result<Option<Intent>> {
        let local = self.state.notes.get(note).and_then(|r| r.local_rev);
        let pos = self
            .state
            .journal
            .intents
            .iter()
            .position(|i| i.note == *note && Some(i.rev) == local);
        let Some(pos) = pos else { return Ok(None) };
        let old = self.state.journal.intents.remove(pos);
        if let Some(rec) = self.state.notes.get_mut(note) {
            rec.graph.vertices.remove(&old.rev);
        }
        self.rev_note.remove(&old.rev);
        // Never uploaded as part of any entry: an intermediate local revision (section 6.2).
        // Its object file may already be on the remote (unreferenced orphan, section 9 rule 5).
        self.state.remove_rev(&old.rev)?;
        if old.kind == KIND_PUT {
            // Its object may be on the remote even if the upload was not yet recorded: remember
            // it, garbage collection may delete it after the orphan grace period (and forgets
            // the record if the remote has no such file).
            self.state.pending.orphans.insert(old.rev, now_ms());
            self.state.touch_pending();
        }
        Ok(Some(old))
    }

    fn queue_local_put(&mut self, vault: &Unlocked, note: &NoteId, rev: RevId) -> Result<()> {
        let nid = Id16(*note.as_bytes());
        let bytes = vault.read_revision_bytes(note)?;
        // Validate before queueing (also proves the revision matches the id we think it has).
        open_revision(vault.keys(), &bytes, &nid, Some(&rev))?;
        let hash = Hash32::of(&bytes);
        self.state.put_rev(&rev, &bytes)?;
        let local = self.state.notes.get(&nid).and_then(|r| r.local_rev);
        // An edit made on top of an older revision than the one the vault now holds (the user
        // kept typing while a sync replaced the note) is a sibling of the newer revision, never
        // its silent successor (PLAN 6.1: no silent last-write-wins).
        let edit_base = self
            .state
            .notes
            .get_mut(&nid)
            .and_then(|r| r.edit_base.take())
            .filter(|b| Some(*b) != local && self.has_vertex(&nid, b));
        let parents = match self.take_unsealed_intent(&nid)? {
            Some(old) => old.parents,
            None => edit_base.map_or_else(|| local.into_iter().collect(), |b| vec![b]),
        };
        self.add_intent(nid, KIND_PUT, rev, Some(hash), parents);
        Ok(())
    }

    fn queue_local_delete(&mut self, note: Id16) -> Result<()> {
        let local = self.state.notes.get(&note).and_then(|r| r.local_rev);
        let parents = match self.take_unsealed_intent(&note)? {
            Some(old) if old.parents.is_empty() => {
                // The note was created and deleted before anything was sent: nothing to tell.
                let rec = self.state.notes.entry(note).or_default();
                rec.local_rev = None;
                if rec.graph.vertices.is_empty() {
                    self.state.notes.remove(&note);
                }
                self.state.touch_notes();
                return Ok(());
            }
            Some(old) => old.parents,
            None => local.into_iter().collect(),
        };
        let tomb = Id16::random()?;
        self.add_intent(note, KIND_DELETE, tomb, None, parents);
        Ok(())
    }

    /// Record an own intent: journal entry, graph vertex and `local_rev`.
    pub(crate) fn add_intent(
        &mut self,
        note: Id16,
        kind: u8,
        rev: RevId,
        ct_hash: Option<Hash32>,
        parents: Vec<RevId>,
    ) {
        let me = self.state.device_id;
        let mut parents = parents;
        parents.sort();
        parents.dedup();
        let time = now_ms();
        self.state.journal.intents.push(Intent {
            note,
            kind,
            rev,
            ct_hash,
            parents: parents.clone(),
            time,
            uploaded: kind == KIND_DELETE,
        });
        let rec: &mut NoteRec = self.state.notes.entry(note).or_default();
        let ins = rec.graph.insert(Vertex {
            rev,
            kind,
            parents,
            ct_hash,
            origins: vec![Origin { device: me, seq: 0 }],
            time,
        });
        debug_assert_eq!(ins, Insert::New);
        rec.local_rev = Some(rev);
        self.rev_note.insert(rev, note);
        self.touched.insert(note);
        self.state.touch_journal();
        self.state.touch_notes();
    }
}
