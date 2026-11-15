//! Pulling other devices' changes: logs (section 6.3 step 4) and objects (step 5), with
//! stray-copy handling (section 8), gap/rollback/withholding detection (section 10) and forks.

use crate::engine::{InvalidEntry, StrayFile, StrayKind, Syncer, Warning};
use crate::error::Result;
use crate::graph::{Insert, Origin, Vertex};
use crate::ids::{DeviceId, Hash32, Id16, RevId};
use crate::local::open_revision;
use crate::log::{KIND_DELETE, KIND_PUT, ParsedEntry, SegmentError, open_segment};
use crate::names::{Class, RemoteName, classify};
use crate::provider::{Provider, ProviderError, RemoteEntry};
use crate::push::stamp;
use crate::state::{Buffered, Inv, Retry};
use nuvoletta_core::now_ms;
use nuvoletta_storage::Unlocked;
use std::collections::{BTreeMap, HashMap};

struct SegRef<'a> {
    entry: &'a RemoteEntry,
    first_seq: u64,
    copy: bool,
}

/// Upper bound for the bytes of segments buffered after gaps (all devices together). A hostile
/// provider cannot make local state grow past it; segments over the bound are simply refetched
/// every cycle, as without buffering.
const MAX_BUFFERED_BYTES: usize = 32 * 1024 * 1024;

enum Outcome {
    Applied,
    NotArrived,
    Gap(u64),
    Forked,
    Invalid,
}

impl<P: Provider> Syncer<P> {
    fn invalid(&mut self, device: DeviceId, seq: u64, reason: &'static str) {
        self.report.invalid.push(InvalidEntry {
            device,
            seq,
            reason,
        });
    }

    /// Step 4: fetch, verify and apply the segments of every device.
    pub(crate) fn pull_logs(&mut self, vault: &Unlocked, listing: &[RemoteEntry]) -> Result<()> {
        let me = self.state.device_id;
        let own_dir = format!("d/{me}/l/");
        let mut segs: BTreeMap<DeviceId, Vec<SegRef<'_>>> = BTreeMap::new();
        for e in listing {
            match classify(&e.name) {
                Class::Protocol(RemoteName::Segment { device, first_seq }) => {
                    segs.entry(device).or_default().push(SegRef {
                        entry: e,
                        first_seq,
                        copy: false,
                    });
                }
                Class::Copy(RemoteName::Segment { device, first_seq }) => {
                    segs.entry(device).or_default().push(SegRef {
                        entry: e,
                        first_seq,
                        copy: true,
                    });
                }
                Class::Protocol(_) | Class::Copy(RemoteName::Object { .. }) => {}
                Class::Copy(RemoteName::Header { .. }) => {} // reported by the header sync
                Class::Copy(_) => self.report.strays.push(StrayFile {
                    name: e.name.clone(),
                    size: e.size,
                    kind: StrayKind::CopyOnly,
                }),
                Class::Temp => {
                    // Our own interrupted uploads only: never touch another writer's files.
                    if e.name.starts_with(&own_dir) {
                        match self.provider.delete(&e.name) {
                            Ok(()) | Err(ProviderError::NotFound(_)) => {}
                            Err(err) => return Err(err.into()),
                        }
                    }
                }
                Class::Stray => self.report.strays.push(StrayFile {
                    name: e.name.clone(),
                    size: e.size,
                    kind: StrayKind::Ignored,
                }),
            }
        }
        for list in segs.values_mut() {
            list.sort_by_key(|s| (s.copy, s.first_seq));
        }
        for (device, list) in &segs {
            self.pull_device(vault, *device, list)?;
        }
        // A buffered segment whose file is no longer listed is dropped: what the remote shows
        // now is what counts (it is fetched again if it comes back).
        let listed: std::collections::HashSet<&str> =
            listing.iter().map(|e| e.name.as_str()).collect();
        let before = self.state.pending.buffered.len();
        self.state
            .pending
            .buffered
            .retain(|n, _| listed.contains(n.as_str()));
        if before != self.state.pending.buffered.len() {
            self.state.touch_pending();
        }
        self.check_rollbacks(&segs);
        self.check_claims();
        Ok(())
    }

    fn pull_device(
        &mut self,
        vault: &Unlocked,
        device: DeviceId,
        list: &[SegRef<'_>],
    ) -> Result<()> {
        if self
            .state
            .peers
            .get(&device)
            .is_some_and(|p| p.fork.is_some())
        {
            // Frozen: its files are evidence, never applied.
            let before = self.state.pending.buffered.len();
            self.state
                .pending
                .buffered
                .retain(|_, b| b.device != device);
            if before != self.state.pending.buffered.len() {
                self.state.touch_pending();
            }
            return Ok(());
        }
        let mut gap: Option<u64> = None;
        for s in list {
            let known = self
                .state
                .inventory
                .get(&s.entry.name)
                .is_some_and(|i| i.size == s.entry.size && i.etag == stamp(s.entry));
            if known {
                continue;
            }
            // Section 6.3 step 4: a segment that arrived after a gap was verified and buffered
            // earlier; use that copy instead of fetching it again, as long as the listing shows
            // the same file.
            let buffered = self
                .state
                .pending
                .buffered
                .get(&s.entry.name)
                .filter(|b| {
                    b.device == device && b.size == s.entry.size && b.etag == stamp(s.entry)
                })
                .map(|b| b.bytes.clone());
            let from_buffer = buffered.is_some();
            let bytes = match buffered {
                Some(b) => b,
                None => match self.provider.get(&s.entry.name) {
                    Ok(b) => b,
                    Err(ProviderError::NotFound(_)) => continue,
                    Err(e) => return Err(e.into()),
                },
            };
            let outcome = self.process_segment(vault, device, s, &bytes)?;
            if !matches!(outcome, Outcome::Gap(_))
                && self.state.pending.buffered.remove(&s.entry.name).is_some()
            {
                // Applied, or it turned out invalid or forked now: handled as for any segment.
                self.state.touch_pending();
            }
            match outcome {
                Outcome::Applied => {
                    self.state.inventory.insert(
                        s.entry.name.clone(),
                        Inv {
                            size: s.entry.size,
                            etag: stamp(s.entry),
                            seen_at: now_ms(),
                        },
                    );
                    self.state.touch_inventory();
                }
                Outcome::Gap(expected) => {
                    gap = Some(expected);
                    if !from_buffer {
                        self.buffer_segment(device, s.entry, bytes);
                    }
                }
                Outcome::Forked => break,
                Outcome::NotArrived | Outcome::Invalid => {}
            }
        }
        let have = self.state.peers.get(&device).map_or(0, |p| p.seq());
        match gap {
            Some(expected) if expected > have => {
                let now = now_ms();
                let entry = self.state.pending.gaps.entry(device).or_insert((
                    expected,
                    Retry {
                        tries: 0,
                        since: now,
                    },
                ));
                if entry.0 != expected {
                    *entry = (
                        expected,
                        Retry {
                            tries: 0,
                            since: now,
                        },
                    );
                }
                entry.1.tries += 1;
                if entry.1.tries >= self.cfg.retries_before_warning {
                    self.report.warnings.push(Warning::StreamGap {
                        device,
                        expected_seq: expected,
                    });
                }
                self.state.touch_pending();
            }
            _ => {
                if self.state.pending.gaps.remove(&device).is_some() {
                    self.state.touch_pending();
                }
            }
        }
        Ok(())
    }

    /// Keep a verified segment that arrived after a gap (bounded, see [`MAX_BUFFERED_BYTES`]).
    fn buffer_segment(&mut self, device: DeviceId, entry: &RemoteEntry, bytes: Vec<u8>) {
        let used: usize = self
            .state
            .pending
            .buffered
            .values()
            .map(|b| b.bytes.len())
            .sum();
        if used + bytes.len() > MAX_BUFFERED_BYTES {
            return;
        }
        self.state.pending.buffered.insert(
            entry.name.clone(),
            Buffered {
                device,
                size: entry.size,
                etag: stamp(entry),
                bytes,
            },
        );
        self.state.touch_pending();
    }

    fn process_segment(
        &mut self,
        vault: &Unlocked,
        device: DeviceId,
        s: &SegRef<'_>,
        bytes: &[u8],
    ) -> Result<Outcome> {
        let me = self.state.device_id;
        let seg = match open_segment(vault.keys(), bytes) {
            Err(SegmentError::Unreadable) => {
                if s.copy {
                    self.report.strays.push(StrayFile {
                        name: s.entry.name.clone(),
                        size: s.entry.size,
                        kind: StrayKind::NotYetArrived,
                    });
                }
                return Ok(Outcome::NotArrived);
            }
            Err(SegmentError::Malformed(reason)) => {
                self.invalid(device, 0, reason);
                return Ok(Outcome::Invalid);
            }
            Ok(seg) => seg,
        };
        if seg.device != device {
            self.invalid(device, 0, "segment scope differs from its directory");
            return Ok(Outcome::Invalid);
        }
        if !s.copy && seg.first_seq() != s.first_seq {
            self.invalid(
                device,
                seg.first_seq(),
                "file name does not match the first sequence number",
            );
            return Ok(Outcome::Invalid);
        }
        let n = self.state.peers.get(&device).map_or(0, |p| p.seq());
        // Entries we already verified must be byte-for-byte the same chain.
        for pe in &seg.entries {
            let seq = pe.entry.seq;
            if seq <= n {
                let known = self.state.peers[&device].hashes[usize::try_from(seq - 1).unwrap_or(0)];
                if known != pe.hash {
                    self.on_fork(vault, device, seq, vec![known, pe.hash])?;
                    if s.copy {
                        self.report.strays.push(StrayFile {
                            name: s.entry.name.clone(),
                            size: s.entry.size,
                            kind: StrayKind::ForkEvidence,
                        });
                    }
                    return Ok(Outcome::Forked);
                }
            }
        }
        let last_hash = self
            .state
            .peers
            .get(&device)
            .map_or(Hash32::ZERO, crate::state::Peer::last_hash);
        let first = seg.first_seq();
        if first == n + 1 && seg.entries[0].entry.prev != last_hash {
            if n == 0 {
                self.invalid(device, first, "first entry of a log must have a zero prev");
                return Ok(Outcome::Invalid);
            }
            self.on_fork(vault, device, n, vec![last_hash, seg.entries[0].entry.prev])?;
            if s.copy {
                self.report.strays.push(StrayFile {
                    name: s.entry.name.clone(),
                    size: s.entry.size,
                    kind: StrayKind::ForkEvidence,
                });
            }
            return Ok(Outcome::Forked);
        }
        if s.copy {
            // Authenticated copies of segments are report-only (section 8).
            self.report.strays.push(StrayFile {
                name: s.entry.name.clone(),
                size: s.entry.size,
                kind: if seg.last_seq() <= n {
                    StrayKind::DuplicateCopy
                } else {
                    StrayKind::CopyOnly
                },
            });
            return Ok(Outcome::Applied);
        }
        if first > n + 1 {
            return Ok(Outcome::Gap(n + 1));
        }
        for pe in seg.entries.iter().filter(|e| e.entry.seq > n) {
            self.apply_entry(device, pe);
        }
        let last = seg.last_seq();
        let peer = self.state.peers.entry(device).or_default();
        peer.segments.insert(first, last);
        let len = peer.seq();
        if device == me {
            self.state.journal.own_len = len;
            self.state.touch_journal();
        }
        self.state.touch_peers();
        Ok(Outcome::Applied)
    }

    /// Apply one verified, chain-linked entry of `device` (also used to adopt own entries
    /// found on the remote after a restore).
    fn apply_entry(&mut self, device: DeviceId, pe: &ParsedEntry) {
        let me = self.state.device_id;
        let e = &pe.entry;
        let now = now_ms();
        let peer = self.state.peers.entry(device).or_default();
        if peer.first_seen == 0 {
            peer.first_seen = now;
        }
        peer.last_seen = now;
        peer.hashes.push(pe.hash);
        self.state.touch_peers();
        if let Err(reason) = e.check() {
            self.invalid(device, e.seq, reason);
            return;
        }
        if let Some(seen) = &e.seen {
            let peer = self.state.peers.get_mut(&device).expect("inserted above");
            for (d, pos) in seen {
                let slot = peer.claims.entry(*d).or_insert(*pos);
                if pos.seq > slot.seq {
                    *slot = *pos;
                }
            }
            peer.last_checkpoint_seq = e.seq;
        }
        if e.kind != KIND_PUT && e.kind != KIND_DELETE {
            return;
        }
        let (Some(note), Some(rev)) = (e.note_id, e.rev_id) else {
            return;
        };
        if self.rev_note.get(&rev).is_some_and(|n| *n != note) {
            self.invalid(device, e.seq, "revision id reused for another note");
            return;
        }
        let rec = self.state.notes.entry(note).or_default();
        let outcome = rec.graph.insert(Vertex {
            rev,
            kind: e.kind,
            parents: e.parents_sorted(),
            ct_hash: e.ct_hash,
            origins: vec![Origin { device, seq: e.seq }],
            time: e.time,
        });
        if outcome == Insert::Conflict {
            self.invalid(
                device,
                e.seq,
                "revision id redefined with different content",
            );
            return;
        }
        self.rev_note.insert(rev, note);
        self.touched.insert(note);
        self.state.touch_notes();
        if device != me {
            self.report.entries_applied += 1;
            self.state.journal.foreign_since_checkpoint = true;
            self.state.touch_journal();
        }
    }

    /// A device whose visible log is shorter than what was verified earlier: rollback.
    fn check_rollbacks(&mut self, segs: &BTreeMap<DeviceId, Vec<SegRef<'_>>>) {
        let me = self.state.device_id;
        let mut found = Vec::new();
        for (device, peer) in &self.state.peers {
            if peer.fork.is_some() {
                continue;
            }
            let mut known = peer.seq();
            if *device == me
                && let Some(s) = &self.state.journal.sealed
            {
                known = s.first_seq - 1;
            }
            if known == 0 {
                continue;
            }
            let listed = segs
                .get(device)
                .map(|l| l.iter().filter(|s| !s.copy).collect::<Vec<_>>())
                .unwrap_or_default();
            let lasts: Vec<Option<u64>> = listed
                .iter()
                .map(|s| peer.segments.get(&s.first_seq).copied())
                .collect();
            if lasts.iter().any(Option::is_none) {
                continue; // something listed has not been read yet: cannot judge
            }
            let remote = lasts.into_iter().flatten().max().unwrap_or(0);
            if remote < known {
                found.push(Warning::Rollback {
                    device: *device,
                    known_seq: known,
                    remote_seq: remote,
                });
            }
        }
        self.report.warnings.extend(found);
    }

    /// Cross-attestation (section 10.2): compare what checkpoints claim with what we verified.
    fn check_claims(&mut self) {
        let mut claimed: BTreeMap<DeviceId, u64> = BTreeMap::new();
        let mut warnings = Vec::new();
        for (reporter, peer) in &self.state.peers {
            if peer.fork.is_some() {
                continue;
            }
            for (device, pos) in &peer.claims {
                if device == reporter {
                    continue;
                }
                let target = self.state.peers.get(device);
                if target.is_some_and(|t| t.fork.is_some()) {
                    continue;
                }
                let have = target.map_or(0, crate::state::Peer::seq);
                if pos.seq <= have {
                    let known =
                        target.expect("have > 0").hashes[usize::try_from(pos.seq - 1).unwrap_or(0)];
                    if known != pos.hash {
                        warnings.push(Warning::DivergentCheckpoint {
                            reporter: *reporter,
                            device: *device,
                            seq: pos.seq,
                        });
                    }
                } else {
                    let slot = claimed.entry(*device).or_insert(0);
                    *slot = (*slot).max(pos.seq);
                }
            }
        }
        let now = now_ms();
        let mut changed = false;
        for (device, seq) in &claimed {
            let have = self
                .state
                .peers
                .get(device)
                .map_or(0, crate::state::Peer::seq);
            let slot = self.state.pending.withheld.entry(*device).or_insert((
                *seq,
                Retry {
                    tries: 0,
                    since: now,
                },
            ));
            slot.0 = *seq;
            slot.1.tries += 1;
            changed = true;
            if slot.1.tries >= self.cfg.retries_before_warning {
                warnings.push(Warning::PossiblyWithheld {
                    device: *device,
                    claimed_seq: *seq,
                    have_seq: have,
                });
            }
        }
        let before = self.state.pending.withheld.len();
        self.state
            .pending
            .withheld
            .retain(|d, _| claimed.contains_key(d));
        if changed || before != self.state.pending.withheld.len() {
            self.state.touch_pending();
        }
        self.report.warnings.extend(warnings);
    }

    /// Step 5: fetch the objects of heads and their direct parents (the revisions merging and
    /// conflict resolution need) that are not cached, verifying hash, ids and decryption.
    pub(crate) fn pull_objects(&mut self, vault: &Unlocked, listing: &[RemoteEntry]) -> Result<()> {
        let mut copies: HashMap<RevId, Vec<String>> = HashMap::new();
        for e in listing {
            if let Class::Copy(RemoteName::Object { id }) = classify(&e.name) {
                copies.entry(id).or_default().push(e.name.clone());
            }
        }
        let mut need: Vec<(Id16, RevId, Hash32)> = Vec::new();
        for (note, rec) in &self.state.notes {
            let heads = rec.graph.heads();
            let mut wanted: Vec<RevId> = heads.clone();
            for h in &heads {
                wanted.extend(rec.graph.vertices[h].parents.iter().copied());
            }
            wanted.sort();
            wanted.dedup();
            for r in wanted {
                if let Some(v) = rec.graph.vertices.get(&r)
                    && let (true, Some(ct)) = (v.is_put(), v.ct_hash)
                    && !self.state.has_rev(&r)
                {
                    need.push((*note, r, ct));
                }
            }
        }
        let now = now_ms();
        let mut still_missing = std::collections::BTreeSet::new();
        for (note, rev, ct) in need {
            let extra = copies.get(&rev).cloned().unwrap_or_default();
            if self.fetch_object(vault, &note, &rev, &ct, &extra)? {
                self.report.objects_fetched += 1;
                self.state.pending.objects.remove(&rev);
                self.state.touch_pending();
            } else {
                still_missing.insert(rev);
                let r = self.state.pending.objects.entry(rev).or_insert(Retry {
                    tries: 0,
                    since: now,
                });
                r.tries += 1;
                if r.tries >= self.cfg.retries_before_warning {
                    self.report
                        .warnings
                        .push(Warning::ObjectUnavailable { rev });
                }
                self.state.touch_pending();
            }
        }
        let before = self.state.pending.objects.len();
        self.state
            .pending
            .objects
            .retain(|r, _| still_missing.contains(r));
        if before != self.state.pending.objects.len() {
            self.state.touch_pending();
        }
        for (id, names) in &copies {
            for n in names {
                let size = listing.iter().find(|e| &e.name == n).map_or(0, |e| e.size);
                self.report.strays.push(StrayFile {
                    name: n.clone(),
                    size,
                    kind: if self.state.has_rev(id) {
                        StrayKind::DuplicateCopy
                    } else {
                        StrayKind::CopyOnly
                    },
                });
            }
        }
        Ok(())
    }

    /// Fetch and verify one revision object from its canonical name or an authenticated copy.
    /// Returns false while it has not arrived (or does not verify yet).
    pub(crate) fn fetch_object(
        &mut self,
        vault: &Unlocked,
        note: &Id16,
        rev: &RevId,
        ct_hash: &Hash32,
        extra_names: &[String],
    ) -> Result<bool> {
        let mut names = vec![RemoteName::Object { id: *rev }.path()];
        names.extend(extra_names.iter().cloned());
        for name in names {
            let bytes = match self.provider.get(&name) {
                Ok(b) => b,
                Err(ProviderError::NotFound(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            if Hash32::of(&bytes) != *ct_hash
                || open_revision(vault.keys(), &bytes, note, Some(rev)).is_err()
            {
                continue; // half-synced or tampered: not (yet) usable, never overwritten
            }
            self.state.put_rev(rev, &bytes)?;
            return Ok(true);
        }
        Ok(false)
    }
}
