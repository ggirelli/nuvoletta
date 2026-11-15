//! Sync: the status button in the top bar, the Sync dialog (status, "needs attention", integrity
//! warnings, folder settings, diagnostics) and the conflict resolution view.
//!
//! Everything that decides anything lives in `nuvoletta-app-core`; this file renders it.

use crate::bridge;
use crate::core::Core;
use crate::picker::PathField;
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{
    App, AttentionReason, CleanupSummary, ConflictAction, ConflictItem, ConflictKind, DiffKind,
    HeadView, SyncEvent, SyncState, SyncStatus,
};
use nuvoletta_core::{NoteId, now_ms};
use std::path::PathBuf;

/// "3 minutes ago" style text for a past wall-clock time.
fn ago(then_ms: i64, now: i64) -> String {
    let s = (now - then_ms).max(0) / 1000;
    match s {
        0..=9 => "just now".to_owned(),
        10..=59 => format!("{s} seconds ago"),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The short text of the status button, and whether it signals trouble.
fn summary(st: &SyncStatus, now: i64) -> (String, bool) {
    if st.lock_pending {
        return ("Locking after sync…".to_owned(), false);
    }
    if st.notices > 0 {
        return (
            format!("Sync: {}", plural(st.notices, "warning", "warnings")),
            true,
        );
    }
    if st.problem.is_some() {
        return ("Sync: problem".to_owned(), true);
    }
    if st.conflicts > 0 {
        return (
            format!("Sync: {}", plural(st.conflicts, "conflict", "conflicts")),
            true,
        );
    }
    let text = match st.state {
        SyncState::Disabled => "Sync: off".to_owned(),
        SyncState::Syncing => "Syncing…".to_owned(),
        SyncState::Idle => match st.last_synced_ms {
            Some(t) => format!("Synced {}", ago(t, now)),
            None => "Not synced yet".to_owned(),
        },
    };
    (text, st.needs_attention > 0)
}

/// Handle what happened to the sync cycle: refresh, tell the user, finish a pending lock.
pub fn handle_event(core: &Core, ui: Ui, ev: SyncEvent) {
    match ev {
        SyncEvent::Started => {}
        SyncEvent::Finished(f) => {
            if let Some(m) = f.message {
                ui.info(m);
            }
        }
        SyncEvent::Locked => {
            bridge::destroy();
            ui.reset_for_lock();
            ui.info("Locked.");
        }
        SyncEvent::LockBlocked(msg) => {
            let mut lb = ui.lock_blocked;
            lb.set(Some(msg));
        }
    }
    core.bump();
}

/// Status button and "Sync now" for the top bar.
#[component]
pub fn SyncControls() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let st = core.read(App::sync_status);
    let now = *ui.clock_ms.read();
    let (text, bad) = summary(&st, now);
    let busy = st.state != SyncState::Idle;
    let c = core.clone();
    rsx! {
        button {
            class: if bad { "syncbtn bad" } else { "syncbtn" },
            title: "Sync status, conflicts and settings",
            onclick: move |_| {
                let mut s = ui.sync;
                s.set(true);
            },
            "{text}"
        }
        span { class: "sr-only", role: "status", "{text}" }
        button {
            disabled: busy,
            title: "Sync now",
            onclick: move |_| {
                if let Err(e) = c.act(App::sync_now) {
                    ui.error(e.user_message());
                }
            },
            "Sync now"
        }
    }
}

fn bytes_text(b: u64) -> String {
    match b {
        0..=1023 => format!("{b} bytes"),
        1024..=1_048_575 => format!("{:.1} KiB", b as f64 / 1024.0),
        _ => format!("{:.1} MiB", b as f64 / 1_048_576.0),
    }
}

/// "Clean up old versions": preview, explicit confirmation, result. All decisions are made by
/// the core; this only shows the counts and asks.
#[component]
fn CleanupSection(disabled: bool) -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let mut preview = use_signal(|| None::<CleanupSummary>);
    let mut result = use_signal(|| None::<CleanupSummary>);
    let mut confirmed = use_signal(|| false);
    let (c_prev, c_run) = (core.clone(), core.clone());
    rsx! {
        section { "aria-labelledby": "sync-clean",
            h2 { id: "sync-clean", "Clean up old versions" }
            p { class: "hint", "Frees space in the sync folder by deleting old versions of notes that this device wrote, once every device has seen the newer ones and at least 30 days have passed. The newest versions, the versions behind them and anything in conflict are always kept. Deleted versions cannot be brought back." }
            div { class: "actions",
                button {
                    disabled,
                    onclick: move |_| {
                        result.set(None);
                        confirmed.set(false);
                        match c_prev.act(App::cleanup_preview) {
                            Ok(p) => preview.set(Some(p)),
                            Err(e) => {
                                preview.set(None);
                                ui.error(e.user_message());
                            }
                        }
                    },
                    "Preview clean-up"
                }
            }
            if let Some(p) = preview() {
                if let Some(why) = p.unavailable.clone() {
                    p { class: "hint", role: "status", "{why}" }
                } else if p.old_versions + p.leftovers == 0 {
                    p { class: "hint", role: "status", "Nothing to clean up right now. {plural(p.kept, \"version is\", \"versions are\")} kept." }
                } else {
                    div { role: "status",
                        p { "Would delete {plural(p.old_versions, \"old version\", \"old versions\")} ({bytes_text(p.old_version_bytes)}) and {plural(p.leftovers, \"leftover file\", \"leftover files\")} ({bytes_text(p.leftover_bytes)}) from the sync folder, and {plural(p.cache_entries, \"old copy\", \"old copies\")} from this device's cache. {plural(p.kept, \"version is\", \"versions are\")} kept." }
                        label { class: "check",
                            input { r#type: "checkbox", checked: confirmed(), onchange: move |e| confirmed.set(e.checked()) }
                            span { "I understand that deleted versions cannot be brought back." }
                        }
                        div { class: "actions",
                            button {
                                class: "danger",
                                disabled: disabled || !confirmed(),
                                onclick: move |_| {
                                    match c_run.act(App::cleanup_run) {
                                        Ok(r) => {
                                            preview.set(None);
                                            confirmed.set(false);
                                            result.set(Some(r));
                                        }
                                        Err(e) => ui.error(e.user_message()),
                                    }
                                },
                                "Delete old versions"
                            }
                        }
                    }
                }
            }
            if let Some(r) = result() {
                p { role: "status",
                    "Deleted {plural(r.old_versions, \"old version\", \"old versions\")} ({bytes_text(r.old_version_bytes)}) and {plural(r.leftovers, \"leftover file\", \"leftover files\")} ({bytes_text(r.leftover_bytes)}); removed {plural(r.cache_entries, \"old copy\", \"old copies\")} from this device's cache. {plural(r.kept, \"version is\", \"versions are\")} kept."
                }
            }
        }
    }
}

fn close_dialog(ui: Ui) {
    let mut s = ui.sync;
    s.set(false);
    let mut c = ui.conflict;
    c.set(None);
}

/// The Sync dialog, or the conflict view when a conflict is being reviewed.
#[component]
pub fn SyncDialog() -> Element {
    let ui = use_context::<Ui>();
    if let Some(id) = *ui.conflict.read() {
        return rsx! { ConflictView { id } };
    }
    rsx! { SyncMain {} }
}

#[component]
fn SyncMain() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let st = core.read(App::sync_status);
    let conflicts = core.read(App::sync_conflicts);
    let attention = core.read(App::sync_attention);
    let notices = core.read(App::sync_notices);
    let strays = core.read(App::sync_strays);
    let invalid = core.read(App::sync_invalid);
    let settings = core.read(|a| a.sync_settings().clone());
    let now = *ui.clock_ms.read();
    let (text, _) = summary(&st, now);
    // "Sync now" needs sync to be on and idle; the settings only need no cycle to be running
    // (with no folder chosen yet the state is `Disabled`, and that is exactly when they are needed).
    let busy = st.state != SyncState::Idle;
    let syncing = st.state == SyncState::Syncing;

    let folder = use_signal(|| {
        settings
            .folder
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut confirm_change = use_signal(|| false);
    let mut folder_msg = use_signal(|| None::<(bool, String)>);
    let had_folder = settings.folder.is_some();
    let (c_now, c_use, c_toggle) = (core.clone(), core.clone(), core.clone());
    let (c_ack, c_open) = (core.clone(), core.clone());

    rsx! {
        div { class: "scrim",
            div { class: "dialog wide", role: "dialog", "aria-modal": "true", "aria-labelledby": "sync-title",
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        close_dialog(ui);
                    }
                },
                h1 { id: "sync-title", "Sync" }
                p { role: "status", "{text}" }
                if let Some(p) = st.problem.clone() {
                    div { class: "error", role: "alert",
                        p { "{p.message}" }
                        p { "{p.hint}" }
                    }
                }
                if st.pending_objects > 0 {
                    p { class: "hint", "{plural(st.pending_objects, \"note has\", \"notes have\")} changes that are not on the sync folder yet." }
                }
                div { class: "actions",
                    button {
                        class: "primary",
                        autofocus: true,
                        disabled: busy,
                        onclick: move |_| {
                            if let Err(e) = c_now.act(App::sync_now) {
                                ui.error(e.user_message());
                            }
                        },
                        "Sync now"
                    }
                    button { onclick: move |_| close_dialog(ui), "Close" }
                }

                if !notices.is_empty() {
                    section { "aria-labelledby": "sync-warn",
                        h2 { id: "sync-warn", "Integrity warnings" }
                        p { class: "hint", "These stay until you acknowledge them. Nothing was changed or deleted because of them." }
                        for (i, n) in notices.iter().enumerate() {
                            div { class: "warning notice", role: "alert", key: "{i}-{n.code}",
                                strong { "{n.title}" }
                                p { "{n.detail}" }
                                p { class: "hint", "First seen {ago(n.first_seen_ms, now)}." }
                                button {
                                    onclick: {
                                        let c = c_ack.clone();
                                        move |_| {
                                            if let Err(e) = c.act(|a| a.acknowledge_sync_notice(i)) {
                                                ui.error(e.user_message());
                                            }
                                        }
                                    },
                                    "Acknowledge"
                                }
                            }
                        }
                    }
                }

                section { "aria-labelledby": "sync-attn",
                    h2 { id: "sync-attn", "Needs attention" }
                    if conflicts.is_empty() && attention.is_empty() {
                        p { class: "hint", "Nothing needs your attention." }
                    }
                    ul { class: "plain",
                        for c in conflicts.iter().cloned() {
                            li { key: "c-{c.note}",
                                strong { "{c.title}" }
                                " "
                                span { class: "hint",
                                    if c.kind == ConflictKind::DeleteVsEdit {
                                        "deleted on one device, edited on another"
                                    } else {
                                        "{c.heads.len()} conflicting versions"
                                    }
                                }
                                " "
                                button {
                                    onclick: move |_| {
                                        let mut s = ui.conflict;
                                        s.set(Some(c.note));
                                    },
                                    "Review"
                                }
                            }
                        }
                        for a in attention.iter().cloned() {
                            li { key: "a-{a.note}",
                                strong { "{a.title}" }
                                " "
                                span { class: "hint",
                                    match a.reason {
                                        AttentionReason::Frozen => "its history was frozen after a device ID was used twice; this device's copy is kept",
                                        AttentionReason::NotArrived => "the newest version has not arrived yet; it is retried every sync",
                                    }
                                }
                                " "
                                button {
                                    onclick: {
                                        let c = c_open.clone();
                                        move |_| {
                                            close_dialog(ui);
                                            if let Err(e) = c.act(|x| x.open_note(a.note)) {
                                                ui.error(e.user_message());
                                            }
                                        }
                                    },
                                    "Open"
                                }
                            }
                        }
                    }
                }

                section { "aria-labelledby": "sync-folder",
                    h2 { id: "sync-folder", "Sync folder" }
                    p { class: "hint", "Choose the folder that your sync tool (Syncthing, Dropbox, iCloud, Nextcloud, …) keeps in step between your devices. Only encrypted files are written there. The choice is stored on this device only." }
                    if let Some(f) = settings.folder.clone() {
                        p { "Current folder: " code { "{f.display()}" } if !settings.enabled { " (sync is off)" } }
                    }
                    form {
                        onsubmit: move |e| {
                            e.prevent_default();
                            let path = PathBuf::from(folder.read().trim());
                            let same = c_use.peek(|a| a.sync_settings().folder.as_deref().and_then(|f| f.canonicalize().ok()) == path.canonicalize().ok());
                            if had_folder && !same && !*confirm_change.read() {
                                folder_msg.set(Some((false, "Confirm that you understand the warning below first.".into())));
                                return;
                            }
                            match c_use.act(|a| a.configure_sync(&path)) {
                                Ok(()) => {
                                    confirm_change.set(false);
                                    folder_msg.set(Some((true, "Sync folder saved. Sync is on.".into())));
                                }
                                Err(e) => folder_msg.set(Some((false, e.user_message()))),
                            }
                        },
                        PathField { id: "sync-dir", label: "Sync folder", value: folder, title: "Choose the sync folder" }
                        if had_folder {
                            label { class: "check",
                                input { r#type: "checkbox", checked: confirm_change(), onchange: move |e| confirm_change.set(e.checked()) }
                                span { "If I choose a different folder than before: I understand that a folder that is empty or holds older data than this device has seen is reported as a rollback warning. Only choose the same folder at a new location." }
                            }
                        }
                        if let Some((ok, m)) = folder_msg() {
                            p { class: if ok { "hint" } else { "error" }, role: if ok { "status" } else { "alert" }, "{m}" }
                        }
                        div { class: "actions",
                            button { r#type: "submit", disabled: syncing, "Use this folder" }
                            if had_folder {
                                button {
                                    r#type: "button",
                                    disabled: syncing,
                                    onclick: move |_| {
                                        let on = !c_toggle.peek(|a| a.sync_settings().enabled);
                                        if let Err(e) = c_toggle.act(|a| a.set_sync_enabled(on)) {
                                            ui.error(e.user_message());
                                        }
                                    },
                                    if settings.enabled { "Turn sync off" } else { "Turn sync on" }
                                }
                            }
                        }
                    }
                }

                CleanupSection { disabled: busy }

                section { "aria-labelledby": "sync-diag",
                    h2 { id: "sync-diag", "Diagnostics" }
                    p { class: "hint", "Files in the sync folder that are not Nuvoletta files, and entries that could not be applied. Nuvoletta only reports them: it never deletes or changes them." }
                    if strays.is_empty() && invalid.is_empty() {
                        p { class: "hint", "No stray files." }
                    }
                    if !strays.is_empty() {
                        ul { class: "plain report",
                            for s in strays {
                                li { key: "{s.name}",
                                    code { "{s.name}" }
                                    " ({s.size} bytes): {s.treatment}"
                                }
                            }
                        }
                    }
                    if !invalid.is_empty() {
                        ul { class: "plain report",
                            for (i, e) in invalid.iter().enumerate() {
                                li { key: "{i}",
                                    "Device {e.device}, "
                                    if e.seq == 0 { "segment" } else { "entry {e.seq}" }
                                    ": {e.reason}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn head_label(h: &HeadView, i: usize, n: usize, now: i64) -> String {
    let who = if h.devices.is_empty() {
        String::new()
    } else {
        format!(", from device {}", h.devices.join(", "))
    };
    let kind = if h.is_delete {
        "deletes the note"
    } else {
        "edited"
    };
    format!(
        "Version {} of {n}{}: {kind}{who}, {}",
        i + 1,
        if h.is_local { " (this device)" } else { "" },
        ago(h.time_ms, now)
    )
}

const MAX_DIFF_LINES: usize = 400;

/// Review one conflicting note: every version, a diff against this device's version, and the
/// resolution actions.
#[component]
fn ConflictView(id: NoteId) -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let error = use_signal(|| None::<String>);
    let now = *ui.clock_ms.read();
    let item: Option<ConflictItem> =
        core.read(|a| a.sync_conflicts().into_iter().find(|c| c.note == id));
    let back = move || {
        let mut c = ui.conflict;
        c.set(None);
    };
    let Some(item) = item else {
        // Resolved elsewhere (or by a sync): go back.
        return rsx! {
            div { class: "scrim",
                div { class: "dialog wide", role: "dialog", "aria-modal": "true", "aria-labelledby": "cv-title",
                    h1 { id: "cv-title", "This note is no longer in conflict" }
                    div { class: "actions", button { class: "primary", autofocus: true, onclick: move |_| back(), "Back" } }
                }
            }
        };
    };
    let n = item.heads.len();
    let content_heads: Vec<&HeadView> = item.heads.iter().filter(|h| !h.is_delete).collect();
    let can_keep_both = n == 2 && content_heads.len() == 2;
    let delete_vs_edit = item.kind == ConflictKind::DeleteVsEdit;

    // Resolve with `action`; optionally open the note afterwards so it can be edited.
    let run = {
        let core = core.clone();
        move |action: ConflictAction, edit_after: bool| match core
            .act(|a| a.resolve_conflict(id, &action))
        {
            Ok(_) => {
                let mut c = ui.conflict;
                c.set(None);
                ui.info("Conflict resolved. It syncs to your other devices; the other versions stay in the sync history.");
                if edit_after {
                    let mut s = ui.sync;
                    s.set(false);
                    if let Err(e) = core.act(|a| a.open_note(id)) {
                        ui.error(e.user_message());
                    } else {
                        bridge::focus_editor();
                    }
                }
            }
            Err(e) => {
                let mut err = error;
                err.set(Some(e.user_message()));
            }
        }
    };

    rsx! {
        div { class: "scrim",
            div { class: "dialog wide", role: "dialog", "aria-modal": "true", "aria-labelledby": "cv-title",
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        back();
                    }
                },
                h1 { id: "cv-title", "Resolve: {item.title}" }
                p { class: "hint",
                    if delete_vs_edit {
                        "This note was deleted on one device and edited on another. Nothing is lost whichever you choose: the other version stays in the sync history."
                    } else {
                        "This note was edited on {n} devices at the same time. Choose which version to use, or keep both. The other versions stay in the sync history. To merge by hand, use the closest version and edit it."
                    }
                }
                if let Some(e) = error() {
                    p { class: "error", role: "alert", "{e}" }
                }
                if delete_vs_edit {
                    div { class: "actions",
                        button { class: "primary", onclick: { let run = run.clone(); move |_| run(ConflictAction::Keep, false) }, "Keep the note" }
                        button { class: "danger", onclick: { let run = run.clone(); move |_| run(ConflictAction::Delete, false) }, "Delete the note" }
                    }
                }
                for (i, h) in item.heads.iter().enumerate() {
                    section { key: "{h.rev}", "aria-label": "Version {i + 1}",
                        h2 { "{head_label(h, i, n, now)}" }
                        HeadBody { id, rev: h.rev.clone() }
                        if !h.available {
                            p { class: "hint", "This version has not arrived yet; you can resolve once it has." }
                        } else if !delete_vs_edit {
                            div { class: "actions",
                                button {
                                    onclick: { let run = run.clone(); let rev = h.rev.clone(); move |_| run(ConflictAction::ChooseHead(rev.clone()), false) },
                                    "Use version {i + 1}"
                                }
                                button {
                                    onclick: { let run = run.clone(); let rev = h.rev.clone(); move |_| run(ConflictAction::ChooseHead(rev.clone()), true) },
                                    "Use version {i + 1} and edit"
                                }
                                if can_keep_both && !h.is_delete {
                                    button {
                                        onclick: {
                                            let run = run.clone();
                                            let keep = h.rev.clone();
                                            let copy = content_heads.iter().find(|o| o.rev != h.rev).map(|o| o.rev.clone()).unwrap_or_default();
                                            move |_| run(ConflictAction::KeepBoth { keep: keep.clone(), copy: copy.clone() }, false)
                                        },
                                        "Keep version {i + 1}, save the other as a copy"
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "actions",
                    button { autofocus: true, onclick: move |_| back(), "Back" }
                }
            }
        }
    }
}

/// The content and diff of one version.
#[component]
fn HeadBody(id: NoteId, rev: String) -> Element {
    let core = use_context::<Core>();
    let detail = core.read(|a| a.conflict_head(id, &rev));
    match detail {
        Err(e) => rsx! { p { class: "hint", "{e.user_message()}" } },
        Ok(d) => {
            let total = d.diff.len();
            let changed = d.diff.iter().any(|l| l.kind != DiffKind::Same);
            rsx! {
                if d.body.is_none() {
                    p { "This version deletes the note." }
                } else {
                    p { class: "hint",
                        "Title: {d.title.clone().unwrap_or_default()}; folder and name: {d.path.clone().unwrap_or_default()}"
                        if !d.tags.is_empty() { "; tags: {d.tags.join(\", \")}" }
                    }
                }
                if d.no_local_version {
                    p { class: "hint", "This device has no current version of the note (it is deleted here)." }
                } else if d.head.is_local {
                    p { class: "hint", "This is the version on this device." }
                } else if !changed {
                    p { class: "hint", "The text is the same as this device's version." }
                } else {
                    p { class: "hint", "Changes compared with this device's version (- removed, + added):" }
                    pre { class: "diff", tabindex: "0", "aria-label": "Changes in this version",
                        for (i, l) in d.diff.iter().take(MAX_DIFF_LINES).enumerate() {
                            div { key: "{i}",
                                class: match l.kind { DiffKind::Added => "added", DiffKind::Removed => "removed", DiffKind::Same => "same" },
                                match l.kind { DiffKind::Added => "+ ", DiffKind::Removed => "- ", DiffKind::Same => "  " }
                                "{l.text}"
                            }
                        }
                        if total > MAX_DIFF_LINES {
                            div { class: "same", "… {total - MAX_DIFF_LINES} more lines" }
                        }
                    }
                }
            }
        }
    }
}

/// Minutes between refreshes of the relative times.
pub const CLOCK_REFRESH_SECS: u64 = 20;

/// The current wall-clock time in ms (for relative times).
pub fn wall_ms() -> i64 {
    now_ms()
}
