//! Plain data types of the sync view: status, notices, conflicts, diagnostics.

use crate::diff::DiffLine;
use nuvoletta_core::NoteId;
use std::path::PathBuf;

/// What the sync machinery is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Sync is off (or no folder is chosen), or the vault is not unlocked.
    Disabled,
    /// Sync is on and no cycle is running.
    Idle,
    /// A cycle is running on the worker thread.
    Syncing,
}

/// Why the last cycle failed, in words a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncProblem {
    /// What went wrong.
    pub message: String,
    /// What to do about it.
    pub hint: String,
}

/// A summary of sync for the status area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStatus {
    /// What sync is doing.
    pub state: SyncState,
    /// The sync folder, if one is chosen.
    pub folder: Option<PathBuf>,
    /// Wall-clock time (ms since the epoch) of the last successful cycle in this session.
    pub last_synced_ms: Option<i64>,
    /// Notes whose latest local change has not been confirmed on the sync folder yet.
    pub pending_objects: usize,
    /// Notes with two or more conflicting versions.
    pub conflicts: usize,
    /// Notes that need attention for other reasons (fork freeze, content not yet arrived).
    pub needs_attention: usize,
    /// Integrity notices the user has not acknowledged.
    pub notices: usize,
    /// The last cycle's failure, cleared by the next success.
    pub problem: Option<SyncProblem>,
    /// A lock was requested and happens when the running cycle ends.
    pub lock_pending: bool,
}

/// An integrity warning. Kept (and stored on this device) until the user acknowledges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncNotice {
    /// Stable machine name, e.g. `rollback`, `device-fork`.
    pub code: String,
    /// One-line headline.
    pub title: String,
    /// What it means and what to do.
    pub detail: String,
    /// When it was first seen (ms since the epoch).
    pub first_seen_ms: i64,
}

/// A file in the sync folder that is not a protocol file (report only; never touched).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrayInfo {
    /// Name relative to the sync folder.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// How the engine treated it.
    pub treatment: String,
}

/// A protocol entry or segment that authenticated but is invalid (reported, never applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidInfo {
    /// Abbreviated writer device id.
    pub device: String,
    /// Sequence number (0: the whole segment).
    pub seq: u64,
    /// What is wrong.
    pub reason: String,
}

/// Why a note is listed as needing attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionReason {
    /// A fork freeze removed the history behind the local content.
    Frozen,
    /// The newest version's content has not arrived yet (it is retried every cycle).
    NotArrived,
}

/// A note that needs attention but is not a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionItem {
    /// The note.
    pub note: NoteId,
    /// Its title.
    pub title: String,
    /// Its path.
    pub path: String,
    /// Why.
    pub reason: AttentionReason,
}

/// One version (head) of a conflicting note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadView {
    /// Revision id (32 hex characters), used to name it in actions.
    pub rev: String,
    /// This version deletes the note.
    pub is_delete: bool,
    /// This is the version currently in this device's vault.
    pub is_local: bool,
    /// Abbreviated ids of the devices that wrote it.
    pub devices: Vec<String>,
    /// Informational time (ms since the epoch); the display order does not depend on it.
    pub time_ms: i64,
    /// Its content has arrived (always true for a deletion).
    pub available: bool,
}

/// What kind of conflict a note is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Two or more edited versions.
    Edits,
    /// A deletion against an edit.
    DeleteVsEdit,
}

/// A note with conflicting versions, heads in the deterministic display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictItem {
    /// The note.
    pub note: NoteId,
    /// Its title.
    pub title: String,
    /// Its path.
    pub path: String,
    /// The kind of conflict.
    pub kind: ConflictKind,
    /// The versions.
    pub heads: Vec<HeadView>,
}

/// A version's content and how it differs from this device's current version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadDetail {
    /// The version.
    pub head: HeadView,
    /// Title of this version (`None` for a deletion).
    pub title: Option<String>,
    /// Path of this version.
    pub path: Option<String>,
    /// Tags of this version.
    pub tags: Vec<String>,
    /// Body of this version (`None` for a deletion).
    pub body: Option<String>,
    /// Line diff from this device's current version to this one; empty if they are equal.
    pub diff: Vec<DiffLine>,
    /// This device has no current version of the note (it is deleted here).
    pub no_local_version: bool,
}

/// A user's resolution of a conflict. Versions are named by [`HeadView::rev`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictAction {
    /// The note becomes that version (or is deleted, if it is a deletion).
    ChooseHead(String),
    /// Delete versus edit: keep the edited note.
    Keep,
    /// Delete versus edit: delete the note (the edited version stays in history).
    Delete,
    /// Keep one version in place and the other as a new "(conflict copy)" note.
    KeepBoth {
        /// Version that stays the note.
        keep: String,
        /// Version that becomes the copy.
        copy: String,
    },
}

/// The result of resolving a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutcome {
    /// The conflict copy created by [`ConflictAction::KeepBoth`].
    pub new_note: Option<NoteId>,
}

/// What happened to the sync cycle (returned by ticks and polls).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    /// A cycle was started on the worker thread.
    Started,
    /// A cycle ended.
    Finished(SyncFinished),
    /// A lock requested during the cycle was carried out.
    Locked,
    /// A lock requested during the cycle was refused (unsaved edits could not be written).
    LockBlocked(String),
}

/// How a cycle ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFinished {
    /// The cycle completed without error.
    pub ok: bool,
    /// Sync changed or removed the open note; the editor must reload it.
    pub open_note_changed: bool,
    /// Something the user must be told about the open note or the sync outcome.
    pub message: Option<String>,
}

/// What "Clean up old versions" would do (a preview) or did (a result). Counts and sizes only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupSummary {
    /// This is a preview: nothing was deleted.
    pub dry_run: bool,
    /// Why nothing can be cleaned up right now, in words a person can act on.
    pub unavailable: Option<String>,
    /// Old versions (files in the sync folder) removed, or that would be.
    pub old_versions: usize,
    /// Bytes they take in the sync folder.
    pub old_version_bytes: u64,
    /// Leftover files of interrupted edits removed, or that would be.
    pub leftovers: usize,
    /// Bytes they take.
    pub leftover_bytes: u64,
    /// Versions kept: the current ones, their direct predecessors, the newest few of every note,
    /// anything recent or not yet seen by every device, and other devices' files.
    pub kept: usize,
    /// Old copies removed from this device's local cache, or that would be.
    pub cache_entries: usize,
}
