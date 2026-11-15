//! Sync integration of the application core: the worker thread that runs a cycle, scheduling,
//! status, integrity notices, conflict viewing and resolution, and diagnostics.
//!
//! **Threading.** A sync cycle needs `&mut Unlocked` for its whole duration (it installs remote
//! versions into the vault), and nothing else may write the vault meanwhile or an edit could be
//! overwritten. So the session *hands the vault and the syncer to a worker thread* and gets
//! them back through a channel. While they are away, reads that need no vault keep working (note
//! list from a snapshot, search, preview, typing in the editor buffer); operations that write or
//! read the vault fail with [`Error::SyncBusy`] and autosave simply waits. The UI thread never
//! blocks on a cycle, except that closing the window waits for it (cycles are short and
//! interruptible by design, but a clean hand-back is what lets the buffer be saved).
//!
//! **Locking.** [`App::lock`] during a cycle records a pending lock; the lock is carried out
//! (saving first, never discarding) when the vault comes back. Nothing syncs while locked: the
//! keys are needed.

use crate::app::{App, Session, State};
use crate::diff::diff_lines;
use crate::error::{Error, Result};
use crate::sync_messages::{
    describe_error, describe_invalid, describe_resolution_error, describe_warning, invalid_info,
    short, stray_info,
};
use crate::sync_settings::{SyncSettings, check_sync_folder, device_state_dir, write_device_file};
use crate::sync_types::{
    AttentionItem, AttentionReason, CleanupSummary, ConflictAction, ConflictItem, ConflictKind,
    HeadDetail, HeadView, InvalidInfo, ResolveOutcome, StrayInfo, SyncEvent, SyncFinished,
    SyncNotice, SyncProblem, SyncState, SyncStatus,
};
use nuvoletta_core::{NoteId, now_ms};
use nuvoletta_storage::{NoteSummary, Unlocked};
use nuvoletta_sync::ids::Id16;
use nuvoletta_sync::state::STATE_DIR;
use nuvoletta_sync::{
    Capabilities, FolderProvider, GcOptions, GcReport, GcSkip, NoteStatus, Provider, ProviderError,
    RemoteEntry, SyncReport, Syncer,
};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod trash;
pub use trash::DeletedItem;

type BoxProvider = Box<dyn Provider + Send>;
type BoxSyncer = Syncer<BoxProvider>;

const NOTICES_FILE: &str = "sync-notices.json";

/// When automatic syncs happen. Defaults: 5 seconds after a save, every 5 minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncTiming {
    /// Quiet time after the last save before a sync starts.
    pub debounce: Duration,
    /// Time between periodic syncs while unlocked.
    pub interval: Duration,
}

impl Default for SyncTiming {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(5),
            interval: Duration::from_secs(300),
        }
    }
}

/// A provider that fails every call: lets conflicts of a vault be viewed and resolved on a device
/// whose sync is off or whose folder is missing, without ever creating a folder.
struct Offline;

impl Offline {
    fn err() -> ProviderError {
        ProviderError::Other("sync is not available".to_owned())
    }
}

impl Provider for Offline {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            create_only: false,
            atomic_rename: false,
            etags: false,
            mtimes: false,
        }
    }
    fn list(&self, _: &str) -> std::result::Result<Vec<RemoteEntry>, ProviderError> {
        Err(Self::err())
    }
    fn get(&self, _: &str) -> std::result::Result<Vec<u8>, ProviderError> {
        Err(Self::err())
    }
    fn put_new(&self, _: &str, _: &[u8]) -> std::result::Result<(), ProviderError> {
        Err(Self::err())
    }
    fn delete(&self, _: &str) -> std::result::Result<(), ProviderError> {
        Err(Self::err())
    }
}

/// What the last cycle (or a resolution) left behind, for the views.
#[derive(Default, Clone)]
pub(crate) struct Snapshot {
    conflicts: Vec<ConflictItem>,
    attention: Vec<AttentionItem>,
    pending: usize,
    strays: Vec<StrayInfo>,
    invalid: Vec<InvalidInfo>,
}

struct Job {
    vault: Unlocked,
    syncer: Box<BoxSyncer>,
}

struct JobResult {
    vault: Unlocked,
    syncer: Box<BoxSyncer>,
    outcome: std::result::Result<SyncReport, nuvoletta_sync::Error>,
    snapshot: Snapshot,
}

/// Sync state of an unlocked session.
#[derive(Default)]
pub(crate) struct SyncSession {
    syncer: Option<Box<BoxSyncer>>,
    job: Option<Receiver<JobResult>>,
    /// A lock was requested during a cycle; the flag is `force`.
    pub(crate) lock_after: Option<bool>,
    snapshot: Snapshot,
    notices: Vec<SyncNotice>,
    problem: Option<SyncProblem>,
    last_synced_ms: Option<i64>,
    /// Time of the last local change not yet handed to a cycle.
    pub(crate) changed_at: Option<Duration>,
    last_started: Option<Duration>,
}

fn open_syncer(
    vault: &Unlocked,
    settings: &SyncSettings,
) -> std::result::Result<Box<BoxSyncer>, nuvoletta_sync::Error> {
    let provider: BoxProvider = match &settings.folder {
        Some(f) if settings.active() && f.is_dir() => match FolderProvider::new(f) {
            Ok(p) => Box::new(p),
            Err(_) => Box::new(Offline),
        },
        _ => Box::new(Offline),
    };
    Syncer::open(vault, provider).map(Box::new)
}

fn load_notices(root: &Path) -> Vec<SyncNotice> {
    let Ok(text) = std::fs::read_to_string(device_state_dir(root).join(NOTICES_FILE)) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return Vec::new();
    };
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    v.iter()
        .map(|n| SyncNotice {
            code: s(n, "code"),
            title: s(n, "title"),
            detail: s(n, "detail"),
            first_seen_ms: n
                .get("first_seen_ms")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        })
        .collect()
}

fn save_notices(root: &Path, notices: &[SyncNotice]) -> Result<()> {
    let v: Vec<_> = notices
        .iter()
        .map(|n| {
            serde_json::json!({
                "code": n.code, "title": n.title, "detail": n.detail, "first_seen_ms": n.first_seen_ms,
            })
        })
        .collect();
    write_device_file(
        root,
        NOTICES_FILE,
        serde_json::Value::Array(v).to_string().as_bytes(),
    )
}

/// Add a notice unless the same one is already shown. Returns whether it was new.
fn add_notice(list: &mut Vec<SyncNotice>, n: SyncNotice) -> bool {
    if list
        .iter()
        .any(|o| o.code == n.code && o.detail == n.detail)
    {
        return false;
    }
    list.push(n);
    true
}

fn build_snapshot(
    syncer: &BoxSyncer,
    vault: &Unlocked,
    strays: Vec<StrayInfo>,
    invalid: Vec<InvalidInfo>,
) -> Snapshot {
    let mut snap = Snapshot {
        strays,
        invalid,
        ..Snapshot::default()
    };
    for c in syncer.conflicts() {
        let local = vault.summary(&c.note);
        let heads: Vec<HeadView> = c
            .heads
            .iter()
            .map(|h| HeadView {
                rev: h.rev.to_string(),
                is_delete: h.is_delete,
                is_local: local.is_some_and(|s| s.revision_id == h.rev.0),
                devices: h.devices.iter().map(short).collect(),
                time_ms: h.time,
                available: h.available,
            })
            .collect();
        let title = local.map(|s| s.title.clone()).or_else(|| {
            c.heads
                .iter()
                .filter(|h| h.available && !h.is_delete)
                .find_map(|h| syncer.read_revision(vault, &c.note, &h.rev).ok())
                .map(|n| n.title.clone())
        });
        snap.conflicts.push(ConflictItem {
            note: c.note,
            title: title.unwrap_or_else(|| format!("Note {}", short(c.note))),
            path: local
                .map(|s| s.path.as_str().to_owned())
                .unwrap_or_default(),
            kind: if heads.iter().any(|h| h.is_delete) {
                ConflictKind::DeleteVsEdit
            } else {
                ConflictKind::Edits
            },
            heads,
        });
    }
    for s in vault.list() {
        let reason = match syncer.note_status(&s.id) {
            NoteStatus::NeedsAttention => AttentionReason::Frozen,
            NoteStatus::Dangling => AttentionReason::NotArrived,
            NoteStatus::Pending => {
                snap.pending += 1;
                continue;
            }
            _ => continue,
        };
        snap.attention.push(AttentionItem {
            note: s.id,
            title: s.title.clone(),
            path: s.path.as_str().to_owned(),
            reason,
        });
    }
    snap
}

fn strays_of(r: &SyncReport) -> (Vec<StrayInfo>, Vec<InvalidInfo>) {
    (
        r.strays.iter().map(stray_info).collect(),
        r.invalid.iter().map(invalid_info).collect(),
    )
}

fn run_job(job: Job) -> JobResult {
    let Job {
        mut vault,
        mut syncer,
    } = job;
    let outcome = catch_unwind(AssertUnwindSafe(|| syncer.sync(&mut vault)))
        .unwrap_or(Err(nuvoletta_sync::Error::State("internal error")));
    let (strays, invalid) = outcome
        .as_ref()
        .map_or_else(|_| (Vec::new(), Vec::new()), strays_of);
    let mut snapshot = build_snapshot(&syncer, &vault, strays, invalid);
    if outcome.is_err() {
        // Keep the last known diagnostics when a cycle fails before listing the folder.
        snapshot.strays.clear();
    }
    JobResult {
        vault,
        syncer,
        outcome,
        snapshot,
    }
}

impl Session {
    /// Replace the syncer to match the settings (not while a cycle runs).
    fn reopen_syncer(&mut self, settings: &SyncSettings) -> Result<()> {
        let Some(vault) = self.vault.as_ref() else {
            return Err(Error::SyncBusy);
        };
        let ever_used = device_state_dir(&self.root)
            .parent()
            .is_some_and(|m| m.join(STATE_DIR).exists());
        self.sync.syncer = if settings.active() || ever_used {
            Some(
                open_syncer(vault, settings)
                    .map_err(|e| Error::Sync(describe_error(&e).message))?,
            )
        } else {
            None
        };
        Ok(())
    }

    /// Bring the index, the open note and the change flags in line with a vault that sync or a
    /// resolution changed. Returns `(open note must be reloaded, message)`.
    fn reconcile_with_vault(&mut self, before: &[NoteSummary]) -> (bool, Option<String>) {
        let Some(vault) = self.vault.as_mut() else {
            return (false, None);
        };
        let old: HashMap<NoteId, [u8; 16]> = before.iter().map(|n| (n.id, n.revision_id)).collect();
        let after = vault.list();
        let now: HashSet<NoteId> = after.iter().map(|n| n.id).collect();
        let changed: Vec<NoteId> = after
            .iter()
            .filter(|n| old.get(&n.id) != Some(&n.revision_id))
            .map(|n| n.id)
            .collect();
        let removed: Vec<NoteId> = old.keys().filter(|id| !now.contains(id)).copied().collect();
        for id in &changed {
            if let Ok(n) = vault.read_note(id) {
                self.index.upsert(&n);
            }
        }
        for id in &removed {
            self.index.remove(*id);
        }
        let Some(o) = self.open.as_mut() else {
            return (false, None);
        };
        let id = o.note.id;
        if removed.contains(&id) {
            if o.dirty() {
                // Keep the unsaved edits: write them back. The next cycle sees a new edit of a
                // deleted note and shows a visible conflict; nothing is discarded.
                let mut n = o.note.clone();
                n.body = (*o.buffer).clone();
                n.modified_at = now_ms();
                return match vault.create_note(&n) {
                    Ok(()) => {
                        self.index.upsert(&n);
                        o.note = n;
                        self.sync.changed_at = Some(Duration::ZERO);
                        (false, Some("This note was deleted on another device while you were editing it. Your edits were kept and the note was restored; the next sync shows it under conflicts.".to_owned()))
                    }
                    Err(e) => {
                        o.save_error = Some(Error::from(e).user_message());
                        (false, Some("This note was deleted on another device while you were editing it. Your edits are still in the editor but could not be saved.".to_owned()))
                    }
                };
            }
            self.open = None;
            return (
                true,
                Some("The note you had open was deleted on another device.".to_owned()),
            );
        }
        if changed.contains(&id)
            && let Ok(new) = vault.read_note(&id)
        {
            if o.dirty() {
                o.note = new;
                o.changed_by_sync = true;
                return (
                    false,
                    Some("This note was updated by sync while you were editing it. Your edits are still in the editor; saving replaces the synced version (which stays in sync history).".to_owned()),
                );
            }
            o.buffer = zeroize::Zeroizing::new(new.body.clone());
            o.note = new;
            o.generation += 1;
            return (true, None);
        }
        (false, None)
    }
}

impl App {
    // ---- settings ---------------------------------------------------------------------------

    /// This vault's sync settings on this device.
    #[must_use]
    pub fn sync_settings(&self) -> &SyncSettings {
        &self.sync_cfg
    }

    /// Change the timing of automatic syncs (tests and tuning).
    pub fn set_sync_timing(&mut self, timing: SyncTiming) {
        self.sync_timing = timing;
    }

    /// Choose the externally synced folder and turn sync on. The folder must exist and be
    /// separate from the vault. Changing to a *different* folder than before makes the remote
    /// look empty or older to the engine, which reports it (rollback notices); the UI warns.
    ///
    /// # Errors
    /// [`Error::Invalid`] for an unusable folder, [`Error::SyncBusy`] during a cycle,
    /// [`Error::Settings`] if the setting cannot be stored.
    pub fn configure_sync(&mut self, folder: &Path) -> Result<()> {
        let root = self.session()?.root.clone();
        let folder = check_sync_folder(&root, folder)?;
        let next = SyncSettings {
            enabled: true,
            folder: Some(folder),
        };
        self.apply_sync_settings(next)
    }

    /// Turn sync on or off, keeping the chosen folder. Turning it on needs a folder.
    ///
    /// # Errors
    /// [`Error::SyncNotConfigured`] (no folder), [`Error::SyncBusy`], [`Error::Settings`].
    pub fn set_sync_enabled(&mut self, enabled: bool) -> Result<()> {
        let mut next = self.sync_cfg.clone();
        if enabled && next.folder.is_none() {
            return Err(Error::SyncNotConfigured);
        }
        next.enabled = enabled;
        self.apply_sync_settings(next)
    }

    fn apply_sync_settings(&mut self, next: SyncSettings) -> Result<()> {
        let s = self.session()?;
        if s.vault.is_none() {
            return Err(Error::SyncBusy);
        }
        next.save(&s.root)?;
        self.sync_cfg = next.clone();
        let s = self.session_mut()?;
        s.sync.last_started = None;
        s.sync.problem = None;
        s.reopen_syncer(&next)
    }

    // ---- life cycle hooks -------------------------------------------------------------------

    /// Called when a vault is unlocked: load settings and notices, open the syncer if sync was
    /// ever used here.
    pub(crate) fn sync_on_unlock(&mut self) {
        let State::Unlocked(s) = &mut self.state else {
            return;
        };
        self.sync_cfg = SyncSettings::load(&s.root);
        s.sync.notices = load_notices(&s.root);
        let cfg = self.sync_cfg.clone();
        if let Err(e) = s.reopen_syncer(&cfg) {
            s.sync.problem = Some(SyncProblem {
                message: e.user_message(),
                hint: "Sync is paused. Your notes are not affected.".to_owned(),
            });
        } else if let Some(syncer) = s.sync.syncer.as_deref()
            && let Some(vault) = s.vault.as_ref()
        {
            s.sync.snapshot = build_snapshot(syncer, vault, Vec::new(), Vec::new());
        }
    }

    // ---- running ----------------------------------------------------------------------------

    /// Start a sync cycle now (the "Sync now" action). Does nothing if one is already running.
    ///
    /// # Errors
    /// [`Error::SyncNotConfigured`], or [`Error::Sync`] if the folder is unavailable or the
    /// sync state cannot be opened. The reason is also in [`App::sync_status`].
    pub fn sync_now(&mut self) -> Result<()> {
        self.start_job().map(|_| ())
    }

    /// Start a cycle if automatic sync is due. Called by [`App::tick`].
    pub(crate) fn maybe_start_sync(&mut self) -> bool {
        if !self.sync_cfg.active() {
            return false;
        }
        let (now, t) = (self.clock_now(), self.sync_timing);
        let Ok(s) = self.session() else {
            return false;
        };
        let idle = s.vault.is_some()
            && s.sync.job.is_none()
            && s.sync.lock_after.is_none()
            && !s.must_change_passphrase
            && !s.open.as_ref().is_some_and(crate::app::OpenNote::dirty);
        let after = |t0: Option<Duration>, wait: Duration| {
            t0.is_some_and(|t0| now.saturating_sub(t0) >= wait)
        };
        let due = after(s.sync.changed_at, t.debounce)
            || s.sync.last_started.is_none()
            || after(s.sync.last_started, t.interval);
        // An error is already recorded in the status; the next attempt is on the schedule.
        idle && due && self.start_job().unwrap_or(false)
    }

    fn start_job(&mut self) -> Result<bool> {
        if !self.sync_cfg.active() {
            return Err(Error::SyncNotConfigured);
        }
        let cfg = self.sync_cfg.clone();
        let now = self.clock_now();
        let s = self.session_mut()?;
        if s.sync.job.is_some() || s.must_change_passphrase {
            return Ok(false);
        }
        s.sync.last_started = Some(now);
        s.sync.changed_at = None;
        let fail = |s: &mut Session, p: SyncProblem| {
            let msg = format!("{} {}", p.message, p.hint);
            s.sync.problem = Some(p);
            Error::Sync(msg)
        };
        if !cfg.folder.as_deref().is_some_and(Path::is_dir) {
            let shown = cfg.folder.as_deref().map(|p| p.display().to_string());
            return Err(fail(
                s,
                SyncProblem {
                    message: format!(
                        "The sync folder {} is missing.",
                        shown.unwrap_or_default()
                    ),
                    hint: "If it is on a removable or network drive, reconnect it. Sync resumes automatically."
                        .to_owned(),
                },
            ));
        }
        // Unsaved edits go to disk first so the cycle sees them; a failure is not fatal (the
        // buffer stays and is saved after the cycle).
        let _ = s.save(now);
        if s.sync.syncer.is_none() {
            s.reopen_syncer(&cfg)?;
        }
        // The syncer must point at the folder (it may have been opened as `Offline`).
        let folder_ok = s.sync.syncer.is_some();
        if !folder_ok {
            return Err(Error::SyncNotConfigured);
        }
        let (Some(vault), Some(syncer)) = (s.vault.take(), s.sync.syncer.take()) else {
            return Ok(false);
        };
        s.listing = vault.list();
        let slot = Arc::new(Mutex::new(Some(Job { vault, syncer })));
        let (tx, rx) = channel();
        let worker = Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("nuvoletta-sync".to_owned())
            .spawn(move || {
                let job = worker.lock().ok().and_then(|mut g| g.take());
                if let Some(job) = job {
                    let _ = tx.send(run_job(job));
                }
            });
        if spawned.is_err() {
            // No thread: take everything back and report it.
            if let Some(Job { vault, syncer }) = slot.lock().ok().and_then(|mut g| g.take()) {
                s.vault = Some(vault);
                s.sync.syncer = Some(syncer);
            }
            return Err(fail(
                s,
                SyncProblem {
                    message: "Sync could not start a background task.".to_owned(),
                    hint: "Try again.".to_owned(),
                },
            ));
        }
        s.sync.job = Some(rx);
        Ok(true)
    }

    /// If a cycle has finished, take its result and return what happened.
    pub fn poll_sync(&mut self) -> Option<SyncEvent> {
        let State::Unlocked(s) = &mut self.state else {
            return None;
        };
        let received = match s.sync.job.as_ref()?.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        s.sync.job = None;
        Some(match received {
            Some(r) => self.finish_job(r),
            None => self.job_lost(),
        })
    }

    /// Block until a running cycle (if any) has finished and its result is applied. Used when
    /// the window closes, and by tests.
    pub fn wait_sync(&mut self) -> Option<SyncEvent> {
        let State::Unlocked(s) = &mut self.state else {
            return None;
        };
        let rx = s.sync.job.take()?;
        Some(match rx.recv() {
            Ok(r) => self.finish_job(r),
            Err(_) => self.job_lost(),
        })
    }

    /// Whether a cycle is running.
    #[must_use]
    pub fn sync_in_flight(&self) -> bool {
        self.session().is_ok_and(|s| s.sync.job.is_some())
    }

    /// Whether a lock is waiting for the running cycle to end.
    #[must_use]
    pub fn lock_pending(&self) -> bool {
        self.session().is_ok_and(|s| s.sync.lock_after.is_some())
    }

    /// The worker vanished without handing the vault back (it cannot normally happen: panics
    /// are caught). The keys are gone with it, so fall back to the locked screen.
    fn job_lost(&mut self) -> SyncEvent {
        let root = self.session().map(|s| s.root.clone()).ok();
        self.state = State::NoVault;
        if let Some(root) = root
            && let Ok(l) = nuvoletta_storage::Locked::open(&root)
        {
            self.state = State::Locked(Box::new(l));
        }
        SyncEvent::LockBlocked(
            "Sync stopped unexpectedly and the vault was locked. Unsaved edits could not be saved."
                .to_owned(),
        )
    }

    fn finish_job(&mut self, r: JobResult) -> SyncEvent {
        let JobResult {
            vault,
            syncer,
            outcome,
            snapshot,
        } = r;
        let State::Unlocked(s) = &mut self.state else {
            return SyncEvent::Finished(SyncFinished {
                ok: false,
                open_note_changed: false,
                message: None,
            });
        };
        let before = std::mem::take(&mut s.listing);
        s.vault = Some(vault);
        s.sync.syncer = Some(syncer);
        let prior = std::mem::replace(&mut s.sync.snapshot, snapshot);
        let ok = match outcome {
            Ok(report) => {
                s.sync.problem = None;
                s.sync.last_synced_ms = Some(now_ms());
                let mut changed = false;
                for w in &report.warnings {
                    changed |= add_notice(&mut s.sync.notices, describe_warning(w, now_ms()));
                }
                for e in &report.invalid {
                    changed |= add_notice(&mut s.sync.notices, describe_invalid(e, now_ms()));
                }
                if changed {
                    let _ = save_notices(&s.root, &s.sync.notices);
                }
                true
            }
            Err(e) => {
                s.sync.problem = Some(describe_error(&e));
                // A failed cycle did not list the folder: keep the last diagnostics.
                s.sync.snapshot.strays = prior.strays;
                s.sync.snapshot.invalid = prior.invalid;
                false
            }
        };
        let (open_note_changed, message) = s.reconcile_with_vault(&before);
        if let Some(force) = s.sync.lock_after.take() {
            return match self.lock(force) {
                Ok(()) => SyncEvent::Locked,
                Err(e) => SyncEvent::LockBlocked(e.user_message()),
            };
        }
        SyncEvent::Finished(SyncFinished {
            ok,
            open_note_changed,
            message,
        })
    }

    // ---- status and diagnostics -------------------------------------------------------------

    /// The sync summary for the status area.
    #[must_use]
    pub fn sync_status(&self) -> SyncStatus {
        let cfg = &self.sync_cfg;
        let Ok(s) = self.session() else {
            return SyncStatus {
                state: SyncState::Disabled,
                folder: cfg.folder.clone(),
                last_synced_ms: None,
                pending_objects: 0,
                conflicts: 0,
                needs_attention: 0,
                notices: 0,
                problem: None,
                lock_pending: false,
            };
        };
        let state = if s.sync.job.is_some() {
            SyncState::Syncing
        } else if cfg.active() {
            SyncState::Idle
        } else {
            SyncState::Disabled
        };
        SyncStatus {
            state,
            folder: cfg.folder.clone(),
            last_synced_ms: s.sync.last_synced_ms,
            pending_objects: s.sync.snapshot.pending,
            conflicts: s.sync.snapshot.conflicts.len(),
            needs_attention: s.sync.snapshot.attention.len(),
            notices: s.sync.notices.len(),
            problem: s.sync.problem.clone(),
            lock_pending: s.sync.lock_after.is_some(),
        }
    }

    /// Integrity warnings not yet acknowledged, oldest first. They stay (also across restarts)
    /// until [`App::acknowledge_sync_notice`]; a later cycle never removes them.
    #[must_use]
    pub fn sync_notices(&self) -> Vec<SyncNotice> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.sync.notices.clone())
    }

    /// Acknowledge (remove) the notice at `index` of [`App::sync_notices`]. An explicit user
    /// action: a notice that is raised again later reappears.
    ///
    /// # Errors
    /// [`Error::Invalid`] for an unknown index; [`Error::Settings`] if storing fails.
    pub fn acknowledge_sync_notice(&mut self, index: usize) -> Result<()> {
        let s = self.session_mut()?;
        if index >= s.sync.notices.len() {
            return Err(Error::Invalid("that notice no longer exists"));
        }
        s.sync.notices.remove(index);
        save_notices(&s.root, &s.sync.notices)
    }

    /// Notes with conflicting versions, as of the last cycle or resolution.
    #[must_use]
    pub fn sync_conflicts(&self) -> Vec<ConflictItem> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.sync.snapshot.conflicts.clone())
    }

    /// Notes that need attention without being conflicts.
    #[must_use]
    pub fn sync_attention(&self) -> Vec<AttentionItem> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.sync.snapshot.attention.clone())
    }

    /// Files in the sync folder that are not protocol files (report only).
    #[must_use]
    pub fn sync_strays(&self) -> Vec<StrayInfo> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.sync.snapshot.strays.clone())
    }

    /// Invalid entries or segments seen in the last cycle (report only).
    #[must_use]
    pub fn sync_invalid(&self) -> Vec<InvalidInfo> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.sync.snapshot.invalid.clone())
    }

    // ---- conflicts --------------------------------------------------------------------------

    /// One version of a conflicting note, with a line diff from this device's current version.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle; [`Error::Sync`] if the version is unknown or has not
    /// arrived.
    pub fn conflict_head(&self, note: NoteId, rev: &str) -> Result<HeadDetail> {
        let s = self.session()?;
        let (vault, syncer) = match (s.vault.as_ref(), s.sync.syncer.as_deref()) {
            (Some(v), Some(sy)) => (v, sy),
            (None, _) => return Err(Error::SyncBusy),
            _ => return Err(Error::SyncNotConfigured),
        };
        let item = s
            .sync
            .snapshot
            .conflicts
            .iter()
            .find(|c| c.note == note)
            .ok_or_else(|| Error::Sync("this note is no longer in conflict".to_owned()))?;
        let head = item
            .heads
            .iter()
            .find(|h| h.rev == rev)
            .ok_or_else(|| Error::Sync("that version no longer exists".to_owned()))?
            .clone();
        let local = vault.read_note(&note).ok();
        if head.is_delete {
            let diff = local
                .as_ref()
                .map(|n| diff_lines(&n.body, ""))
                .unwrap_or_default();
            return Ok(HeadDetail {
                head,
                title: None,
                path: None,
                tags: Vec::new(),
                body: None,
                diff,
                no_local_version: local.is_none(),
            });
        }
        let rev_id = Id16::from_hex(rev).ok_or_else(|| Error::Sync("bad version id".to_owned()))?;
        let n = syncer
            .read_revision(vault, &note, &rev_id)
            .map_err(|e| Error::Sync(describe_resolution_error(&e)))?;
        let diff = match &local {
            Some(l) if l.body == n.body => Vec::new(),
            Some(l) => diff_lines(&l.body, &n.body),
            None => diff_lines("", &n.body),
        };
        Ok(HeadDetail {
            head,
            title: Some(n.title.clone()),
            path: Some(n.path.as_str().to_owned()),
            tags: n.tags.clone(),
            body: Some(n.body.clone()),
            diff,
            no_local_version: local.is_none(),
        })
    }

    /// Resolve a conflict. Every action writes a new version whose parents are all current
    /// versions, so nothing is discarded and the conflict disappears on every device. Unsaved
    /// edits of the open note are saved first. To merge by hand, choose the closest version and
    /// then edit the note normally: the edit syncs as an ordinary change.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle; [`Error::Sync`] with an explanation if the versions
    /// changed or a version has not arrived; [`Error::Unsaved`] if the open note cannot be saved.
    pub fn resolve_conflict(
        &mut self,
        note: NoteId,
        action: &ConflictAction,
    ) -> Result<ResolveOutcome> {
        let now = self.clock_now();
        let s = self.session_mut()?;
        if s.vault.is_none() {
            return Err(Error::SyncBusy);
        }
        s.save(now).map_err(|e| Error::Unsaved(Box::new(e)))?;
        let before = s.list();
        let Session { vault, sync, .. } = &mut *s;
        let (Some(vault), Some(syncer)) = (vault.as_mut(), sync.syncer.as_deref_mut()) else {
            return Err(Error::SyncNotConfigured);
        };
        let hex = |h: &str| Id16::from_hex(h);
        let bad = || Error::Sync("bad version id".to_owned());
        let map = |e: nuvoletta_sync::Error| Error::Sync(describe_resolution_error(&e));
        // Make sure edits made since the last cycle are known to the engine before it replaces
        // anything; if that changes the heads, the action below fails and the user reviews again.
        syncer.record_local_changes(vault).map_err(map)?;
        let mut new_note = None;
        let result = match action {
            ConflictAction::ChooseHead(r) => {
                syncer.resolve_choose_head(vault, &note, &hex(r).ok_or_else(bad)?)
            }
            ConflictAction::Keep => syncer.resolve_keep(vault, &note),
            ConflictAction::Delete => syncer.resolve_delete(vault, &note),
            ConflictAction::KeepBoth { keep, copy } => syncer
                .resolve_keep_both(
                    vault,
                    &note,
                    &hex(keep).ok_or_else(bad)?,
                    &hex(copy).ok_or_else(bad)?,
                )
                .map(|n| new_note = Some(n)),
        };
        let strays = std::mem::take(&mut sync.snapshot.strays);
        let invalid = std::mem::take(&mut sync.snapshot.invalid);
        sync.snapshot = build_snapshot(syncer, vault, strays, invalid);
        sync.changed_at = Some(now);
        s.reconcile_with_vault(&before);
        result.map_err(map)?;
        Ok(ResolveOutcome { new_note })
    }
}

fn summarize(r: &GcReport) -> CleanupSummary {
    let unavailable = r.skipped.map(|s| match s {
        GcSkip::Forks => "A device ID was used twice (see the integrity warnings). Nothing is deleted until that is sorted out.".to_owned(),
        GcSkip::Rebuilding => "This device is still rebuilding its sync knowledge. Sync again, then retry.".to_owned(),
    });
    CleanupSummary {
        dry_run: r.dry_run,
        unavailable,
        old_versions: r.objects_collected,
        old_version_bytes: r.bytes_collected,
        leftovers: r.orphans_collected,
        leftover_bytes: r.orphan_bytes,
        kept: r.kept.total(),
        cache_entries: r.revs_pruned,
    }
}

impl App {
    /// What "Clean up old versions" would delete from the sync folder, without deleting
    /// anything. Only versions this device wrote can ever be deleted, and only old ones that
    /// every active device has seen (`SYNC_PROTOCOL.md` section 9). Works from what the last
    /// sync cycle learned; sync first for the freshest picture.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a cycle, [`Error::SyncNotConfigured`] without an available
    /// sync folder, [`Error::Sync`] if the folder cannot be read.
    pub fn cleanup_preview(&mut self) -> Result<CleanupSummary> {
        self.cleanup(true)
    }

    /// Delete old versions from the sync folder (after the user confirmed a preview). Nothing
    /// that is needed to merge, resolve conflicts or restore the newest versions is deleted.
    ///
    /// # Errors
    /// As [`App::cleanup_preview`]; a folder failure stops the cleanup, and repeating it
    /// continues safely.
    pub fn cleanup_run(&mut self) -> Result<CleanupSummary> {
        self.cleanup(false)
    }

    fn cleanup(&mut self, dry_run: bool) -> Result<CleanupSummary> {
        if !self.sync_cfg.active() || !self.sync_cfg.folder.as_deref().is_some_and(Path::is_dir) {
            return Err(Error::SyncNotConfigured);
        }
        let s = self.session_mut()?;
        let (Some(vault), Some(syncer)) = (s.vault.as_ref(), s.sync.syncer.as_deref_mut()) else {
            return Err(Error::SyncBusy);
        };
        let opts = GcOptions {
            dry_run,
            ..GcOptions::default()
        };
        syncer
            .collect_garbage(vault, &opts)
            .map(|r| summarize(&r))
            .map_err(|e| Error::Sync(describe_error(&e).message))
    }
}
