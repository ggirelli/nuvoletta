//! "Use the new passphrase": adopting a vault header that another device published after a
//! passphrase or recovery-key change. Opened from the sync warning (unlocked device) or from the
//! unlock screen (locked device). Everything that decides lives in `nuvoletta-app-core`
//! (`adopt.rs` there).

use crate::core::Core;
use crate::picker::{PathField, Wordmark};
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{
    AdoptPreview, AdoptSearchEvent, App, JoinSecret, JoinSource, WebDavInput,
};
use std::path::PathBuf;
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Source,
    Searching,
    Secret,
    Weaker,
}

fn close(ui: Ui, core: &Core) {
    core.quiet(App::adopt_cancel);
    let mut a = ui.adopt;
    a.set(false);
}

/// The adoption screens. `locked`: shown on the unlock screen instead of in a dialog.
#[component]
pub fn AdoptDialog(locked: bool) -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let needs_source = core.peek(App::adopt_needs_source);
    let mut step = use_signal(|| {
        if needs_source {
            Step::Source
        } else {
            Step::Searching
        }
    });
    let mut use_dav = use_signal(|| crate::platform::ANDROID);
    let folder = use_signal(String::new);
    let mut url = use_signal(String::new);
    let mut user = use_signal(String::new);
    let mut pass = use_signal(String::new);
    let mut pin = use_signal(|| None::<String>);
    let mut trusted = use_signal(|| false);
    let mut pin_prompt = use_signal(|| None::<(String, String)>);
    let mut recovery = use_signal(|| false);
    let mut secret = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut preview = use_signal(|| None::<AdoptPreview>);
    let mut costly = use_signal(|| None::<String>);
    let mut diverged = use_signal(|| false);

    let c_start = core.clone();
    let start = move || {
        let source = if needs_source {
            Some(if use_dav() {
                JoinSource::WebDav(WebDavInput {
                    url: url.read().clone(),
                    username: user.read().clone(),
                    password: Zeroizing::new(pass.read().clone()),
                    pin: pin.read().clone(),
                })
            } else {
                JoinSource::Folder(PathBuf::from(folder.read().trim()))
            })
        } else {
            None
        };
        error.set(None);
        match c_start.act(|a| a.adopt_search_start(source.as_ref())) {
            Ok(()) => step.set(Step::Searching),
            Err(e) => {
                error.set(Some(e.user_message()));
                step.set(if needs_source {
                    Step::Source
                } else {
                    Step::Searching
                });
            }
        }
    };

    // Start at once when the location is known.
    let mut begin = start.clone();
    use_hook(move || {
        if !needs_source {
            begin();
        }
    });
    let mut start_form = start.clone();
    let start_pin = start;

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
                let Some(ev) = c.quiet(App::adopt_search_poll) else {
                    continue;
                };
                match ev {
                    AdoptSearchEvent::Found {
                        too_costly,
                        diverged: d,
                        ..
                    } => {
                        costly.set(too_costly);
                        diverged.set(d);
                        step.set(Step::Secret);
                    }
                    AdoptSearchEvent::UpToDate => {
                        error.set(Some("No newer vault header was found at the sync location. If you just changed the passphrase on another device, wait for it to sync and try again.".into()));
                        step.set(Step::Source);
                    }
                    AdoptSearchEvent::Failed { message, hint } => {
                        error.set(Some(format!("{message} {hint}")));
                        step.set(Step::Source);
                    }
                    AdoptSearchEvent::Refused { title, explanation } => {
                        error.set(Some(format!("{title}. {explanation}")));
                        step.set(Step::Source);
                    }
                    AdoptSearchEvent::NeedsPin {
                        fingerprint,
                        reason,
                    } => {
                        trusted.set(false);
                        pin_prompt.set(Some((fingerprint, reason)));
                        step.set(Step::Source);
                    }
                }
            }
        }
    });

    let c_verify = core.clone();
    let verify = move |e: FormEvent| {
        e.prevent_default();
        error.set(None);
        let typed = secret.read().clone();
        let arg = if recovery() {
            JoinSecret::RecoveryKey(&typed)
        } else {
            JoinSecret::Passphrase(&typed)
        };
        let result = c_verify.act(|a| a.adopt_verify(arg));
        secret.set(String::new());
        match result {
            Ok(p) => {
                let needs_confirm = p.weaker_kdf.is_some();
                preview.set(Some(p));
                if needs_confirm {
                    step.set(Step::Weaker);
                } else {
                    commit(&c_verify, ui, false, locked, error);
                }
            }
            Err(e) => error.set(Some(e.user_message())),
        }
    };
    let (c_weak, c_close) = (core.clone(), core.clone());
    let current = step();
    let pv = preview();

    let body = rsx! {
        match current {
            Step::Source | Step::Searching => rsx! {
                form {
                    onsubmit: move |e| { e.prevent_default(); pin.set(None); start_form(); },
                    p { class: "hint", "Looking for the vault header your other device published." }
                    if needs_source {
                        if locked {
                            p { class: "hint", "This device is locked, so the saved sync account cannot be read. Enter the location again." }
                        }
                        if !crate::platform::ANDROID {
                            div { class: "tabs", role: "group", "aria-label": "Where is the vault synced?",
                                button { r#type: "button", "aria-pressed": "{!use_dav()}", onclick: move |_| use_dav.set(false), "Synced folder" }
                                button { r#type: "button", "aria-pressed": "{use_dav()}", onclick: move |_| use_dav.set(true), "WebDAV" }
                            }
                        }
                        if use_dav() {
                            label { r#for: "a-url", "Folder address" }
                            input { id: "a-url", r#type: "url", autocomplete: "off", value: "{url}", oninput: move |e| url.set(e.value()) }
                            label { r#for: "a-user", "User name" }
                            input { id: "a-user", autocomplete: "username", value: "{user}", oninput: move |e| user.set(e.value()) }
                            label { r#for: "a-pass", "Password" }
                            input { id: "a-pass", r#type: "password", autocomplete: "current-password", value: "{pass}", oninput: move |e| pass.set(e.value()) }
                        } else {
                            PathField { id: "a-folder", label: "Sync folder", value: folder, title: "Choose the sync folder" }
                        }
                    }
                    if current == Step::Searching {
                        p { class: "hint", role: "status", "Searching…" }
                    }
                    if let Some((fp, reason)) = pin_prompt() {
                        div { class: "warning", role: "alert",
                            strong { "This server's certificate is not signed by a known authority" }
                            p { "Reason: {reason}. Compare this fingerprint (SHA-256) with the one your server shows:" }
                            p { code { "{fp}" } }
                            label { class: "check",
                                input { r#type: "checkbox", checked: trusted(), onchange: move |e| trusted.set(e.checked()) }
                                span { "The fingerprints are identical. Trust exactly this certificate." }
                            }
                            button {
                                r#type: "button", disabled: !trusted(),
                                onclick: {
                                    let fp = fp.clone();
                                    let mut start = start_pin.clone();
                                    move |_| { pin.set(Some(fp.clone())); pin_prompt.set(None); start(); }
                                },
                                "Trust this certificate and continue"
                            }
                        }
                    }
                    if let Some(e) = error() { p { class: "error", role: "alert", "{e}" } }
                    div { class: "actions",
                        if needs_source {
                            button { class: "primary", r#type: "submit", disabled: current == Step::Searching, "Look for the new header" }
                        }
                        button { r#type: "button", onclick: { let c = c_close.clone(); move |_| close(ui, &c) }, "Close" }
                    }
                }
            },
            Step::Secret => rsx! {
                form { onsubmit: verify,
                    p { "The passphrase or recovery key of this vault was changed on another device. Enter the new one to use it here. Your notes are not changed." }
                    if diverged() {
                        p { class: "warning", role: "alert", "The location holds two different headers of the newest generation, which happens when the passphrase was changed on two devices at once. The one your passphrase opens is used." }
                    }
                    if let Some(why) = costly() {
                        p { class: "warning", role: "alert", "{why} The recovery key still works." }
                    }
                    div { class: "tabs", role: "group", "aria-label": "Use",
                        button { r#type: "button", "aria-pressed": "{!recovery()}", onclick: move |_| { recovery.set(false); error.set(None); }, "New passphrase" }
                        button { r#type: "button", "aria-pressed": "{recovery()}", onclick: move |_| { recovery.set(true); error.set(None); }, "New recovery key" }
                    }
                    label { r#for: "a-secret", if recovery() { "New recovery key" } else { "New passphrase" } }
                    input {
                        id: "a-secret",
                        r#type: if recovery() { "text" } else { "password" },
                        autocomplete: "off", spellcheck: "false", autofocus: true,
                        value: "{secret}", oninput: move |e| secret.set(e.value()),
                    }
                    if let Some(e) = error() { p { class: "error", role: "alert", "{e}" } }
                    div { class: "actions",
                        button { class: "primary", r#type: "submit", "Use it" }
                        button { r#type: "button", onclick: { let c = c_close.clone(); move |_| close(ui, &c) }, "Cancel" }
                    }
                }
            },
            Step::Weaker => rsx! {
                if let Some(p) = pv {
                    p { "The new header opened (generation {p.generation}, you have {p.current_generation})." }
                    if let Some(w) = p.weaker_kdf.clone() {
                        p { class: "warning", role: "alert", "{w}" }
                    }
                    for (i, w) in p.warnings.iter().enumerate() {
                        p { key: "{i}", class: "warning", role: "alert", "{w}" }
                    }
                    if let Some(e) = error() { p { class: "error", role: "alert", "{e}" } }
                    div { class: "actions",
                        button { class: "danger", r#type: "button", onclick: move |_| commit(&c_weak, ui, true, locked, error), "Adopt it anyway" }
                        button { r#type: "button", onclick: { let c = c_close.clone(); move |_| close(ui, &c) }, "Cancel" }
                    }
                }
            },
        }
    };

    if locked {
        rsx! {
            div { class: "screen",
                div { class: "card", role: "group", "aria-labelledby": "adopt-title",
                    Wordmark {}
                    h1 { id: "adopt-title", "Use the new passphrase" }
                    {body}
                }
            }
        }
    } else {
        rsx! {
            div { class: "scrim",
                div { class: "dialog", role: "dialog", "aria-modal": "true", "aria-labelledby": "adopt-title",
                    onkeydown: {
                        let c = core.clone();
                        move |e| { if e.key() == Key::Escape { close(ui, &c); } }
                    },
                    h1 { id: "adopt-title", "Use the new passphrase" }
                    {body}
                }
            }
        }
    }
}

fn commit(
    core: &Core,
    ui: Ui,
    accept_weaker: bool,
    locked: bool,
    mut error: Signal<Option<String>>,
) {
    match core.act(|a| a.adopt_commit(accept_weaker)) {
        Ok(out) => {
            let mut a = ui.adopt;
            a.set(false);
            let mut text = format!(
                "This device now uses vault header generation {}.",
                out.generation
            );
            if locked {
                text.push_str(" Unlock with the new passphrase.");
            }
            if !out.warnings.is_empty() {
                text.push_str(" A warning about another header was added to the sync warnings.");
            }
            ui.info(text);
        }
        Err(e) => error.set(Some(e.user_message())),
    }
}
