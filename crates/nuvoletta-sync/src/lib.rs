//! Sync engine of Nuvoletta (`docs/SYNC_PROTOCOL.md`).
//!
//! Layers: a [`Provider`] (dumb store of immutable files, with the [`FolderProvider`] backend),
//! the strict [`names`] grammar, per-device hash-chained [`log`]s, a per-note version DAG
//! ([`graph`]), metadata-only [`merge`], encrypted local sync [`state`], and the [`Syncer`] that
//! runs the idempotent sync cycle against a vault from `nuvoletta-storage`.
//!
//! The engine never overwrites or deletes a remote file, never discards a revision and never
//! resolves a real conflict silently; see the protocol document for the full rules.

#![forbid(unsafe_code)]

mod engine;
pub mod error;
pub mod folder;
mod fork;
mod gc;
pub mod graph;
pub mod ids;
mod local;
pub mod log;
pub mod merge;
pub mod names;
pub mod provider;
mod pull;
mod push;
mod reconcile;
mod resolve;
pub mod state;
mod trash;

pub use engine::{InvalidEntry, StrayFile, StrayKind, SyncConfig, SyncReport, Syncer, Warning};
pub use error::{Error, Result};
pub use folder::FolderProvider;
pub use gc::{DEFAULT_KEEP_REVISIONS, GcKept, GcOptions, GcReport, GcSkip};
pub use ids::{DeviceId, Hash32, Id16, RevId};
pub use provider::{Capabilities, Provider, ProviderError, RemoteEntry};
pub use resolve::{Conflict, HeadInfo, NoteStatus};
pub use trash::DeletedNote;

/// Points of the sync cycle at which tests can simulate a crash (see [`Syncer::arm_crash`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CrashPoint {
    /// After scanning local changes and persisting the journal.
    AfterScan,
    /// After uploading objects, before sealing the segment.
    AfterObjectsPushed,
    /// After sealing a segment and persisting its bytes, before uploading it.
    AfterSealed,
    /// After the segment is uploaded and visible, before it is confirmed in the journal.
    BeforeConfirm,
    /// After the push half of the cycle.
    AfterUpload,
    /// After the remote listing.
    AfterList,
    /// After foreign logs were applied and persisted.
    AfterLogsPulled,
    /// After objects were fetched.
    AfterObjectsPulled,
    /// After the plan was persisted, before the vault was written.
    AfterPlanSaved,
    /// After the vault was written, before the final persist.
    AfterMerge,
}
