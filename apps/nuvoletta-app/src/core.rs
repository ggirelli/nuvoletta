//! Shared handle to the application core.

use dioxus::prelude::*;
use nuvoletta_app_core::{App, Settings, SystemClock};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// The application core plus a revision signal. Components call [`Core::read`] while rendering,
/// which subscribes them to the revision; code that changes state calls [`Core::act`], which
/// bumps it. [`Core::quiet`] changes state without re-rendering (activity pings, keystrokes).
#[derive(Clone)]
pub struct Core {
    app: Rc<RefCell<App>>,
    rev: Signal<u64>,
    settings_path: Rc<PathBuf>,
}

impl Core {
    /// Build the core with settings loaded from the user's config directory.
    pub fn new(rev: Signal<u64>) -> Self {
        let settings_path = crate::platform::settings_path();
        let settings = Settings::load(&settings_path);
        let app = App::new(Arc::new(SystemClock::new()), settings);
        Self {
            app: Rc::new(RefCell::new(app)),
            rev,
            settings_path: Rc::new(settings_path),
        }
    }

    /// Read state while rendering; re-runs the caller when [`Core::act`] changes state.
    pub fn read<R>(&self, f: impl FnOnce(&App) -> R) -> R {
        let _ = (self.rev)();
        f(&self.app.borrow())
    }

    /// Read state without subscribing.
    pub fn peek<R>(&self, f: impl FnOnce(&App) -> R) -> R {
        f(&self.app.borrow())
    }

    /// Change state and re-render subscribers.
    pub fn act<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        let r = f(&mut self.app.borrow_mut());
        self.bump();
        r
    }

    /// Change state without re-rendering.
    pub fn quiet<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        f(&mut self.app.borrow_mut())
    }

    /// Re-render subscribers.
    pub fn bump(&self) {
        let mut rev = self.rev;
        rev += 1;
    }

    /// Write the settings file; failures are reported to the caller, never fatal.
    pub fn save_settings(&self) -> nuvoletta_app_core::Result<()> {
        self.peek(|a| a.settings().save(&self.settings_path))
    }
}
