//! The sync engine: [`Syncer`] runs the idempotent sync cycle of `SYNC_PROTOCOL.md` section 6.3
//! between a vault (`nuvoletta-storage`) and a [`Provider`].
//!
//! The cycle is split by subject: `local` (own edits become journal intents), `push` (objects
//! and own log segments), `pull` (listing, foreign logs, objects, forks), `reconcile`
//! (fast-forward, automatic merge, conflicts) and `resolve` (user resolution actions).
//!
//! Crash safety: state is persisted at the points where the next run would otherwise misread
//! the world (after sealing a segment, after applying foreign entries, before touching the
//! vault's notes), and every step is idempotent, so a crash anywhere is repaired by running the
//! cycle again.

use crate::CrashPoint;
use crate::error::{Error, Result};
use crate::ids::{DeviceId, Id16, RevId};
use crate::provider::Provider;
use crate::state::{STATE_DIR, State};
use nuvoletta_storage::Unlocked;
use nuvoletta_storage::layout::META_DIR;
use std::collections::{BTreeSet, HashMap};

/// Tunables. The defaults are the protocol's; tests lower the intervals.
#[derive(Clone, Debug)]
pub struct SyncConfig {
    /// Minimum time between `seen` attachments while only editing (section 10.2: hourly). A
    /// checkpoint is always written after a cycle that applied foreign entries.
    pub checkpoint_interval_ms: i64,
    /// Cycles an expected file or claimed entry may be missing before a warning is raised
    /// ("possibly corrupted or withheld").
    pub retries_before_warning: u32,
    /// How long a half-written file under a name this device needs may stay before the device
    /// stops waiting for it and re-seals under a fresh name (only on providers whose listing is
    /// authoritative, such as WebDAV, where it will never complete by itself; a synced folder
    /// is always waited for). The wait covers a slow upload of another request still in flight.
    pub partial_upload_grace_ms: i64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval_ms: 60 * 60 * 1000,
            retries_before_warning: 3,
            partial_upload_grace_ms: 10 * 60 * 1000,
        }
    }
}

/// Something noteworthy the user should be able to see in a status or diagnostics screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The remote shows a shorter log for a device than this device verified earlier.
    Rollback {
        /// The device.
        device: DeviceId,
        /// Highest sequence number verified earlier.
        known_seq: u64,
        /// Highest sequence number now visible.
        remote_seq: u64,
    },
    /// A device id was used twice; its stream is frozen below `seq`.
    DeviceFork {
        /// The forked device id.
        device: DeviceId,
        /// First sequence number that is not applied.
        seq: u64,
    },
    /// This device found a fork of its own id and switched to a new one.
    OwnForkRecovered {
        /// The id that was used twice.
        old: DeviceId,
        /// The new id.
        new: DeviceId,
    },
    /// A checkpoint claims entries of a device that this device cannot see.
    PossiblyWithheld {
        /// The device whose entries are missing.
        device: DeviceId,
        /// Highest sequence number claimed by a checkpoint.
        claimed_seq: u64,
        /// Highest sequence number verified here.
        have_seq: u64,
    },
    /// A checkpoint records a different hash for an entry than this device verified.
    DivergentCheckpoint {
        /// The device that wrote the checkpoint.
        reporter: DeviceId,
        /// The device whose entry hash differs.
        device: DeviceId,
        /// The sequence number.
        seq: u64,
    },
    /// A device's stream stops at a gap that has not filled for several cycles.
    StreamGap {
        /// The device.
        device: DeviceId,
        /// The sequence number expected next.
        expected_seq: u64,
    },
    /// A log segment of a device that does not authenticate (truncated, corrupted or forged)
    /// has stayed that way for several cycles. Other segments are unaffected.
    SegmentUnreadable {
        /// The device whose directory holds the file.
        device: DeviceId,
        /// First sequence number named by the file.
        first_seq: u64,
    },
    /// A referenced object has not arrived or fails verification after several cycles.
    ObjectUnavailable {
        /// The revision.
        rev: RevId,
    },
    /// A half-written file of an interrupted upload blocked this device's next log segment, and
    /// the provider cannot remove it: the device continues its log under a new id. Entries
    /// already confirmed stay valid under the old id; other devices keep reporting its stream as
    /// stopped at the damaged file.
    LogRestarted {
        /// The id whose next file name is blocked.
        old: DeviceId,
        /// The id used from now on.
        new: DeviceId,
    },
    /// A half-written object blocked the name of a note revision; the revision was sealed again
    /// under a new id.
    ObjectResealed {
        /// The blocked revision.
        rev: RevId,
    },
    /// The remote has a newer vault header generation than this device (re-unlock needed).
    HeaderNewer {
        /// Remote generation.
        generation: u32,
    },
    /// A remote header file of this vault, of the generation this device holds, authenticates
    /// under this vault's keys but has different bytes: another device changed the passphrase
    /// at the same moment and its file won the name. Both are genuine; this device adopts the
    /// other one with the new passphrase and changes again from there.
    HeaderDiverged {
        /// The generation both headers carry.
        generation: u32,
    },
    /// The highest remote header generation is lower than one seen before.
    HeaderRollback {
        /// Highest generation seen before.
        seen: u32,
        /// Highest generation on the remote now.
        remote: u32,
    },
    /// A remote header file is invalid, belongs to another vault, fails its MAC under this
    /// vault's keys or differs from the local one of the same generation. It is never counted
    /// as a newer generation.
    HeaderInvalid {
        /// Generation named by the file.
        generation: u32,
    },
    /// The local journal could not be read and was moved aside; the sync continued with an
    /// empty one, so unsent edits are found again by the scan (nothing is lost).
    JournalReset {
        /// File name (in the sync-state folder) the damaged journal was moved to.
        moved_to: String,
    },
}

/// An entry that authenticated but is not valid; reported, never applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidEntry {
    /// Writer.
    pub device: DeviceId,
    /// Sequence number (0 if the whole segment is invalid).
    pub seq: u64,
    /// What is wrong.
    pub reason: &'static str,
}

/// How a file that is not strictly a protocol file was treated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrayKind {
    /// Outside the grammar: ignored.
    Ignored,
    /// A copy that authenticates and duplicates a protocol file: deduplicated.
    DuplicateCopy,
    /// A copy that authenticates but has no protocol file counterpart: report-only.
    CopyOnly,
    /// A copy that authenticates and contradicts a verified entry: evidence of a fork.
    ForkEvidence,
    /// A copy that does not authenticate (half-synced): not yet arrived.
    NotYetArrived,
}

/// A file seen in the listing that is not a protocol file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrayFile {
    /// Name relative to the sync root.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Treatment.
    pub kind: StrayKind,
}

/// What one [`Syncer::sync`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Local edits turned into journal entries.
    pub intents_created: usize,
    /// Objects uploaded.
    pub objects_pushed: usize,
    /// Log segments uploaded and confirmed.
    pub segments_uploaded: usize,
    /// Foreign entries applied to the version graph.
    pub entries_applied: usize,
    /// Objects fetched and verified.
    pub objects_fetched: usize,
    /// Notes fast-forwarded to a remote revision (including remote deletes).
    pub fast_forwards: usize,
    /// Automatic merges written.
    pub auto_merges: usize,
    /// Notes in conflict after the cycle.
    pub conflicts: usize,
    /// Heads whose content is not yet available (dangling).
    pub dangling: usize,
    /// Cached old revisions removed from the local `revs/` cache (section 6.1).
    pub revs_pruned: usize,
    /// Notes awaiting attention after a fork freeze.
    pub needs_attention: usize,
    /// Warnings.
    pub warnings: Vec<Warning>,
    /// Invalid entries or segments.
    pub invalid: Vec<InvalidEntry>,
    /// Files outside the strict grammar.
    pub strays: Vec<StrayFile>,
}

/// The sync engine for one vault and one provider.
pub struct Syncer<P> {
    pub(crate) provider: P,
    pub(crate) state: State,
    pub(crate) cfg: SyncConfig,
    pub(crate) crash: Option<CrashPoint>,
    /// Which note introduced each known revision (rebuilt from the notes state).
    pub(crate) rev_note: HashMap<RevId, Id16>,
    pub(crate) report: SyncReport,
    pub(crate) touched: BTreeSet<Id16>,
}

impl<P> core::fmt::Debug for Syncer<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Syncer(device {})", self.state.device_id)
    }
}

impl<P: Provider> Syncer<P> {
    /// Open (or initialise) the sync state of an unlocked vault.
    ///
    /// # Errors
    /// I/O failures or a corrupt journal.
    pub fn open(vault: &Unlocked, provider: P) -> Result<Self> {
        Self::open_with(vault, provider, SyncConfig::default())
    }

    /// Like [`Syncer::open`] with explicit tunables.
    ///
    /// # Errors
    /// I/O failures or a corrupt journal.
    pub fn open_with(vault: &Unlocked, provider: P, cfg: SyncConfig) -> Result<Self> {
        let dir = vault.root().join(META_DIR).join(STATE_DIR);
        let mut state = State::load(&dir, vault.keys())?;
        // The journal names the device it belongs to: if a crash interrupted a device id
        // change, the journal wins over the id file.
        match state.journal.device {
            Some(d) if d != state.device_id => state.set_device_id(d)?,
            Some(_) => {}
            None => {
                state.journal.device = Some(state.device_id);
                state.touch_journal();
            }
        }
        let mut rev_note = HashMap::new();
        for (note, rec) in &state.notes {
            for rev in rec.graph.vertices.keys() {
                rev_note.insert(*rev, *note);
            }
        }
        Ok(Self {
            provider,
            state,
            cfg,
            crash: None,
            rev_note,
            report: SyncReport::default(),
            touched: BTreeSet::new(),
        })
    }

    /// This device's id (it changes only after a fork of the own id is recovered).
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        self.state.device_id
    }

    /// The provider.
    #[must_use]
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Arm a simulated crash: the next cycle returns [`Error::Crash`] when it reaches `point`.
    /// For tests; the caller then drops the syncer, which loses everything not yet persisted.
    pub fn arm_crash(&mut self, point: Option<CrashPoint>) {
        self.crash = point;
    }

    pub(crate) fn crash_at(&self, point: CrashPoint) -> Result<()> {
        if self.crash == Some(point) {
            Err(Error::Crash(point))
        } else {
            Ok(())
        }
    }

    /// Run one sync cycle.
    ///
    /// Safe to interrupt anywhere and to repeat: a cycle with nothing to do changes nothing.
    ///
    /// # Errors
    /// Provider, storage or crypto failures. Local state stays consistent; run again.
    pub fn sync(&mut self, vault: &mut Unlocked) -> Result<SyncReport> {
        self.report = SyncReport::default();
        self.touched.clear();
        if let Some(moved_to) = self.state.journal_moved_to.take() {
            self.report
                .warnings
                .push(Warning::JournalReset { moved_to });
        }
        self.ensure_own_chain(vault)?;
        self.sync_headers(vault)?;
        if !self.state.rebuilt {
            self.record_local_changes(vault)?;
        }
        self.state.save(vault.keys())?;
        self.crash_at(CrashPoint::AfterScan)?;
        self.push_objects(&mut *vault)?;
        self.crash_at(CrashPoint::AfterObjectsPushed)?;
        self.seal_and_upload(vault)?;
        self.crash_at(CrashPoint::AfterUpload)?;
        let listing = self.provider.list_cycle()?;
        self.crash_at(CrashPoint::AfterList)?;
        self.pull_logs(vault, &listing)?;
        self.state.save(vault.keys())?;
        self.crash_at(CrashPoint::AfterLogsPulled)?;
        self.pull_objects(vault, &listing)?;
        self.crash_at(CrashPoint::AfterObjectsPulled)?;
        self.record_local_changes(vault)?;
        self.state.save(vault.keys())?;
        self.reconcile(vault)?;
        self.crash_at(CrashPoint::AfterMerge)?;
        // Anything created by this cycle (scan after the pull, automatic merges, fork
        // re-emission) goes out in the same call.
        self.push_objects(&mut *vault)?;
        self.seal_and_upload(vault)?;
        self.state.rebuilt = false;
        // The local revision cache only ever holds what merging and conflict resolution need;
        // pruning it is local, cheap and loses nothing (everything is refetchable or retained).
        self.report.revs_pruned = self.prune_revs(crate::gc::DEFAULT_KEEP_REVISIONS, false)?.0;
        self.state.save(vault.keys())?;
        self.finish_report();
        Ok(std::mem::take(&mut self.report))
    }

    fn finish_report(&mut self) {
        let mut conflicts = 0;
        let mut dangling = 0;
        let mut attention = 0;
        for rec in self.state.notes.values() {
            if rec.needs_attention {
                attention += 1;
            }
            let heads = rec.graph.heads();
            if heads.len() > 1 {
                conflicts += 1;
            }
            for h in &heads {
                let v = &rec.graph.vertices[h];
                if v.is_put() && !self.state.has_rev(h) {
                    dangling += 1;
                }
            }
        }
        self.report.conflicts = conflicts;
        self.report.dangling = dangling;
        self.report.needs_attention = attention;
    }
}
