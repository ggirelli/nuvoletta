//! UI-only state shared between components.

use dioxus::prelude::*;
use zeroize::Zeroizing;

/// A message shown in the banner.
#[derive(Clone, PartialEq)]
pub struct Notice {
    /// Errors use `role="alert"`; others `role="status"`.
    pub error: bool,
    /// The text.
    pub text: String,
}

/// Signals that are not application state.
#[derive(Clone, Copy)]
pub struct Ui {
    /// Banner message.
    pub notice: Signal<Option<Notice>>,
    /// A freshly created or rotated recovery key to show once.
    pub recovery_key: Signal<Option<Zeroizing<String>>>,
    /// Settings dialog visible.
    pub settings: Signal<bool>,
    /// Why the vault could not be locked (unsaved edits); shows the decision dialog.
    pub lock_blocked: Signal<Option<String>>,
    /// Preview pane visible.
    pub preview: Signal<bool>,
    /// Search box content.
    pub query: Signal<String>,
    /// Tag filter.
    pub tag: Signal<Option<String>>,
    /// Bumped on every edit so the preview re-renders without re-rendering everything else.
    pub edit_tick: Signal<u64>,
    /// Note pending delete confirmation.
    pub confirm_delete: Signal<Option<nuvoletta_core::NoteId>>,
    /// Narrow screens show the note list or the open note, not both: true shows the list.
    /// Ignored on wide screens, where both are visible.
    pub list_open: Signal<bool>,
    /// Sync dialog visible.
    pub sync: Signal<bool>,
    /// Conflicting note being reviewed inside the sync dialog.
    pub conflict: Signal<Option<nuvoletta_core::NoteId>>,
    /// Recently deleted dialog visible.
    pub trash: Signal<bool>,
    /// Wall-clock time (ms), refreshed periodically so relative times stay current.
    pub clock_ms: Signal<i64>,
}

impl Ui {
    /// Show an error.
    pub fn error(mut self, text: impl Into<String>) {
        self.notice.set(Some(Notice {
            error: true,
            text: text.into(),
        }));
    }

    /// Show an informational message.
    pub fn info(mut self, text: impl Into<String>) {
        self.notice.set(Some(Notice {
            error: false,
            text: text.into(),
        }));
    }

    /// Forget everything that mirrors vault content (on lock).
    pub fn reset_for_lock(mut self) {
        self.list_open.set(true);
        self.query.set(String::new());
        self.tag.set(None);
        self.settings.set(false);
        self.sync.set(false);
        self.trash.set(false);
        self.conflict.set(None);
        self.confirm_delete.set(None);
        self.recovery_key.set(None);
        self.notice.set(None);
    }
}
