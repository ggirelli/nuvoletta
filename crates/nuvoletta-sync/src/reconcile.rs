//! Classification and automatic resolution per note (section 7.1 and 7.2), and the two-phase
//! application of changes to the vault: plan in memory, persist, then write the vault. A crash
//! between persisting and writing is repaired by the next scan (the vault revision is a known
//! vertex, so it is adopted, and the fast-forward is planned again).

use crate::CrashPoint;
use crate::engine::Syncer;
use crate::error::Result;
use crate::ids::{Hash32, Id16, RevId};
use crate::local::{open_revision, seal_revision};
use crate::merge;
use crate::provider::Provider;
use nuvoletta_core::NoteId;
use nuvoletta_storage::Unlocked;

/// A change to the vault planned by the engine.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    /// Install the cached revision as the note's stored revision.
    Install {
        note: Id16,
        rev: RevId,
        /// The revision the vault held before, to be cached before it is overwritten.
        old_local: Option<RevId>,
    },
    /// Remove the note file (a remote or chosen deletion).
    Remove {
        note: Id16,
        /// The tombstone revision the note moves to.
        rev: RevId,
        old_local: Option<RevId>,
    },
}

impl<P: Provider> Syncer<P> {
    /// Step 6: fast-forward, automatically merge or leave conflicts, then write the vault.
    pub(crate) fn reconcile(&mut self, vault: &mut Unlocked) -> Result<()> {
        let notes: Vec<Id16> = self.state.notes.keys().copied().collect();
        let mut actions = Vec::new();
        for note in notes {
            self.classify_note(vault, note, &mut actions)?;
        }
        self.apply_actions(vault, actions)
    }

    fn classify_note(
        &mut self,
        vault: &Unlocked,
        note: Id16,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        let (local, heads, orphan, attention, head_is_delete, head_has_rev) = {
            let rec = &self.state.notes[&note];
            if rec.graph.vertices.is_empty() {
                return Ok(());
            }
            let heads = rec.graph.heads();
            let single = (heads.len() == 1).then(|| heads[0]);
            (
                rec.local_rev,
                heads,
                rec.local_rev
                    .is_some_and(|l| !rec.graph.vertices.contains_key(&l)),
                rec.needs_attention,
                single.is_some_and(|h| rec.graph.vertices[&h].is_delete()),
                single.is_some_and(|h| self.state.has_rev(&h)),
            )
        };
        if orphan {
            // The local content was introduced by entries a fork freeze removed: keep it,
            // change nothing automatically (section 10.4 step 1).
            if !attention {
                self.state
                    .notes
                    .get_mut(&note)
                    .expect("listed")
                    .needs_attention = true;
                self.state.touch_notes();
            }
            return Ok(());
        }
        if attention {
            self.state
                .notes
                .get_mut(&note)
                .expect("listed")
                .needs_attention = false;
            self.state.touch_notes();
        }
        if heads.len() == 1 {
            let h = heads[0];
            if local == Some(h) {
                return Ok(());
            }
            if head_is_delete {
                actions.push(Action::Remove {
                    note,
                    rev: h,
                    old_local: local,
                });
            } else if head_has_rev {
                actions.push(Action::Install {
                    note,
                    rev: h,
                    old_local: local,
                });
            } else {
                return Ok(()); // dangling: the object has not arrived, retried next cycle
            }
            self.report.fast_forwards += 1;
            self.state.touch_notes();
            return Ok(());
        }
        self.try_auto_merge(vault, note, &heads, local, actions)
    }

    /// Section 7.2: exactly two content heads, either identical in content (a designated device
    /// writes the merge, so concurrent merges cannot ping-pong) or differing from their unique
    /// common ancestor in disjoint fields.
    fn try_auto_merge(
        &mut self,
        vault: &Unlocked,
        note: Id16,
        heads: &[RevId],
        local: Option<RevId>,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        if heads.len() != 2 {
            return Ok(());
        }
        let (a, b) = (heads[0], heads[1]);
        let graph = self.state.notes[&note].graph.clone();
        let (va, vb) = (&graph.vertices[&a], &graph.vertices[&b]);
        if !(va.is_put() && vb.is_put()) {
            return Ok(());
        }
        let (Some(ba), Some(bb)) = (self.state.get_rev(&a)?, self.state.get_rev(&b)?) else {
            return Ok(());
        };
        let keys = vault.keys();
        let (Ok(pa), Ok(pb)) = (
            open_revision(keys, &ba, &note, Some(&a)),
            open_revision(keys, &bb, &note, Some(&b)),
        ) else {
            return Ok(());
        };
        let merged = if merge::same_content(&pa, &pb) {
            let designated = [va, vb]
                .iter()
                .filter_map(|v| v.origins.first().map(|o| o.device))
                .min();
            if designated != Some(self.state.device_id) {
                return Ok(());
            }
            merge::identical(&pa, &pb)
        } else {
            let Some(c) = graph.common_base(&a, &b) else {
                return Ok(());
            };
            let Some(vc) = graph.vertices.get(&c) else {
                return Ok(());
            };
            let (true, Some(ct)) = (vc.is_put(), vc.ct_hash) else {
                return Ok(());
            };
            if !self.state.has_rev(&c) && !self.fetch_object(vault, &note, &c, &ct, &[])? {
                return Ok(());
            }
            let Some(bc) = self.state.get_rev(&c)? else {
                return Ok(());
            };
            let Ok(pc) = open_revision(keys, &bc, &note, Some(&c)) else {
                return Ok(());
            };
            merge::three_way(&pc, &pa, &pb)
        };
        let Some(m) = merged else { return Ok(()) };
        let (rev, bytes) = seal_revision(keys, &m)?;
        self.state.put_rev(&rev, &bytes)?;
        self.add_intent(
            note,
            crate::log::KIND_PUT,
            rev,
            Some(Hash32::of(&bytes)),
            vec![a, b],
        );
        actions.push(Action::Install {
            note,
            rev,
            old_local: local,
        });
        self.report.auto_merges += 1;
        Ok(())
    }

    /// Persist the plan, then write the vault.
    pub(crate) fn apply_actions(
        &mut self,
        vault: &mut Unlocked,
        actions: Vec<Action>,
    ) -> Result<()> {
        if actions.is_empty() {
            return Ok(());
        }
        for action in &actions {
            let (note, target) = match action {
                Action::Install { note, rev, .. } | Action::Remove { note, rev, .. } => {
                    (*note, *rev)
                }
            };
            if let Some(rec) = self.state.notes.get_mut(&note) {
                rec.applying = Some(target);
            }
        }
        self.state.touch_notes();
        self.state.save(vault.keys())?;
        self.crash_at(CrashPoint::AfterPlanSaved)?;
        for action in actions {
            let (note, target, old_local) = match &action {
                Action::Install {
                    note,
                    rev,
                    old_local,
                }
                | Action::Remove {
                    note,
                    rev,
                    old_local,
                } => (*note, *rev, *old_local),
            };
            let nid = NoteId::from_bytes(note.0);
            // Never lose the content being replaced: it must be in the cache first.
            if let (Some(old), Some(s)) = (old_local, vault.summary(&nid))
                && s.revision_id == old.0
                && !self.state.has_rev(&old)
            {
                let bytes = vault.read_revision_bytes(&nid)?;
                self.state.put_rev(&old, &bytes)?;
            }
            match action {
                Action::Install { rev, .. } => {
                    let Some(bytes) = self.state.get_rev(&rev)? else {
                        continue;
                    };
                    vault.install_revision(&bytes)?;
                }
                Action::Remove { .. } => {
                    if vault.summary(&nid).is_some() {
                        vault.delete_note(&nid)?;
                    }
                }
            }
            if let Some(rec) = self.state.notes.get_mut(&note) {
                rec.local_rev = Some(target);
                rec.base = Some(target);
                rec.applying = None;
            }
        }
        self.state.touch_notes();
        Ok(())
    }
}
