//! Pushing own changes: objects first, then the own log segment (section 6.3 steps 1 and 2),
//! plus the vault header file.

use crate::CrashPoint;
use crate::engine::{Syncer, Warning};
use crate::error::Result;
use crate::ids::{Hash32, Id16, RevId};
use crate::log::{
    Entry, KIND_CHECKPOINT, KIND_DELETE, KIND_PUT, SeenPos, SegmentError, open_segment,
    seal_segment,
};
use crate::names::{Class, RemoteName, classify};
use crate::provider::{Provider, ProviderError};
use crate::state::{Inv, Sealed};
use nuvoletta_core::now_ms;
use nuvoletta_storage::Unlocked;
use std::collections::BTreeMap;

pub(crate) fn stamp(e: &crate::provider::RemoteEntry) -> Option<String> {
    e.etag
        .clone()
        .or_else(|| e.mtime_ms.map(|m| format!("m{m}")))
}

impl<P: Provider> Syncer<P> {
    /// Step 1: upload the object of every own PUT intent that is not confirmed on the remote.
    /// An existing file under the name is never overwritten: identical bytes confirm, anything
    /// else (a half-propagated file) is retried next cycle.
    pub(crate) fn push_objects(&mut self, vault: &mut Unlocked) -> Result<()> {
        let mut blocked_revs: Vec<RevId> = Vec::new();
        for idx in 0..self.state.journal.intents.len() {
            let intent = self.state.journal.intents[idx].clone();
            if intent.kind != KIND_PUT || intent.uploaded {
                continue;
            }
            let name = RemoteName::Object { id: intent.rev }.path();
            let confirmed = match self.state.get_rev(&intent.rev)? {
                Some(bytes) => match self.provider.put_new(&name, &bytes) {
                    Ok(()) => {
                        self.report.objects_pushed += 1;
                        true
                    }
                    Err(ProviderError::AlreadyExists(_)) => match self.provider.get(&name) {
                        Ok(remote) if remote == bytes => true,
                        Ok(_) => {
                            blocked_revs.push(intent.rev);
                            false
                        }
                        Err(ProviderError::NotFound(_)) => false,
                        Err(e) => return Err(e.into()),
                    },
                    Err(e) => return Err(e.into()),
                },
                None => {
                    // Re-emitted after a fork and no longer cached: the remote copy is the
                    // original, accept it if it matches the hash the entry will carry.
                    match self.provider.get(&name) {
                        Ok(remote) if Some(Hash32::of(&remote)) == intent.ct_hash => {
                            self.state.put_rev(&intent.rev, &remote)?;
                            true
                        }
                        Ok(_) | Err(ProviderError::NotFound(_)) => false,
                        Err(e) => return Err(e.into()),
                    }
                }
            };
            if confirmed {
                self.state.journal.intents[idx].uploaded = true;
                self.clear_blocked(&intent.rev);
                self.state.touch_journal();
            }
        }
        for rev in blocked_revs {
            self.handle_blocked_object(vault, rev)?;
        }
        Ok(())
    }

    fn clear_blocked(&mut self, key: &Id16) {
        if let Some(b) = self.state.journal.blocked.as_mut()
            && b.remove(key).is_some()
        {
            if b.is_empty() {
                self.state.journal.blocked = None;
            }
            self.state.touch_journal();
        }
    }

    /// Record that `key` is blocked by a half-written remote file and tell whether the grace
    /// period has passed. Only providers whose listing is authoritative ever give up waiting: on
    /// a synced folder the rest of the file may still arrive.
    fn blocked_too_long(&mut self, key: Id16) -> bool {
        if !self.provider.capabilities().authoritative {
            return false;
        }
        let now = now_ms();
        let since = *self
            .state
            .journal
            .blocked
            .get_or_insert_with(BTreeMap::new)
            .entry(key)
            .or_insert(now);
        self.state.touch_journal();
        now.saturating_sub(since) >= self.cfg.partial_upload_grace_ms
    }

    /// An object name taken by a file with other bytes (a half-written earlier upload that the
    /// credentials can neither overwrite nor delete). After the grace period the revision is
    /// sealed again under a fresh random id: the note is rewritten with identical content, the
    /// next scan sees a new local revision with the same parents and replaces the intent.
    fn handle_blocked_object(&mut self, vault: &mut Unlocked, rev: RevId) -> Result<()> {
        if !self.blocked_too_long(rev) {
            return Ok(());
        }
        let Some(intent) = self
            .state
            .journal
            .intents
            .iter()
            .find(|i| i.rev == rev)
            .cloned()
        else {
            return Ok(());
        };
        // Only the note's current revision can be re-sealed in place; an older re-emitted one
        // stays blocked (it is reported by the missing object, never overwritten).
        if self.state.notes.get(&intent.note).and_then(|r| r.local_rev) != Some(rev) {
            return Ok(());
        }
        let Some(bytes) = self.state.get_rev(&rev)? else {
            return Ok(());
        };
        let payload = crate::local::open_revision(vault.keys(), &bytes, &intent.note, Some(&rev))?;
        let (_, fresh) = crate::local::seal_revision(vault.keys(), &payload)?;
        vault.install_revision(&fresh)?;
        self.clear_blocked(&rev);
        self.record_local_changes(vault)?;
        self.report.warnings.push(Warning::ObjectResealed { rev });
        Ok(())
    }

    /// Step 2: seal the eligible intents into one segment (persisting the sealed bytes before
    /// any upload) and upload it. A previously sealed segment is retried byte-identical.
    pub(crate) fn seal_and_upload(&mut self, vault: &Unlocked) -> Result<()> {
        if self.state.journal.sealed.is_none() {
            self.seal(vault)?;
        }
        if self.state.journal.sealed.is_some() {
            self.upload_sealed(vault)?;
        }
        Ok(())
    }

    fn seen_vector(&self) -> BTreeMap<Id16, SeenPos> {
        self.state
            .peers
            .iter()
            .filter(|(d, p)| **d != self.state.device_id && p.seq() > 0)
            .map(|(d, p)| {
                (
                    *d,
                    SeenPos {
                        seq: p.seq(),
                        hash: p.last_hash(),
                    },
                )
            })
            .collect()
    }

    fn seal(&mut self, vault: &Unlocked) -> Result<()> {
        let me = self.state.device_id;
        let now = now_ms();
        let eligible: Vec<usize> = self
            .state
            .journal
            .intents
            .iter()
            .enumerate()
            .filter(|(_, i)| i.uploaded)
            .map(|(k, _)| k)
            .collect();
        let mut bodies: Vec<Entry> = eligible
            .iter()
            .map(|k| {
                let i = &self.state.journal.intents[*k];
                let mut e = Entry::body(i.kind, i.time);
                e.note_id = Some(i.note);
                e.rev_id = Some(i.rev);
                e.ct_hash = i.ct_hash;
                e.parents = Some(i.parents.clone());
                e
            })
            .collect();
        let vector = self.seen_vector();
        let j = &self.state.journal;
        let changed = !vector.is_empty() && vector != j.last_seen;
        let attach = changed
            && (j.foreign_since_checkpoint
                || now.saturating_sub(j.last_checkpoint_ms) >= self.cfg.checkpoint_interval_ms);
        let standalone = changed && j.foreign_since_checkpoint;
        let mut wrote_seen = false;
        if !bodies.is_empty() && attach {
            if let Some(last) = bodies.last_mut() {
                last.seen = Some(vector.clone());
                wrote_seen = true;
            }
        } else if bodies.is_empty() && standalone {
            let mut c = Entry::body(KIND_CHECKPOINT, now);
            c.seen = Some(vector.clone());
            bodies.push(c);
            wrote_seen = true;
        }
        if bodies.is_empty() {
            return Ok(());
        }
        let (next_seq, last_hash) = {
            let p = self.state.peers.entry(me).or_default();
            (p.seq() + 1, p.last_hash())
        };
        let seg = seal_segment(vault.keys(), me, next_seq, last_hash, bodies)?;
        // Update the in-memory chain, graph and journal, then persist everything at once.
        let last_seq = seg.entries[seg.entries.len() - 1].entry.seq;
        let peer = self.state.peers.entry(me).or_default();
        if peer.first_seen == 0 {
            peer.first_seen = now;
        }
        peer.last_seen = now;
        peer.hashes.extend(seg.entries.iter().map(|e| e.hash));
        peer.segments.insert(seg.first_seq, last_seq);
        for pe in &seg.entries {
            if let (Some(note), Some(rev)) = (pe.entry.note_id, pe.entry.rev_id)
                && let Some(rec) = self.state.notes.get_mut(&note)
            {
                rec.graph.assign_seq(&rev, &me, pe.entry.seq);
            }
        }
        for k in eligible.iter().rev() {
            self.state.journal.intents.remove(*k);
        }
        let j = &mut self.state.journal;
        j.own_len = last_seq;
        j.sealed = Some(Sealed {
            first_seq: seg.first_seq,
            bytes: seg.bytes,
        });
        if wrote_seen {
            j.last_seen = vector;
            j.last_checkpoint_ms = now;
            j.foreign_since_checkpoint = false;
        }
        self.state.touch_journal();
        self.state.touch_notes();
        self.state.touch_peers();
        self.state.save(vault.keys())?;
        self.crash_at(CrashPoint::AfterSealed)
    }

    fn upload_sealed(&mut self, vault: &Unlocked) -> Result<()> {
        let me = self.state.device_id;
        let sealed = self
            .state
            .journal
            .sealed
            .clone()
            .expect("caller checked sealed");
        let name = RemoteName::Segment {
            device: me,
            first_seq: sealed.first_seq,
        }
        .path();
        match self.provider.put_new(&name, &sealed.bytes) {
            Ok(()) => {}
            Err(ProviderError::AlreadyExists(_)) => match self.provider.get(&name) {
                Ok(remote) if remote == sealed.bytes => {}
                Ok(remote) => match open_segment(vault.keys(), &remote) {
                    Err(SegmentError::Unreadable) => {
                        // Not fully arrived: wait. On a provider where a listed file is the
                        // truth it never will, and nothing may remove it: after the grace
                        // period continue under a new device id (the entries of this segment
                        // are unconfirmed, so they are re-emitted).
                        if self.blocked_too_long(me) {
                            self.clear_blocked(&me);
                            self.recover_own_fork(vault, sealed.first_seq, true)?;
                            let new = self.state.device_id;
                            self.report
                                .warnings
                                .retain(|w| !matches!(w, Warning::OwnForkRecovered { .. }));
                            self.report
                                .warnings
                                .push(Warning::LogRestarted { old: me, new });
                        }
                        return Ok(());
                    }
                    Ok(seg) if seg.device == me => {
                        let ours = open_segment(vault.keys(), &sealed.bytes)
                            .map_err(|_| crate::Error::State("sealed segment unreadable"))?;
                        let same = seg.entries.len() == ours.entries.len()
                            && seg
                                .entries
                                .iter()
                                .zip(&ours.entries)
                                .all(|(a, b)| a.hash == b.hash);
                        if !same {
                            let f = first_difference(&seg.entries, &ours.entries, sealed.first_seq);
                            return self.recover_own_fork(vault, f, true);
                        }
                    }
                    _ => return self.recover_own_fork(vault, sealed.first_seq, true),
                },
                Err(ProviderError::NotFound(_)) => return Ok(()),
                Err(e) => return Err(e.into()),
            },
            Err(e) => return Err(e.into()),
        }
        // Confirm with a listing: present under the final name with the expected size.
        let dir = RemoteName::segment_dir(&me);
        let listed = self.provider.list(&dir)?;
        let Some(entry) = listed.iter().find(|e| e.name == name) else {
            return Ok(()); // delayed or dropped: the same bytes are retried next cycle
        };
        if entry.size != sealed.bytes.len() as u64 {
            return Ok(());
        }
        self.crash_at(CrashPoint::BeforeConfirm)?;
        self.confirm_sealed(vault, &name, entry.size, stamp(entry))
    }

    fn confirm_sealed(
        &mut self,
        vault: &Unlocked,
        name: &str,
        size: u64,
        etag: Option<String>,
    ) -> Result<()> {
        let sealed = self.state.journal.sealed.take().expect("checked");
        self.clear_blocked(&self.state.device_id.clone());
        if let Ok(seg) = open_segment(vault.keys(), &sealed.bytes) {
            for pe in &seg.entries {
                if let (Some(note), Some(rev)) = (pe.entry.note_id, pe.entry.rev_id)
                    && let Some(rec) = self.state.notes.get_mut(&note)
                    && (pe.entry.kind == KIND_PUT || pe.entry.kind == KIND_DELETE)
                {
                    rec.base = Some(rev);
                }
            }
        }
        self.state.inventory.insert(
            name.to_owned(),
            Inv {
                size,
                etag,
                seen_at: now_ms(),
            },
        );
        self.report.segments_uploaded += 1;
        self.state.touch_journal();
        self.state.touch_notes();
        self.state.touch_inventory();
        self.state.save(vault.keys())?;
        Ok(())
    }

    /// Publish this device's vault header generation and observe the remote ones (section 2:
    /// one immutable file per generation; `max-header-gen` detects an old header).
    pub(crate) fn sync_headers(&mut self, vault: &Unlocked) -> Result<()> {
        let header = vault.header();
        let local_gen = header.generation();
        let vault_id = header.vault_id();
        let mut remote_max = 0u32;
        let mut have_local = false;
        let listed = self.provider.list("h")?;
        for e in &listed {
            if let Class::Copy(RemoteName::Header { generation }) = classify(&e.name) {
                let main = RemoteName::Header { generation }.path();
                let same = matches!(
                    (self.provider.get(&e.name), self.provider.get(&main)),
                    (Ok(a), Ok(b)) if a == b
                );
                self.report.strays.push(crate::engine::StrayFile {
                    name: e.name.clone(),
                    size: e.size,
                    kind: if same {
                        crate::engine::StrayKind::DuplicateCopy
                    } else {
                        crate::engine::StrayKind::CopyOnly
                    },
                });
                continue;
            }
            let Class::Protocol(RemoteName::Header { generation }) = classify(&e.name) else {
                continue;
            };
            have_local |= generation == local_gen;
            remote_max = remote_max.max(generation);
            let ok = match self.provider.get(&e.name) {
                Ok(bytes) => nuvoletta_crypto::VaultHeader::parse(&bytes).is_ok_and(|h| {
                    h.vault_id() == vault_id
                        && h.generation() == generation
                        && (generation != local_gen || bytes == header.as_bytes())
                }),
                Err(ProviderError::NotFound(_)) => true,
                Err(e) => return Err(e.into()),
            };
            if !ok {
                self.report
                    .warnings
                    .push(Warning::HeaderInvalid { generation });
            }
        }
        if !have_local {
            let name = RemoteName::Header {
                generation: local_gen,
            }
            .path();
            match self.provider.put_new(&name, header.as_bytes()) {
                Ok(()) | Err(ProviderError::AlreadyExists(_)) => {}
                Err(e) => return Err(e.into()),
            }
            remote_max = remote_max.max(local_gen);
        }
        if remote_max > local_gen {
            self.report.warnings.push(Warning::HeaderNewer {
                generation: remote_max,
            });
        }
        if remote_max < self.state.max_header_gen {
            self.report.warnings.push(Warning::HeaderRollback {
                seen: self.state.max_header_gen,
                remote: remote_max,
            });
        }
        let top = self.state.max_header_gen.max(remote_max).max(local_gen);
        if top != self.state.max_header_gen {
            self.state.max_header_gen = top;
            self.state.touch_header_gen();
        }
        Ok(())
    }
}

fn first_difference(
    remote: &[crate::log::ParsedEntry],
    ours: &[crate::log::ParsedEntry],
    first_seq: u64,
) -> u64 {
    for (k, (a, b)) in remote.iter().zip(ours).enumerate() {
        if a.hash != b.hash {
            return first_seq + k as u64;
        }
    }
    first_seq + remote.len().min(ours.len()) as u64
}
