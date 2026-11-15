//! UI-independent application logic of Nuvoletta (`PLAN.md` Phase 3).
//!
//! The Dioxus app in `apps/nuvoletta-app` is a thin view over [`App`]. Everything that decides
//! behaviour lives here so it can be tested without a webview: the session state machine
//! (no vault, locked, unlocked), the open-note buffer with dirty tracking, autosave with a
//! debounce, the inactivity timer, index upkeep, queries, preview rendering, and export/import.
//! Time comes from an injectable [`Clock`], so tests never sleep.
//!
//! Plaintext handling: the buffer and the index own note plaintext and zeroize it on drop (best
//! effort). Locking drops the unlocked vault, the index and the buffer. The editor in the
//! webview holds a copy that cannot be zeroized; the UI clears it on lock (`docs/THREAT_MODEL.md`).

#![forbid(unsafe_code)]

mod app;
mod clock;
mod csp;
mod diff;
mod error;
mod links;
mod settings;
mod sync;
mod sync_messages;
mod sync_settings;
mod sync_types;
mod tree;

pub use app::{
    App, DEFAULT_AUTOSAVE_DEBOUNCE, NoteInfo, NoteRef, Phase, SearchResult, TickOutcome,
    UnresolvedRef,
};
pub use clock::{Clock, ManualClock, SystemClock};
pub use csp::CONTENT_SECURITY_POLICY;
pub use diff::{DiffKind, DiffLine, diff_lines};
pub use error::{Error, Result};
pub use links::{LinkAction, classify_link};
pub use settings::{DEFAULT_AUTO_LOCK_MINUTES, MAX_AUTO_LOCK_MINUTES, Settings, parse_tags};
pub use sync::SyncTiming;
pub use sync_settings::{SyncSettings, check_sync_folder};
pub use sync_types::{
    AttentionItem, AttentionReason, CleanupSummary, ConflictAction, ConflictItem, ConflictKind,
    HeadDetail, HeadView, InvalidInfo, ResolveOutcome, StrayInfo, SyncEvent, SyncFinished,
    SyncNotice, SyncProblem, SyncState, SyncStatus,
};
pub use tree::{TreeRow, build_tree};
