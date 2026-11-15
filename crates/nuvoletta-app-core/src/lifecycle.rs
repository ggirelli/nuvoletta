//! App lifecycle on mobile: what happens when the app leaves the foreground.
//!
//! Policy (`docs/ADR/0006-android-app.md`): when the app goes to the background (home, app
//! switch, screen off, an incoming call) the open note is saved and the vault is locked at once,
//! so keys, the index and the buffer are dropped before the process can be frozen or killed.
//! This is stricter than the inactivity timer on purpose: a backgrounded process cannot be
//! trusted to run timers (Android freezes it, and the monotonic clock does not advance in deep
//! sleep). Nothing is ever discarded: if the save fails the vault stays unlocked and the UI
//! reports it when the app returns.

use crate::app::{App, Phase};

/// What [`App::on_background`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundOutcome {
    /// There was no unlocked vault; nothing to do.
    NotUnlocked,
    /// Unsaved edits were saved and the vault is locked. The UI must drop the editor content.
    Locked,
    /// A sync cycle has the vault; it will be saved and locked when the cycle ends.
    LockPending,
    /// Unsaved edits could not be written, so the vault was **not** locked and nothing was
    /// discarded. Carries the user-facing reason for the dialog shown on return.
    LockBlocked(String),
}

impl App {
    /// The app left the foreground: save, then lock. See the module docs for the policy.
    pub fn on_background(&mut self) -> BackgroundOutcome {
        if self.phase() != Phase::Unlocked {
            return BackgroundOutcome::NotUnlocked;
        }
        match self.lock(false) {
            Ok(()) if self.phase() == Phase::Unlocked => BackgroundOutcome::LockPending,
            Ok(()) => BackgroundOutcome::Locked,
            Err(e) => BackgroundOutcome::LockBlocked(e.user_message()),
        }
    }
}
