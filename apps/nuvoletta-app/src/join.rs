//! Joining this device to an existing vault: the screens before the vault exists (sync location,
//! vault choice, passphrase or recovery key, fingerprint confirmation) and the progress screen of
//! the first sync. Everything that decides lives in `nuvoletta-app-core` (`join.rs` there).

use crate::core::Core;
use crate::picker::{PathField, Wordmark};
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{
    App, JoinCancel, JoinCandidate, JoinPreview, JoinSearchEvent, JoinSecret, JoinSource,
    WebDavInput,
};
use std::path::PathBuf;
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Source,
    Searching,
    Choose,
    Secret,
    Confirm,
}

/// What the last search said, when it needs the user to act.
#[derive(Clone, PartialEq)]
enum Shown {
    Error { message: String, hint: String },
    NeedsPin { fingerprint: String, reason: String },
}

fn cancel(core: &Core, ui: Ui) {
    if let Err(e) = core.act(App::join_cancel) {
        ui.error(e.user_message());
    }
    let mut j = ui.join;
    j.set(false);
}

/// The screens of a join up to the first sync.
#[component]
pub fn JoinScreen() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let fixed = crate::platform::fixed_vault_dir();
    let mut step = use_signal(|| Step::Source);
    let mut use_dav = use_signal(|| crate::platform::ANDROID);
    let folder = use_signal(String::new);
    let mut url = use_signal(String::new);
    let mut user = use_signal(String::new);
    let mut pass = use_signal(String::new);
    let mut pin = use_signal(|| None::<String>);
    let mut trusted = use_signal(|| false);
    let mut shown = use_signal(|| None::<Shown>);
    let mut found = use_signal(Vec::<JoinCandidate>::new);
    let mut chosen = use_signal(|| 0usize);
    let mut recovery = use_signal(|| false);
    let mut older = use_signal(|| false);
    let mut secret = use_signal(String::new);
    let mut preview = use_signal(|| None::<JoinPreview>);
    let dir = use_signal(|| {
        fixed
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut error = use_signal(|| None::<String>);

    // Collect the result of the search, which runs on a worker thread.
    let c_poll = core.clone();
    use_future(move || {
        let c = c_poll.clone();
        async move {
            loop {
                let searching = *step.peek() == Step::Searching;
                tokio::time::sleep(Duration::from_millis(if searching { 150 } else { 600 })).await;
                if !searching {
                    continue;
                }
                let Some(ev) = c.quiet(App::join_search_poll) else {
                    continue;
                };
                match ev {
                    JoinSearchEvent::Found(list) => {
                        chosen.set(0);
                        step.set(if list.len() == 1 {
                            Step::Secret
                        } else {
                            Step::Choose
                        });
                        found.set(list);
                    }
                    JoinSearchEvent::NoVault { message } => {
                        shown.set(Some(Shown::Error {
                            message,
                            hint: String::new(),
                        }));
                        step.set(Step::Source);
                    }
                    JoinSearchEvent::Failed { message, hint } => {
                        shown.set(Some(Shown::Error { message, hint }));
                        step.set(Step::Source);
                    }
                    JoinSearchEvent::Refused { title, explanation } => {
                        pin.set(None);
                        shown.set(Some(Shown::Error {
                            message: title,
                            hint: explanation,
                        }));
                        step.set(Step::Source);
                    }
                    JoinSearchEvent::NeedsPin {
                        fingerprint,
                        reason,
                    } => {
                        trusted.set(false);
                        shown.set(Some(Shown::NeedsPin {
                            fingerprint,
                            reason,
                        }));
                        step.set(Step::Source);
                    }
                }
            }
        }
    });

    let c_start = core.clone();
    let start = move || {
        let source = if use_dav() {
            JoinSource::WebDav(WebDavInput {
                url: url.read().clone(),
                username: user.read().clone(),
                password: Zeroizing::new(pass.read().clone()),
                pin: pin.read().clone(),
            })
        } else {
            let f = folder.read().trim().to_owned();
            if f.is_empty() {
                shown.set(Some(Shown::Error {
                    message: "Choose the sync folder first.".into(),
                    hint: String::new(),
                }));
                return;
            }
            JoinSource::Folder(PathBuf::from(f))
        };
        shown.set(None);
        match c_start.act(|a| a.join_search_start(&source)) {
            Ok(()) => step.set(Step::Searching),
            Err(e) => shown.set(Some(Shown::Error {
                message: e.user_message(),
                hint: String::new(),
            })),
        }
    };
    let mut start_form = start.clone();
    let start_pin = start;

    let c_verify = core.clone();
    let verify = move |e: FormEvent| {
        e.prevent_default();
        error.set(None);
        let typed = secret.read().clone();
        let secret_arg = if recovery() {
            JoinSecret::RecoveryKey(&typed)
        } else {
            JoinSecret::Passphrase(&typed)
        };
        let result = c_verify.act(|a| a.join_verify(chosen(), secret_arg, older()));
        secret.set(String::new());
        match result {
            Ok(p) => {
                preview.set(Some(p));
                step.set(Step::Confirm);
            }
            Err(e) => error.set(Some(e.user_message())),
        }
    };

    let c_confirm = core.clone();
    let confirm = move |_| {
        error.set(None);
        let target = PathBuf::from(dir.read().trim());
        if target.as_os_str().is_empty() {
            error.set(Some(
                "Choose a folder for the vault on this device first.".into(),
            ));
            return;
        }
        match c_confirm.act(|a| a.join_confirm(&target)) {
            Ok(()) => {
                let _ = c_confirm.save_settings();
                let mut j = ui.join;
                j.set(false);
            }
            Err(e) => error.set(Some(e.user_message())),
        }
    };

    let c_cancel = core.clone();
    let current = step();
    let candidates = found();
    let me = candidates.get(chosen()).cloned();
    let pv = preview();

    rsx! {
        div { class: "screen",
            div { class: "card", role: "group", "aria-labelledby": "join-title",
                Wordmark {}
                h1 { id: "join-title", "Join an existing vault" }
                match current {
                    Step::Source | Step::Searching => rsx! {
                        form {
                            onsubmit: move |e| { e.prevent_default(); pin.set(None); start_form(); },
                            p { class: "hint", "Your notes stay encrypted. This looks for the vault's header in the place your other device syncs to; nothing is read or changed besides that. You need the vault's passphrase (or its recovery key) to continue." }
                            if !crate::platform::ANDROID {
                                div { class: "tabs", role: "group", "aria-label": "Where is the vault synced?",
                                    button { r#type: "button", "aria-pressed": "{!use_dav()}", onclick: move |_| use_dav.set(false), "Synced folder" }
                                    button { r#type: "button", "aria-pressed": "{use_dav()}", onclick: move |_| use_dav.set(true), "WebDAV" }
                                }
                            }
                            if use_dav() {
                                label { r#for: "j-url", "Folder address" }
                                input { id: "j-url", r#type: "url", placeholder: "https://cloud.example.org/remote.php/dav/files/me/Nuvoletta/", autocomplete: "off", value: "{url}", oninput: move |e| url.set(e.value()) }
                                label { r#for: "j-user", "User name" }
                                input { id: "j-user", autocomplete: "username", value: "{user}", oninput: move |e| user.set(e.value()) }
                                label { r#for: "j-pass", "Password (an app password if your server offers them)" }
                                input { id: "j-pass", r#type: "password", autocomplete: "current-password", value: "{pass}", oninput: move |e| pass.set(e.value()) }
                                p { class: "hint", "The account is checked first: it must be able to read and create files but not change or delete them. The check creates a few small test files named .nuvoletta-probe in the folder." }
                            } else {
                                PathField { id: "j-folder", label: "Sync folder", value: folder, title: "Choose the sync folder" }
                            }
                            if current == Step::Searching {
                                p { class: "hint", role: "status", "Looking for the vault header…" }
                            }
                            match shown() {
                                Some(Shown::Error { message, hint }) => rsx! {
                                    div { class: "error", role: "alert",
                                        p { "{message}" }
                                        if !hint.is_empty() { p { "{hint}" } }
                                    }
                                },
                                Some(Shown::NeedsPin { fingerprint, reason }) => rsx! {
                                    div { class: "warning", role: "alert",
                                        strong { "This server's certificate is not signed by a known authority" }
                                        p { "Reason: {reason}. Nuvoletta has not sent your password anywhere. If you trust this server, compare this fingerprint (SHA-256) with the one your server shows for its certificate:" }
                                        p { code { "{fingerprint}" } }
                                        label { class: "check",
                                            input { r#type: "checkbox", checked: trusted(), onchange: move |e| trusted.set(e.checked()) }
                                            span { "The fingerprints are identical. Trust exactly this certificate for this address." }
                                        }
                                        div { class: "actions",
                                            button {
                                                r#type: "button",
                                                disabled: !trusted(),
                                                onclick: {
                                                    let fp = fingerprint.clone();
                                                    let mut start = start_pin.clone();
                                                    move |_| { pin.set(Some(fp.clone())); start(); }
                                                },
                                                "Trust this certificate and continue"
                                            }
                                        }
                                    }
                                },
                                None => rsx! {},
                            }
                            div { class: "actions",
                                button { class: "primary", r#type: "submit", disabled: current == Step::Searching, "Look for the vault" }
                                button { r#type: "button", onclick: move |_| cancel(&c_cancel, ui), "Back" }
                            }
                        }
                    },
                    Step::Choose => rsx! {
                        p { "This location holds more than one vault. Choose yours by comparing its fingerprint with the one in Settings on your other device." }
                        ul { class: "vault-choices",
                            for c in candidates.clone() {
                                li { key: "{c.index}",
                                    button {
                                        r#type: "button",
                                        onclick: { let i = c.index; move |_| { chosen.set(i); step.set(Step::Secret); } },
                                        code { "{c.fingerprint}" }
                                        " (header generation {c.newest_generation})"
                                    }
                                }
                            }
                        }
                        div { class: "actions",
                            button { r#type: "button", onclick: move |_| step.set(Step::Source), "Back" }
                        }
                    },
                    Step::Secret => rsx! {
                        form { onsubmit: verify,
                            if let Some(c) = me.clone() {
                                p { class: "hint", "Vault " code { "{c.fingerprint}" } ", header generation {c.newest_generation}." }
                                if c.diverged {
                                    p { class: "warning", role: "alert", "The folder holds two different headers of the newest generation, which happens when the passphrase was changed on two devices at once. The one your passphrase opens is used." }
                                }
                                if let Some(why) = c.too_costly.clone() {
                                    p { class: "warning", role: "alert", "{why} The recovery key still works." }
                                }
                            }
                            div { class: "tabs", role: "group", "aria-label": "Open the vault with",
                                button { r#type: "button", "aria-pressed": "{!recovery()}", onclick: move |_| { recovery.set(false); error.set(None); }, "Passphrase" }
                                button { r#type: "button", "aria-pressed": "{recovery()}", onclick: move |_| { recovery.set(true); error.set(None); }, "Recovery key" }
                            }
                            label { r#for: "j-secret", if recovery() { "Recovery key" } else { "Current passphrase" } }
                            input {
                                id: "j-secret",
                                r#type: if recovery() { "text" } else { "password" },
                                autocomplete: "off", spellcheck: "false", autofocus: true,
                                value: "{secret}", oninput: move |e| secret.set(e.value()),
                            }
                            if recovery() {
                                p { class: "hint", "After the first sync you will choose a new passphrase for this vault." }
                            }
                            p { class: "hint", "The header in the sync folder lets anyone who has it try passphrases offline, so use a long passphrase." }
                            label { class: "check",
                                input { r#type: "checkbox", checked: older(), onchange: move |e| older.set(e.checked()) }
                                span { "My passphrase is from before the latest change: allow an older header." }
                            }
                            if let Some(e) = error() { p { class: "error", role: "alert", "{e}" } }
                            div { class: "actions",
                                button { class: "primary", r#type: "submit", "Continue" }
                                button { r#type: "button", onclick: move |_| step.set(Step::Source), "Back" }
                            }
                        }
                    },
                    Step::Confirm => rsx! {
                        if let Some(p) = pv {
                            p { "The header opened. Check that this is your vault before it is created on this device:" }
                            p { "Fingerprint: " code { "{p.fingerprint}" } }
                            p { "Header generation: {p.generation}" }
                            p { class: "hint", "Compare both with Settings on a device that already has the vault. A lower generation than there means the sync location offers an older passphrase setting; if you changed the passphrase because the old one may have leaked, do not continue." }
                            for (i, w) in p.warnings.iter().enumerate() {
                                p { key: "{i}", class: "warning", role: "alert", "{w}" }
                            }
                            if !crate::platform::ANDROID {
                                PathField { id: "j-dir", label: "Folder for the vault on this device (an empty folder)", value: dir, title: "Choose the vault folder" }
                            }
                            if let Some(e) = error() { p { class: "error", role: "alert", "{e}" } }
                            div { class: "actions",
                                button { class: "primary", r#type: "button", onclick: confirm, "Join this vault" }
                                button { r#type: "button", onclick: move |_| step.set(Step::Secret), "Back" }
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Shown while the first sync of a joined vault has not succeeded yet.
#[component]
pub fn JoinProgress() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let running = core.read(App::sync_in_flight);
    let problem = core.read(|a| a.sync_status().problem);
    let (c_retry, c_cancel) = (core.clone(), core.clone());
    let mut confirm_cancel = use_signal(|| false);
    rsx! {
        div { class: "screen",
            div { class: "card", role: "group", "aria-labelledby": "jp-title",
                Wordmark {}
                h1 { id: "jp-title", "Joining your vault" }
                if running {
                    p { role: "status", "Fetching your notes from the sync location…" }
                } else if let Some(p) = problem {
                    div { class: "error", role: "alert",
                        p { "{p.message}" }
                        p { "{p.hint}" }
                    }
                } else {
                    p { role: "status", "Waiting for the first sync…" }
                }
                if confirm_cancel() {
                    p { class: "warning", role: "alert", "Cancel the join? The vault created on this device is removed, if it holds no notes. Nothing on the sync location is changed." }
                }
                div { class: "actions",
                    button { class: "primary", r#type: "button", disabled: running,
                        onclick: move |_| {
                            if let Err(e) = c_retry.act(App::sync_now) { ui.error(e.user_message()); }
                        },
                        "Try again"
                    }
                    button {
                        class: if confirm_cancel() { "danger" } else { "" },
                        r#type: "button",
                        onclick: move |_| {
                            if !confirm_cancel() { confirm_cancel.set(true); return; }
                            match c_cancel.act(App::join_cancel) {
                                Ok(JoinCancel::Kept(p)) => ui.info(format!("The join was cancelled. This device's vault holds notes, so it was left in {}.", p.display())),
                                Ok(_) => ui.info("The join was cancelled and the new vault was removed."),
                                Err(e) => ui.error(e.user_message()),
                            }
                            confirm_cancel.set(false);
                        },
                        if confirm_cancel() { "Yes, cancel the join" } else { "Cancel the join" }
                    }
                }
            }
        }
    }
}
