//! Vault picker (create or open) and the unlock screen.

use crate::assets;
use crate::core::Core;
use crate::ui::Ui;
use dioxus::prelude::*;
use std::path::PathBuf;

/// The brand wordmark, light or dark following the system theme.
#[component]
pub fn Wordmark() -> Element {
    rsx! {
        // Both are in the page; CSS shows the one that matches the system theme.
        img { class: "wordmark wm-light", src: "{*assets::WORDMARK_LIGHT}", alt: "Nuvoletta" }
        img { class: "wordmark wm-dark", src: "{*assets::WORDMARK_DARK}", alt: "" }
    }
}

pub use crate::platform::pick_folder;

/// A path field with a Browse button.
#[component]
pub fn PathField(id: String, label: String, value: Signal<String>, title: String) -> Element {
    let mut value = value;
    rsx! {
        label { r#for: "{id}", "{label}" }
        div { class: "row",
            input {
                id: "{id}",
                r#type: "text",
                value: "{value}",
                autocomplete: "off",
                spellcheck: "false",
                oninput: move |e| value.set(e.value()),
            }
            button {
                r#type: "button",
                onclick: move |_| {
                    let title = title.clone();
                    spawn(async move {
                        if let Some(p) = pick_folder(&title).await {
                            value.set(p.to_string_lossy().into_owned());
                        }
                    });
                },
                "Browse…"
            }
        }
    }
}

/// Choose a vault directory: open an existing vault or create a new one.
#[component]
pub fn Picker() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let last = core.read(|a| a.settings().last_vault.clone());
    // Android: the vault location is fixed (app-private). Reaching this screen there means no
    // vault exists yet (an existing one is opened at start-up), so offer to create it.
    let fixed = crate::platform::fixed_vault_dir();
    let mut creating = use_signal(|| fixed.is_some());
    let path = use_signal(|| {
        fixed
            .as_ref()
            .or(last.as_ref())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut pass = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let c = core.clone();
    let submit = move |e: FormEvent| {
        e.prevent_default();
        error.set(None);
        let dir = PathBuf::from(path.read().trim());
        if dir.as_os_str().is_empty() {
            error.set(Some("Choose a folder first.".into()));
            return;
        }
        if *creating.read() {
            if *pass.read() != *confirm.read() {
                error.set(Some("The two passphrases differ.".into()));
                return;
            }
            let result = c.act(|a| a.create_vault(&dir, &pass.read()));
            pass.set(String::new());
            confirm.set(String::new());
            match result {
                Ok(key) => {
                    let _ = c.save_settings();
                    let mut k = ui.recovery_key;
                    k.set(Some(key));
                }
                Err(e) => error.set(Some(e.user_message())),
            }
        } else {
            match c.act(|a| a.open_vault(&dir)) {
                Ok(()) => {
                    let _ = c.save_settings();
                }
                Err(e) => error.set(Some(e.user_message())),
            }
        }
    };

    rsx! {
        div { class: "screen",
            form { class: "card", onsubmit: submit,
                Wordmark {}
                h1 { "Welcome" }
                p { class: "hint", "Your notes are encrypted in a folder you choose. Nothing leaves this device." }
                div { class: "tabs", role: "group", "aria-label": "What do you want to do?",
                    button { r#type: "button", "aria-pressed": "{!creating()}", onclick: move |_| creating.set(false), "Open a vault" }
                    button { r#type: "button", "aria-pressed": "{creating()}", onclick: move |_| creating.set(true), "Create a vault" }
                }
                if crate::platform::ANDROID {
                    p { class: "hint", "The vault is stored in this app's private storage, which other apps cannot read." }
                } else {
                    PathField {
                        id: "vault-path",
                        label: if creating() { "Folder for the new vault" } else { "Vault folder" },
                        value: path,
                        title: "Choose the vault folder",
                    }
                }
                if creating() {
                    label { r#for: "new-pass", "Passphrase (at least 12 characters)" }
                    input { id: "new-pass", r#type: "password", autocomplete: "new-password", value: "{pass}", oninput: move |e| pass.set(e.value()) }
                    label { r#for: "new-pass2", "Repeat the passphrase" }
                    input { id: "new-pass2", r#type: "password", autocomplete: "new-password", value: "{confirm}", oninput: move |e| confirm.set(e.value()) }
                    p { class: "hint", "There is no way to reset a lost passphrase. You will get a recovery key next." }
                }
                if let Some(e) = error() {
                    p { class: "error", role: "alert", "{e}" }
                }
                div { class: "actions",
                    button { class: "primary", r#type: "submit", if creating() { "Create vault" } else { "Open vault" } }
                }
            }
        }
    }
}

/// Unlock the chosen vault with the passphrase or the recovery key.
#[component]
pub fn Unlock() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let root = core.read(|a| {
        a.vault_root()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut recovery = use_signal(|| false);
    let mut secret = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let c = core.clone();
    let submit = move |e: FormEvent| {
        e.prevent_default();
        let result = if *recovery.read() {
            c.act(|a| a.unlock_with_recovery_key(&secret.read()))
        } else {
            c.act(|a| a.unlock_with_passphrase(&secret.read()))
        };
        secret.set(String::new());
        match result {
            Ok(()) => error.set(None),
            Err(e) => error.set(Some(e.user_message())),
        }
    };
    let c = core.clone();

    rsx! {
        div { class: "screen",
            form { class: "card", onsubmit: submit,
                Wordmark {}
                h1 { "Unlock your vault" }
                p { class: "hint", "{root}" }
                div { class: "tabs", role: "group", "aria-label": "Unlock with",
                    button { r#type: "button", "aria-pressed": "{!recovery()}", onclick: move |_| { recovery.set(false); error.set(None); }, "Passphrase" }
                    button { r#type: "button", "aria-pressed": "{recovery()}", onclick: move |_| { recovery.set(true); error.set(None); }, "Recovery key" }
                }
                label { r#for: "secret", if recovery() { "Recovery key" } else { "Passphrase" } }
                input {
                    id: "secret",
                    r#type: if recovery() { "text" } else { "password" },
                    autocomplete: "off",
                    spellcheck: "false",
                    autofocus: true,
                    value: "{secret}",
                    oninput: move |e| secret.set(e.value()),
                }
                if recovery() {
                    p { class: "hint", "After unlocking with the recovery key you will choose a new passphrase." }
                }
                if let Some(e) = error() {
                    p { class: "error", role: "alert", "{e}" }
                }
                div { class: "actions",
                    button { class: "primary", r#type: "submit", "Unlock" }
                    if !crate::platform::ANDROID {
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                if let Err(e) = c.act(|a| a.close_vault(false)) {
                                    ui.error(e.user_message());
                                }
                            },
                            "Choose another vault"
                        }
                    }
                }
            }
        }
    }
}
