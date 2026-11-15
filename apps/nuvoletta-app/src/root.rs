//! Root component: screen selection, JS bridge handling, timers.

use crate::assets;
use crate::bridge;
use crate::core::Core;
use crate::dialogs::{ForcedPassphrase, LockBlocked, RecoveryScreen, Settings};
use crate::editor::Workspace;
use crate::picker::{Picker, Unlock};
use crate::platform::shell::tao::event::{Event, WindowEvent};
use crate::platform::shell::use_wry_event_handler;
use crate::sidebar::Sidebar;
use crate::sync::{SyncControls, SyncDialog};
use crate::trash::TrashDialog;
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{BackgroundOutcome, LinkAction, Phase, TickOutcome, classify_link};
use std::time::Duration;

/// The application root.
#[component]
pub fn Root() -> Element {
    let rev = use_signal(|| 0u64);
    let core = use_context_provider(|| {
        let core = Core::new(rev);
        // Android: the vault is always in the same private place, so go straight to unlocking it.
        // (Not a vault, or missing: the picker is shown to create one.)
        if let Some(dir) = crate::platform::fixed_vault_dir() {
            let _ = core.quiet(|a| a.open_vault(&dir));
        }
        core
    });
    let ui = use_context_provider(|| Ui {
        notice: Signal::new(None),
        recovery_key: Signal::new(None),
        settings: Signal::new(false),
        lock_blocked: Signal::new(None),
        preview: Signal::new(true),
        query: Signal::new(String::new()),
        tag: Signal::new(None),
        edit_tick: Signal::new(0),
        confirm_delete: Signal::new(None),
        list_open: Signal::new(true),
        sync: Signal::new(false),
        conflict: Signal::new(None),
        trash: Signal::new(false),
        clock_ms: Signal::new(crate::sync::wall_ms()),
    });

    // JS -> Rust messages.
    let c = core.clone();
    use_future(move || {
        let c = c.clone();
        async move {
            let mut ev = bridge::connect();
            while let Ok(msg) = ev.recv::<serde_json::Value>().await {
                handle_message(&c, ui, &msg);
            }
        }
    });

    // Once a second while unlocked (autosave and inactivity lock); every few seconds otherwise,
    // when there is nothing to save or lock, to keep the idle app from waking the CPU.
    let c = core.clone();
    use_future(move || {
        let c = c.clone();
        async move {
            loop {
                let unlocked = c.peek(|a| a.phase() == nuvoletta_app_core::Phase::Unlocked);
                tokio::time::sleep(Duration::from_secs(if unlocked {
                    1
                } else {
                    crate::sync::IDLE_TICK_SECS
                }))
                .await;
                match c.quiet(nuvoletta_app_core::App::tick) {
                    TickOutcome::Idle => {}
                    TickOutcome::Saved | TickOutcome::SaveFailed(_) => c.bump(),
                    TickOutcome::Locked => {
                        bridge::destroy();
                        ui.reset_for_lock();
                        ui.info("Locked after inactivity.");
                        c.bump();
                    }
                    TickOutcome::LockBlocked(msg) => {
                        let mut lb = ui.lock_blocked;
                        if lb.peek().as_ref() != Some(&msg) {
                            lb.set(Some(msg));
                        }
                    }
                    TickOutcome::LockPending => c.bump(),
                    TickOutcome::Sync(ev) => crate::sync::handle_event(&c, ui, ev),
                }
            }
        }
    });

    // Collect the result of a running sync cycle quickly (the cycle itself runs on a worker
    // thread, so the UI never waits for it).
    let c = core.clone();
    use_future(move || {
        let c = c.clone();
        async move {
            loop {
                // Fast only while a cycle runs; the one-second tick starts and notices cycles.
                let busy = c.peek(nuvoletta_app_core::App::sync_in_flight);
                tokio::time::sleep(Duration::from_millis(if busy { 200 } else { 1000 })).await;
                if busy && let Some(ev) = c.quiet(nuvoletta_app_core::App::poll_sync) {
                    crate::sync::handle_event(&c, ui, ev);
                }
            }
        }
    });

    // Keep relative times ("synced 3 min ago") current.
    let core_clock = core.clone();
    use_future(move || {
        let core_clock = core_clock.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(crate::sync::CLOCK_REFRESH_SECS)).await;
                // Nothing shows relative times while locked, so do not re-render for them.
                if core_clock.peek(|a| a.phase() == nuvoletta_app_core::Phase::Unlocked) {
                    let mut t = ui.clock_ms;
                    t.set(crate::sync::wall_ms());
                }
            }
        }
    });

    // Closing the window flushes unsaved edits (best effort: a failure cannot stop the close).
    // On mobile, leaving the foreground saves and locks (ADR 0006); the app may be frozen or
    // killed at any moment after that, so nothing is left to a timer.
    let c = core.clone();
    use_wry_event_handler(move |event, _| match event {
        Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } => {
            let _ = c.quiet(|a| a.close_vault(false));
        }
        Event::Suspended => match c.quiet(nuvoletta_app_core::App::on_background) {
            BackgroundOutcome::Locked => {
                bridge::destroy();
                ui.reset_for_lock();
                c.bump();
            }
            BackgroundOutcome::LockBlocked(msg) => {
                let mut lb = ui.lock_blocked;
                lb.set(Some(msg));
            }
            BackgroundOutcome::NotUnlocked | BackgroundOutcome::LockPending => {}
        },
        _ => {}
    });

    let phase = core.read(nuvoletta_app_core::App::phase);
    let must_change = core.read(nuvoletta_app_core::App::must_change_passphrase);
    let key_pending = ui.recovery_key.read().is_some();
    let settings_open = *ui.settings.read();
    let sync_open = *ui.sync.read();
    let lock_blocked = ui.lock_blocked.read().clone();
    let notice = ui.notice.read().clone();
    // On narrow screens: the list when asked for, and whenever no note is open.
    let show_list = *ui.list_open.read() || core.read(|a| a.open_info().is_none());

    rsx! {
        a { class: "skip", href: "#cm",
            onclick: move |e| {
                // `#cm` is not focusable by itself: move focus into the editor.
                e.prevent_default();
                bridge::focus_editor();
            },
            "Skip to editor"
        }
        if let Some(n) = notice {
            div { class: if n.error { "banner" } else { "banner ok" },
                role: if n.error { "alert" } else { "status" },
                span { "{n.text}" }
                div { class: "actions",
                    button { onclick: move |_| {
                        let mut n = ui.notice;
                        n.set(None);
                    }, "Dismiss" }
                }
            }
        }
        if key_pending {
            RecoveryScreen {}
        } else {
            match phase {
                Phase::NoVault => rsx! { Picker {} },
                Phase::Locked => rsx! { Unlock {} },
                Phase::Unlocked => rsx! {
                    if must_change {
                        ForcedPassphrase {}
                    } else {
                        div { class: "app",
                            Topbar {}
                            div { class: if show_list { "body show-list" } else { "body" },
                                Sidebar {}
                                Workspace {}
                            }
                        }
                        if settings_open { Settings {} }
                        if sync_open { SyncDialog {} }
                        if *ui.trash.read() { TrashDialog {} }
                    }
                },
            }
        }
        if let Some(msg) = lock_blocked {
            LockBlocked { message: msg }
        }
    }
}

/// Lock the vault now. Unsaved edits are saved first; if that fails nothing is locked and the
/// user decides (retry, keep working, or lock and discard).
pub fn lock(core: &Core, ui: Ui, force: bool) {
    match core.act(|a| a.lock(force)) {
        Ok(()) if core.peek(nuvoletta_app_core::App::phase) == Phase::Unlocked => {
            // A sync cycle has the vault: the core locks when it ends (after saving).
            let mut lb = ui.lock_blocked;
            lb.set(None);
            ui.info("Locking as soon as the sync in progress has finished.");
        }
        Ok(()) => {
            bridge::destroy();
            ui.reset_for_lock();
            let mut lb = ui.lock_blocked;
            lb.set(None);
        }
        Err(e) => {
            let mut lb = ui.lock_blocked;
            lb.set(Some(e.user_message()));
        }
    }
}

/// Create a note in `folder` and focus the editor.
pub fn new_note(core: &Core, ui: Ui, folder: &str) {
    match core.act(|a| a.create_note(folder)) {
        Ok(_) => {
            let mut l = ui.list_open;
            l.set(false);
            bridge::focus_editor();
        }
        Err(e) => ui.error(e.user_message()),
    }
}

fn handle_message(core: &Core, ui: Ui, msg: &serde_json::Value) {
    match msg.get("t").and_then(serde_json::Value::as_str) {
        Some("edit") => {
            let (Some(id), Some(text)) = (
                msg.get("id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|s| s.parse().ok()),
                msg.get("text").and_then(serde_json::Value::as_str),
            ) else {
                return;
            };
            let was_dirty = core.peek(|a| a.open_info().is_some_and(|i| i.dirty));
            if core.quiet(|a| a.edit(id, text)).is_ok() {
                let mut tick = ui.edit_tick;
                tick += 1;
                let now_dirty = core.peek(|a| a.open_info().is_some_and(|i| i.dirty));
                if was_dirty != now_dirty {
                    core.bump();
                }
            }
        }
        Some("act") => core.quiet(nuvoletta_app_core::App::touch),
        Some("link") => {
            core.quiet(nuvoletta_app_core::App::touch);
            let href = msg
                .get("href")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            match classify_link(href) {
                LinkAction::OpenNote(id) => {
                    if let Err(e) = core.act(|a| a.open_note(id)) {
                        ui.error(e.user_message());
                    }
                }
                // Only ever reached by a click on a link in the preview (see js/entry.js).
                LinkAction::External(url) => {
                    if !crate::platform::open_external(&url) {
                        ui.error("Could not open the link in your browser.");
                    }
                }
                LinkAction::Ignore => {}
            }
        }
        Some("key") => {
            core.quiet(nuvoletta_app_core::App::touch);
            let name = msg
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if core.peek(nuvoletta_app_core::App::phase) != Phase::Unlocked {
                return;
            }
            match name {
                "new" => new_note(core, ui, ""),
                "search" => bridge::focus_id("search"),
                "lock" => lock(core, ui, false),
                "save" => {
                    if let Err(e) = core.act(nuvoletta_app_core::App::save_now) {
                        ui.error(e.user_message());
                    }
                }
                "preview" => {
                    let mut p = ui.preview;
                    p.toggle();
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Top bar: brand mark, search, and the main actions.
#[component]
fn Topbar() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let info = core.read(nuvoletta_app_core::App::open_info);
    let (status, bad) = match &info {
        Some(i) if i.save_error.is_some() => ("Not saved", true),
        Some(i) if i.dirty => ("Unsaved changes", false),
        Some(_) => ("Saved", false),
        None => ("", false),
    };
    let c1 = core.clone();
    let c2 = core.clone();
    let c3 = core.clone();
    rsx! {
        header { class: "topbar",
            img { class: "mark", src: "{*assets::MARK}", alt: "Nuvoletta" }
            div { class: "search",
                label { class: "sr-only", r#for: "search", "Search notes" }
                input {
                    id: "search",
                    r#type: "search",
                    placeholder: "Search notes",
                    value: "{ui.query}",
                    oninput: move |e| {
                        let mut q = ui.query;
                        q.set(e.value());
                    },
                }
            }
            button { onclick: move |_| crate::root::new_note(&c1, ui, ""), title: "New note (Ctrl+N)", "New note" }
            button {
                disabled: info.is_none(),
                onclick: move |_| {
                    if let Err(e) = c2.act(nuvoletta_app_core::App::save_now) {
                        ui.error(e.user_message());
                    }
                },
                title: "Save now (Ctrl+S)",
                "Save"
            }
            button {
                "aria-pressed": "{ui.preview}",
                onclick: move |_| {
                    let mut p = ui.preview;
                    p.toggle();
                },
                title: "Toggle preview (Ctrl+Shift+P)",
                "Preview"
            }
            span { class: if bad { "status bad" } else { "status" }, role: "status", "{status}" }
            div { class: "spacer" }
            SyncControls {}
            button {
                onclick: move |_| {
                    let mut s = ui.settings;
                    s.set(true);
                },
                "Settings"
            }
            button { class: "primary", onclick: move |_| crate::root::lock(&c3, ui, false), title: "Lock (Ctrl+L)", "Lock" }
        }
    }
}
