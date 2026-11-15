//! Garbage collection (`SYNC_PROTOCOL.md` section 9) and pruning of the local `revs/` cache
//! (section 6.1).
//!
//! Collection is manual ([`Syncer::collect_garbage`]), conservative (when in doubt, keep) and
//! destructive only for **content objects this device wrote itself**. It never deletes a log
//! segment (they are the only record of the history and are only deletable after a snapshot,
//! which is post-MVP), a header, another device's file, a stray file, a head, a head's direct
//! parent, anything of a note in conflict, or anything while the state is incomplete or forked.
//! It works from the sync state as the last cycle left it and is idempotent: a crash in the
//! middle only leaves some deletions undone.

use crate::engine::Syncer;
use crate::error::Result;
use crate::graph::NoteGraph;
use crate::ids::{DeviceId, RevId};
use crate::names::{Class, RemoteName, classify};
use crate::provider::{Provider, ProviderError};
use nuvoletta_core::now_ms;
use nuvoletta_storage::Unlocked;
use std::collections::{BTreeMap, BTreeSet};

/// Default number of revisions kept per note (section 9 rule 3).
pub const DEFAULT_KEEP_REVISIONS: usize = 5;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Parameters of a collection. [`GcOptions::default`] is the protocol's defaults with the real
/// clock; tests inject the clock through `now_ms`.
#[derive(Clone, Debug)]
pub struct GcOptions {
    /// Compute the report without deleting or changing anything.
    pub dry_run: bool,
    /// A device whose log advanced within this window must have acknowledged a superseding
    /// entry before the revision it supersedes is collected (default 90 days).
    pub ack_window_ms: i64,
    /// Minimum age of the superseding entry (default 30 days).
    pub grace_ms: i64,
    /// Revisions kept per note, at least (default 5).
    pub keep_revisions: usize,
    /// Minimum age of an orphan object before it is deleted (default 7 days).
    pub orphan_age_ms: i64,
    /// The current time in milliseconds since the epoch.
    pub now_ms: i64,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            ack_window_ms: 90 * DAY_MS,
            grace_ms: 30 * DAY_MS,
            keep_revisions: DEFAULT_KEEP_REVISIONS,
            orphan_age_ms: 7 * DAY_MS,
            now_ms: now_ms(),
        }
    }
}

/// Why a collection did nothing at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcSkip {
    /// A device id was used twice: no object of either branch may be deleted (section 10.4).
    Forks,
    /// The local sync state is being rebuilt from the remote and is incomplete.
    Rebuilding,
}

/// Aggregate reasons why content revisions on the remote were kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcKept {
    /// Heads.
    pub heads: usize,
    /// Direct parents of a head.
    pub head_parents: usize,
    /// Every revision of a note that is in conflict or needs attention.
    pub in_conflict: usize,
    /// Among the newest revisions of their note.
    pub recent: usize,
    /// Not written solely by this device (only the writer deletes).
    pub other_writers: usize,
    /// Some device that was active within the window has not acknowledged the superseding entry.
    pub not_acknowledged: usize,
    /// Superseded less than the grace period ago.
    pub within_grace: usize,
}

impl GcKept {
    /// Total number of kept revisions.
    #[must_use]
    pub fn total(&self) -> usize {
        self.heads
            + self.head_parents
            + self.in_conflict
            + self.recent
            + self.other_writers
            + self.not_acknowledged
            + self.within_grace
    }
}

/// What a collection did (or, in a dry run, would do).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// This was a dry run: nothing was deleted.
    pub dry_run: bool,
    /// The collection refused to run, and why.
    pub skipped: Option<GcSkip>,
    /// Old revision objects deleted (or that would be).
    pub objects_collected: usize,
    /// Bytes freed on the remote by them.
    pub bytes_collected: u64,
    /// Orphan objects deleted (or that would be).
    pub orphans_collected: usize,
    /// Bytes freed by orphans.
    pub orphan_bytes: u64,
    /// Orphans not yet old enough.
    pub orphans_waiting: usize,
    /// Collectable objects that were already absent from the remote.
    pub already_gone: usize,
    /// Content revisions kept, by reason.
    pub kept: GcKept,
    /// Own log segments kept (segments are never deleted before snapshots exist).
    pub segments_kept: usize,
    /// Cached revisions removed (or that would be) from the local `revs/` cache.
    pub revs_pruned: usize,
    /// Bytes of the local cache freed by that.
    pub revs_pruned_bytes: u64,
}

/// The revisions of a note that count as "the last K": the K newest by entry time and every
/// revision within K generations of a head. The union errs on the side of keeping.
fn recent_set(graph: &NoteGraph, k: usize) -> BTreeSet<RevId> {
    let mut by_time: Vec<(i64, RevId)> = graph
        .vertices
        .values()
        .filter(|v| v.is_put())
        .map(|v| (v.time, v.rev))
        .collect();
    by_time.sort_unstable();
    let mut out: BTreeSet<RevId> = by_time.iter().rev().take(k).map(|(_, r)| *r).collect();
    let mut frontier = graph.heads();
    for _ in 0..k {
        let mut next = Vec::new();
        for r in frontier {
            out.insert(r);
            if let Some(v) = graph.vertices.get(&r) {
                next.extend(v.parents.iter().copied());
            }
        }
        frontier = next;
    }
    out
}

/// Heads and their direct parents.
fn head_set(graph: &NoteGraph) -> (BTreeSet<RevId>, BTreeSet<RevId>) {
    let heads = graph.heads();
    let parents = heads
        .iter()
        .filter_map(|h| graph.vertices.get(h))
        .flat_map(|v| v.parents.iter().copied())
        .collect();
    (heads.into_iter().collect(), parents)
}

impl<P: Provider> Syncer<P> {
    /// Whether any device id is known to have been used twice.
    fn any_fork(&self) -> bool {
        self.state.peers.values().any(|p| p.fork.is_some())
    }

    /// Devices that must have acknowledged an entry written by `writer` at `seq` before it
    /// counts as seen everywhere: every other device that was active within the window, and
    /// every device that only appears in somebody's checkpoint (nothing is known about it).
    fn unacknowledged(&self, writer: &DeviceId, seq: u64, opts: &GcOptions) -> bool {
        let me = self.state.device_id;
        let cutoff = opts.now_ms - opts.ack_window_ms;
        let acked = |claims: &BTreeMap<DeviceId, crate::log::SeenPos>| {
            claims.get(writer).is_some_and(|p| p.seq >= seq)
        };
        for (d, peer) in &self.state.peers {
            if *d == me || d == writer {
                continue;
            }
            // `last_seen` is this device's own clock at the time the log advanced.
            let active = peer.last_seen == 0 || peer.last_seen >= cutoff;
            if active && !acked(&peer.claims) {
                return true;
            }
        }
        // A device named in a checkpoint whose log this device never saw.
        let known = |d: &DeviceId| self.state.peers.contains_key(d);
        self.state
            .peers
            .values()
            .flat_map(|p| p.claims.keys())
            .any(|d| *d != me && d != writer && !known(d))
    }

    /// Delete old revision objects and orphans that this device wrote, per section 9. See the
    /// module documentation for what is never deleted. Run a sync cycle first so the decision
    /// is based on fresh knowledge (a stale state is safe, only more conservative).
    ///
    /// With `dry_run` nothing is deleted or changed and the report says what would be.
    ///
    /// # Errors
    /// Provider failures (transient or otherwise): collection stops at the failing deletion.
    /// Everything deleted before it was collectable, and a repeat continues where it stopped.
    pub fn collect_garbage(&mut self, vault: &Unlocked, opts: &GcOptions) -> Result<GcReport> {
        let mut report = GcReport {
            dry_run: opts.dry_run,
            ..GcReport::default()
        };
        if self.state.rebuilt {
            report.skipped = Some(GcSkip::Rebuilding);
            return Ok(report);
        }
        if self.any_fork() {
            report.skipped = Some(GcSkip::Forks);
            return Ok(report);
        }
        let me = self.state.device_id;
        let listing = self.provider.list("")?;
        let mut sizes: BTreeMap<RevId, u64> = BTreeMap::new();
        for e in &listing {
            match classify(&e.name) {
                Class::Protocol(RemoteName::Object { id }) => {
                    sizes.insert(id, e.size);
                }
                Class::Protocol(RemoteName::Segment { device, .. }) if device == me => {
                    report.segments_kept += 1;
                }
                _ => {}
            }
        }

        let mut doomed: Vec<RevId> = Vec::new();
        for rec in self.state.notes.values() {
            let g = &rec.graph;
            let (heads, head_parents) = head_set(g);
            let recent = recent_set(g, opts.keep_revisions);
            let conflict = heads.len() > 1 || rec.needs_attention;
            let mut children: BTreeMap<RevId, Vec<&crate::graph::Vertex>> = BTreeMap::new();
            for v in g.vertices.values() {
                for p in &v.parents {
                    children.entry(*p).or_default().push(v);
                }
            }
            for v in g.vertices.values().filter(|v| v.is_put()) {
                let kept = &mut report.kept;
                if heads.contains(&v.rev) {
                    kept.heads += 1;
                } else if head_parents.contains(&v.rev) {
                    kept.head_parents += 1;
                } else if conflict {
                    kept.in_conflict += 1;
                } else if recent.contains(&v.rev) {
                    kept.recent += 1;
                } else if v.origins.is_empty() || v.origins.iter().any(|o| o.device != me) {
                    // Only the writer deletes. Several origins (a re-emission after a fork)
                    // mean another log introduces it too.
                    kept.other_writers += 1;
                } else {
                    let kids = children.get(&v.rev).map_or(&[][..], Vec::as_slice);
                    // Not a head, so it has children; every one of them must be old enough and
                    // acknowledged by every device that matters. Unsealed own entries have
                    // seq 0: not acknowledged by anybody.
                    let newest = kids.iter().map(|k| k.time).max();
                    if newest.is_none_or(|t| opts.now_ms.saturating_sub(t) < opts.grace_ms) {
                        kept.within_grace += 1;
                    } else if kids
                        .iter()
                        .flat_map(|k| &k.origins)
                        .any(|o| o.seq == 0 || self.unacknowledged(&o.device, o.seq, opts))
                    {
                        kept.not_acknowledged += 1;
                    } else {
                        doomed.push(v.rev);
                    }
                }
            }
        }

        // Orphans: own objects never referenced by an entry (section 9 rule 5).
        let journal_revs: BTreeSet<RevId> =
            self.state.journal.intents.iter().map(|i| i.rev).collect();
        let mut orphans: Vec<RevId> = Vec::new();
        let mut stale_records: Vec<RevId> = Vec::new();
        for (rev, since) in &self.state.pending.orphans {
            if self.rev_note.contains_key(rev) || journal_revs.contains(rev) {
                stale_records.push(*rev); // it was referenced after all: never an orphan
            } else if opts.now_ms.saturating_sub(*since) < opts.orphan_age_ms {
                report.orphans_waiting += 1;
            } else {
                orphans.push(*rev);
            }
        }

        let mut deleted: Vec<RevId> = Vec::new();
        for (revs, is_orphan) in [(&doomed, false), (&orphans, true)] {
            for rev in revs {
                let Some(size) = sizes.get(rev).copied() else {
                    if is_orphan {
                        deleted.push(*rev); // nothing on the remote: forget the record
                    } else {
                        report.already_gone += 1;
                    }
                    continue;
                };
                if !opts.dry_run {
                    match self
                        .provider
                        .delete(&RemoteName::Object { id: *rev }.path())
                    {
                        Ok(()) | Err(ProviderError::NotFound(_)) => {}
                        Err(e) => {
                            self.forget_orphans(&deleted, &stale_records, vault)?;
                            return Err(e.into());
                        }
                    }
                }
                if is_orphan {
                    report.orphans_collected += 1;
                    report.orphan_bytes += size;
                    deleted.push(*rev);
                } else {
                    report.objects_collected += 1;
                    report.bytes_collected += size;
                }
            }
        }
        if !opts.dry_run {
            self.forget_orphans(&deleted, &stale_records, vault)?;
        }
        let (n, bytes) = self.prune_revs(opts.keep_revisions, opts.dry_run)?;
        report.revs_pruned = n;
        report.revs_pruned_bytes = bytes;
        if !opts.dry_run {
            self.state.save(vault.keys())?;
        }
        Ok(report)
    }

    fn forget_orphans(
        &mut self,
        deleted: &[RevId],
        stale: &[RevId],
        vault: &Unlocked,
    ) -> Result<()> {
        for r in deleted.iter().chain(stale) {
            self.state.pending.orphans.remove(r);
        }
        if !deleted.is_empty() || !stale.is_empty() {
            self.state.touch_pending();
        }
        self.state.save(vault.keys())
    }

    /// Prune the local `revs/` cache (section 6.1). Kept per note: the `base` and the local
    /// revision, every head, every direct parent of a head and the last `keep` revisions; for a
    /// note in conflict (or needing attention) everything, because merging needs ancestors.
    /// Revisions queued in the journal and files the sync state does not know are never removed.
    /// Returns the number and the bytes of the removed (or, in a dry run, removable) files.
    pub(crate) fn prune_revs(&mut self, keep: usize, dry_run: bool) -> Result<(usize, u64)> {
        let mut retain: BTreeSet<RevId> =
            self.state.journal.intents.iter().map(|i| i.rev).collect();
        for rec in self.state.notes.values() {
            retain.extend(rec.base);
            retain.extend(rec.local_rev);
            retain.extend(rec.applying);
            let g = &rec.graph;
            let (heads, parents) = head_set(g);
            let conflict = heads.len() > 1 || rec.needs_attention;
            retain.extend(heads);
            retain.extend(parents);
            retain.extend(recent_set(g, keep));
            if conflict {
                retain.extend(g.vertices.keys().copied());
            }
        }
        let (mut n, mut bytes) = (0, 0);
        for rev in self.state.cached_revs()? {
            if retain.contains(&rev) || !self.rev_note.contains_key(&rev) {
                continue;
            }
            n += 1;
            bytes += self.state.rev_size(&rev).unwrap_or(0);
            if !dry_run {
                self.state.remove_rev(&rev)?;
            }
        }
        Ok((n, bytes))
    }
}
