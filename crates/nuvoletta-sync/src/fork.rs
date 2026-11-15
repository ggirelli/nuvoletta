//! Fork handling (section 10.4): detect and freeze a device id that was used twice, and recover
//! this device when the forked id is its own.
//!
//! Freezing depends only on the fork point, so every device that has seen both branches computes
//! the same graph. Nothing is deleted: the frozen entries' objects stay retained, the local
//! content they installed stays in the vault, and the affected notes are marked "needs
//! attention".

use crate::engine::{Syncer, Warning};
use crate::error::Result;
use crate::graph::{Origin, Vertex};
use crate::ids::{DeviceId, Hash32, Id16};
use crate::log::{ParsedEntry, SegmentError, open_segment};
use crate::provider::Provider;
use crate::state::{ForkRec, Intent};
use nuvoletta_core::now_ms;
use nuvoletta_storage::Unlocked;

impl<P: Provider> Syncer<P> {
    /// Entry point for a detected fork of `device` at `seq` (first sequence number with two
    /// different entries, or the last one both branches may still share plus one).
    pub(crate) fn on_fork(
        &mut self,
        vault: &Unlocked,
        device: DeviceId,
        seq: u64,
        evidence: Vec<Hash32>,
    ) -> Result<()> {
        if device == self.state.device_id {
            // Entries of a sealed segment that never became visible were not published: the
            // other branch simply won the name (create-only). Nobody saw our branch.
            let unpublished = self
                .state
                .journal
                .sealed
                .as_ref()
                .is_some_and(|s| seq >= s.first_seq);
            return self.recover_own_fork(vault, seq, unpublished);
        }
        self.freeze_peer(device, seq, evidence);
        Ok(())
    }

    /// Freeze a foreign device's stream below `seq` for every reader (section 10.4 step 1).
    pub(crate) fn freeze_peer(&mut self, device: DeviceId, seq: u64, evidence: Vec<Hash32>) {
        let seq = seq.max(1);
        let peer = self.state.peers.entry(device).or_default();
        if peer.fork.as_ref().is_some_and(|f| f.seq <= seq) {
            return; // already frozen at or before this point
        }
        let keep = usize::try_from(seq - 1).unwrap_or(usize::MAX);
        peer.hashes.truncate(keep);
        let limit = seq - 1;
        peer.segments.retain(|first, _| *first <= limit);
        for last in peer.segments.values_mut() {
            *last = (*last).min(limit);
        }
        peer.fork = Some(ForkRec {
            seq,
            hashes: evidence,
        });
        self.state.touch_peers();
        self.drop_origins(&device, seq, true);
        self.report
            .warnings
            .push(Warning::DeviceFork { device, seq });
    }

    /// Remove all introductions by `device` at or after `seq` from every graph.
    fn drop_origins(&mut self, device: &DeviceId, seq: u64, mark_attention: bool) {
        let notes: Vec<Id16> = self.state.notes.keys().copied().collect();
        for note in notes {
            let rec = self.state.notes.get_mut(&note).expect("listed");
            let removed = rec.graph.remove_origins(device, seq);
            if removed.is_empty() {
                continue;
            }
            for r in &removed {
                self.rev_note.remove(r);
            }
            if mark_attention && rec.local_rev.is_some_and(|l| removed.contains(&l)) {
                rec.needs_attention = true;
            }
            self.touched.insert(note);
            self.state.touch_notes();
        }
    }

    /// The own device id was used twice (or the own log disagrees with the journal): stop
    /// writing to it, switch to a new id and re-emit, under the new log, every revision that is
    /// not reachable from the frozen prefix, with the original `rev_id`, `ct_hash` and
    /// `parents` (section 10.4 step 3). `fork_seq` is the first sequence number that is not
    /// shared by both branches. If `unpublished`, the diverging entries were never visible to
    /// anyone (our sealed segment lost the race for its name): the other branch is then simply
    /// the history of the old id, read like any other device's, not a fork other devices see,
    /// so the old id is not frozen. Otherwise both branches exist on the remote and the old id
    /// is frozen at the fork point for everyone.
    pub(crate) fn recover_own_fork(
        &mut self,
        vault: &Unlocked,
        fork_seq: u64,
        unpublished: bool,
    ) -> Result<()> {
        let old = self.state.device_id;
        let fork_seq = fork_seq.max(1);
        // 1. Which vertices exist only through own entries at or after the fork point?
        let mut reemit: Vec<(Id16, Vertex)> = Vec::new();
        for (note, rec) in &self.state.notes {
            for v in rec.graph.vertices.values() {
                let own_late = !v.origins.is_empty()
                    && v.origins
                        .iter()
                        .all(|o| o.device == old && o.seq >= fork_seq);
                if own_late {
                    reemit.push((*note, v.clone()));
                }
            }
        }
        // 2. Freeze the old id at the fork point; its prefix stays valid history.
        let peer = self.state.peers.entry(old).or_default();
        let keep = usize::try_from(fork_seq - 1).unwrap_or(usize::MAX);
        let evidence = peer.hashes.get(keep).copied().into_iter().collect();
        peer.hashes.truncate(keep);
        let limit = fork_seq - 1;
        peer.segments.retain(|first, _| *first <= limit);
        for last in peer.segments.values_mut() {
            *last = (*last).min(limit);
        }
        if !unpublished {
            peer.fork = Some(ForkRec {
                seq: fork_seq,
                hashes: evidence,
            });
        }
        self.drop_origins(&old, fork_seq, false);
        // 3. Entries of an unconfirmed sealed segment that were not covered above (the chain
        // was lost rather than forked) become intents again.
        if let Some(sealed) = self.state.journal.sealed.take() {
            if let Ok(seg) = open_segment(vault.keys(), &sealed.bytes) {
                for pe in &seg.entries {
                    if let Some(i) = intent_of(pe)
                        && !reemit.iter().any(|(_, v)| v.rev == i.rev)
                        && !self.state.journal.intents.iter().any(|x| x.rev == i.rev)
                    {
                        let v = vertex_of(&i, old);
                        reemit.push((i.note, v));
                    }
                }
            } else {
                debug_assert!(matches!(
                    open_segment(vault.keys(), &sealed.bytes),
                    Err(SegmentError::Unreadable | SegmentError::Malformed(_))
                ));
            }
        }
        // 4. New identity, re-emission intents first (chronological), then unsent ones.
        let new = Id16::random()?;
        reemit.sort_by_key(|(_, v)| (v.time, v.rev));
        let mut intents: Vec<Intent> = reemit
            .iter()
            .map(|(note, v)| Intent {
                note: *note,
                kind: v.kind,
                rev: v.rev,
                ct_hash: v.ct_hash,
                parents: v.parents.clone(),
                time: v.time,
                uploaded: v.is_delete(),
            })
            .collect();
        intents.append(&mut self.state.journal.intents);
        self.state.journal.intents = intents;
        for (note, v) in reemit {
            let rec = self.state.notes.entry(note).or_default();
            let mut v = v;
            v.origins = vec![Origin {
                device: new,
                seq: 0,
            }];
            rec.graph.insert(v.clone());
            self.rev_note.insert(v.rev, note);
        }
        // The unsent intents' vertices carry the old id as origin; point them at the new one.
        for rec in self.state.notes.values_mut() {
            for v in rec.graph.vertices.values_mut() {
                for o in &mut v.origins {
                    if o.device == old && o.seq == 0 {
                        o.device = new;
                    }
                }
            }
            if rec
                .local_rev
                .is_some_and(|l| rec.graph.vertices.contains_key(&l))
            {
                rec.needs_attention = false;
            }
        }
        let j = &mut self.state.journal;
        j.device = Some(new);
        j.own_len = 0;
        j.last_seen.clear();
        j.foreign_since_checkpoint = false;
        j.last_checkpoint_ms = 0;
        self.state.peers.entry(new).or_default().first_seen = now_ms();
        self.state.touch_journal();
        self.state.touch_notes();
        self.state.touch_peers();
        // State first, id file last: `Syncer::open` repairs the file from the journal.
        self.state.device_id = new;
        self.state.save(vault.keys())?;
        self.state.set_device_id(new)?;
        self.report
            .warnings
            .push(Warning::OwnForkRecovered { old, new });
        Ok(())
    }

    /// If the own chain is not what the journal says (state lost or half-written), continue
    /// under a new id; the old log is then read like any other device's, from sequence 1.
    pub(crate) fn ensure_own_chain(&mut self, vault: &Unlocked) -> Result<()> {
        let me = self.state.device_id;
        let have = self.state.peers.get(&me).map_or(0, |p| p.seq());
        if self.state.journal.own_len == have {
            return Ok(());
        }
        // Keep nothing of the old chain in `peers`: it will be re-read as a foreign device.
        self.state.peers.remove(&me);
        self.recover_own_fork_lost(vault)
    }

    fn recover_own_fork_lost(&mut self, vault: &Unlocked) -> Result<()> {
        let old = self.state.device_id;
        // Entries of an unconfirmed segment become intents again.
        let mut reemit: Vec<Intent> = Vec::new();
        if let Some(sealed) = self.state.journal.sealed.take()
            && let Ok(seg) = open_segment(vault.keys(), &sealed.bytes)
        {
            for pe in &seg.entries {
                if let Some(i) = intent_of(pe) {
                    reemit.push(i);
                }
            }
        }
        reemit.append(&mut self.state.journal.intents);
        self.state.journal.intents = reemit;
        let new = Id16::random()?;
        let j = &mut self.state.journal;
        j.device = Some(new);
        j.own_len = 0;
        j.last_seen.clear();
        j.foreign_since_checkpoint = false;
        j.last_checkpoint_ms = 0;
        for rec in self.state.notes.values_mut() {
            for v in rec.graph.vertices.values_mut() {
                for o in &mut v.origins {
                    if o.device == old && o.seq == 0 {
                        o.device = new;
                    }
                }
            }
        }
        self.state.touch_journal();
        self.state.touch_notes();
        self.state.touch_peers();
        self.state.device_id = new;
        self.state.save(vault.keys())?;
        self.state.set_device_id(new)?;
        self.report
            .warnings
            .push(Warning::OwnForkRecovered { old, new });
        Ok(())
    }
}

fn intent_of(pe: &ParsedEntry) -> Option<Intent> {
    let e = &pe.entry;
    Some(Intent {
        note: e.note_id?,
        kind: e.kind,
        rev: e.rev_id?,
        ct_hash: e.ct_hash,
        parents: e.parents_sorted(),
        time: e.time,
        uploaded: false,
    })
}

fn vertex_of(i: &Intent, device: DeviceId) -> Vertex {
    Vertex {
        rev: i.rev,
        kind: i.kind,
        parents: i.parents.clone(),
        ct_hash: i.ct_hash,
        origins: vec![Origin { device, seq: 0 }],
        time: i.time,
    }
}
