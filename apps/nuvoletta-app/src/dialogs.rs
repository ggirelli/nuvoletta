//! Modal screens and dialogs: recovery key, forced passphrase change, lock decision, delete
//! confirmation, settings (auto-lock, passphrase, recovery key, export, import).

use crate::core::Core;
use crate::picker::{PathField, Wordmark};
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_core::NoteId;
use std::path::PathBuf;

/// Shows a recovery key once, with an explicit "I have written it down" step.
#[component]
pub fn RecoveryScreen() -> Element {
    let ui = use_context::<Ui>();
    let mut written = use_signal(|| false);
    let key = ui.recovery_key.read().clone().unwrap_or_default();
    rsx! {
        div { class: "screen",
            div { class: "card", role: "dialog", "aria-modal": "true", "aria-labelledby": "rk-title",
                Wordmark {}
                h1 { id: "rk-title", "Write down your recovery key" }
                p { "If you forget your passphrase, this key is the only way back into your notes. Nobody can recover it for you." }
                p { class: "warning", "This is the only time it is shown. Write it on paper and keep it somewhere safe, away from this computer." }
                div { class: "recovery-key", role: "group", "aria-label": "Recovery key", tabindex: "0",
                    for (i, group) in key.split('-').enumerate() {
                        span { class: "grp", key: "{i}", "{group}" }
                    }
                }
                label { class: "check",
                    input {
                        r#type: "checkbox",
                        checked: written(),
                        onchange: move |e| written.set(e.checked()),
                    }
                    span { "I have written down my recovery key." }
                }
                div { class: "actions",
                    button {
                        class: "primary",
                        disabled: !written(),
                        onclick: move |_| {
                            let mut k = ui.recovery_key;
                            k.set(None);
                        },
                        "Continue"
                    }
                }
            }
        }
    }
}

/// After a recovery-key unlock the user must set a new passphrase before continuing.
#[component]
pub fn ForcedPassphrase() -> Element {
    let core = use_context::<Core>();
    let mut pass = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    rsx! {
        div { class: "screen",
            form {
                class: "card",
                onsubmit: move |e| {
                    e.prevent_default();
                    if *pass.read() != *confirm.read() {
                        error.set(Some("The two passphrases differ.".into()));
                        return;
                    }
                    let r = core.act(|a| a.change_passphrase(&pass.read()));
                    pass.set(String::new());
                    confirm.set(String::new());
                    match r {
                        Ok(()) => error.set(None),
                        Err(e) => error.set(Some(e.user_message())),
                    }
                },
                Wordmark {}
                h1 { "Choose a new passphrase" }
                p { class: "hint", "You unlocked with the recovery key. Set a new passphrase now (at least 12 characters)." }
                label { r#for: "np", "New passphrase" }
                input { id: "np", r#type: "password", autocomplete: "new-password", autofocus: true, value: "{pass}", oninput: move |e| pass.set(e.value()) }
                label { r#for: "np2", "Repeat the passphrase" }
                input { id: "np2", r#type: "password", autocomplete: "new-password", value: "{confirm}", oninput: move |e| confirm.set(e.value()) }
                if let Some(e) = error() {
                    p { class: "error", role: "alert", "{e}" }
                }
                div { class: "actions",
                    button { class: "primary", r#type: "submit", "Set passphrase" }
                }
            }
        }
    }
}

/// Locking was refused because unsaved edits could not be written.
#[component]
pub fn LockBlocked(message: String) -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let (c1, c2) = (core.clone(), core.clone());
    rsx! {
        div { class: "scrim",
            div { class: "dialog", role: "alertdialog", "aria-modal": "true", "aria-labelledby": "lb-title", "aria-describedby": "lb-desc",
                // Escape is the safe choice: keep working, discard nothing.
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        let mut lb = ui.lock_blocked;
                        lb.set(None);
                    }
                },
                h1 { id: "lb-title", "The vault is still unlocked" }
                p { id: "lb-desc", class: "error", "{message}" }
                p { "Your edits could not be saved, so the vault was not locked and nothing was discarded." }
                div { class: "actions",
                    button {
                        class: "primary",
                        autofocus: true,
                        onclick: move |_| crate::root::lock(&c1, ui, false),
                        "Retry save and lock"
                    }
                    button {
                        onclick: move |_| {
                            let mut lb = ui.lock_blocked;
                            lb.set(None);
                        },
                        "Keep working"
                    }
                    button {
                        class: "danger",
                        onclick: move |_| crate::root::lock(&c2, ui, true),
                        "Lock and discard unsaved edits"
                    }
                }
            }
        }
    }
}

/// Confirm deleting a note.
#[component]
pub fn ConfirmDelete(id: NoteId) -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let title = core.read(|a| {
        a.notes()
            .into_iter()
            .find(|n| n.id == id)
            .map(|n| n.title)
            .unwrap_or_default()
    });
    rsx! {
        div { class: "scrim",
            div { class: "dialog", role: "alertdialog", "aria-modal": "true", "aria-labelledby": "cd-title",
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        let mut d = ui.confirm_delete;
                        d.set(None);
                    }
                },
                h1 { id: "cd-title", "Delete this note?" }
                p { "“{title}” will be deleted from this vault. This cannot be undone." }
                div { class: "actions",
                    button {
                        class: "danger",
                        onclick: move |_| {
                            if let Err(e) = core.act(|a| a.delete_note(id)) {
                                ui.error(e.user_message());
                            }
                            let mut d = ui.confirm_delete;
                            d.set(None);
                        },
                        "Delete"
                    }
                    button {
                        autofocus: true,
                        onclick: move |_| {
                            let mut d = ui.confirm_delete;
                            d.set(None);
                        },
                        "Cancel"
                    }
                }
            }
        }
    }
}

/// Settings: auto-lock, passphrase, recovery key, export and import.
#[component]
pub fn Settings() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let minutes = core.read(|a| a.settings().auto_lock_minutes);
    let mut minutes_s = use_signal(|| minutes.to_string());

    let mut np = use_signal(String::new);
    let mut np2 = use_signal(String::new);
    let mut pass_msg = use_signal(|| None::<(bool, String)>);

    let mut cur = use_signal(String::new);
    let mut rk_msg = use_signal(|| None::<String>);

    let export_dir = use_signal(String::new);
    let mut understood = use_signal(|| false);
    let mut export_msg = use_signal(|| None::<(bool, String)>);
    let import_dir = use_signal(String::new);
    let mut import_msg = use_signal(|| None::<(bool, String)>);

    let (c_min, c_pass, c_rk, c_exp, c_imp) = (
        core.clone(),
        core.clone(),
        core.clone(),
        core.clone(),
        core.clone(),
    );

    rsx! {
        div { class: "scrim",
            div { class: "dialog", role: "dialog", "aria-modal": "true", "aria-labelledby": "set-title",
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        let mut s = ui.settings;
                        s.set(false);
                    }
                },
                h1 { id: "set-title", "Settings" }

                section {
                    h2 { "Auto-lock" }
                    form {
                        onsubmit: move |e| {
                            e.prevent_default();
                            let parsed = minutes_s.read().trim().parse::<u32>();
                            match parsed {
                                Ok(m) => {
                                    c_min.act(|a| a.set_auto_lock_minutes(m));
                                    minutes_s.set(c_min.peek(|a| a.settings().auto_lock_minutes).to_string());
                                    if let Err(e) = c_min.save_settings() {
                                        ui.error(e.user_message());
                                    } else {
                                        ui.info("Auto-lock period saved.");
                                    }
                                }
                                Err(_) => ui.error("Enter a whole number of minutes."),
                            }
                        },
                        label { r#for: "autolock", "Lock after this many minutes of inactivity (1 to 1440)" }
                        div { class: "row",
                            input { id: "autolock", r#type: "number", min: "1", max: "1440", autofocus: true, value: "{minutes_s}", oninput: move |e| minutes_s.set(e.value()) }
                            button { r#type: "submit", "Save" }
                        }
                    }
                }

                section {
                    h2 { "Passphrase" }
                    form {
                        onsubmit: move |e| {
                            e.prevent_default();
                            if *np.read() != *np2.read() {
                                pass_msg.set(Some((false, "The two passphrases differ.".into())));
                                return;
                            }
                            let r = c_pass.act(|a| a.change_passphrase(&np.read()));
                            np.set(String::new());
                            np2.set(String::new());
                            pass_msg.set(Some(match r {
                                Ok(()) => (true, "Passphrase changed.".into()),
                                Err(e) => (false, e.user_message()),
                            }));
                        },
                        label { r#for: "sp1", "New passphrase (at least 12 characters)" }
                        input { id: "sp1", r#type: "password", autocomplete: "new-password", value: "{np}", oninput: move |e| np.set(e.value()) }
                        label { r#for: "sp2", "Repeat the passphrase" }
                        input { id: "sp2", r#type: "password", autocomplete: "new-password", value: "{np2}", oninput: move |e| np2.set(e.value()) }
                        if let Some((ok, m)) = pass_msg() {
                            p { class: if ok { "hint" } else { "error" }, role: if ok { "status" } else { "alert" }, "{m}" }
                        }
                        div { class: "actions", button { r#type: "submit", "Change passphrase" } }
                    }
                }

                section {
                    h2 { "Recovery key" }
                    p { class: "hint", "Generate a new recovery key. The old one stops working." }
                    form {
                        onsubmit: move |e| {
                            e.prevent_default();
                            let r = c_rk.act(|a| a.rotate_recovery_key(&cur.read()));
                            cur.set(String::new());
                            match r {
                                Ok(key) => {
                                    rk_msg.set(None);
                                    let mut k = ui.recovery_key;
                                    k.set(Some(key));
                                }
                                Err(e) => rk_msg.set(Some(e.user_message())),
                            }
                        },
                        label { r#for: "rk-pass", "Current passphrase" }
                        input { id: "rk-pass", r#type: "password", autocomplete: "current-password", value: "{cur}", oninput: move |e| cur.set(e.value()) }
                        if let Some(m) = rk_msg() {
                            p { class: "error", role: "alert", "{m}" }
                        }
                        div { class: "actions", button { r#type: "submit", "Generate new recovery key" } }
                    }
                }

                if crate::platform::ANDROID {
                    section {
                        h2 { "Export and import" }
                        p { class: "hint", "Not available on Android yet: they need access to a folder outside the app, which this version does not request. Your notes stay encrypted in the app's private storage." }
                    }
                } else {
                    section {
                        h2 { "Export notes" }
                        p { class: "warning", "Export writes your notes as plain, unencrypted Markdown files. Anyone who can read that folder can read your notes, and locking the vault does not remove them. Choose a place you trust and delete the files when you are done." }
                        form {
                            onsubmit: move |e| {
                                e.prevent_default();
                                let dest = PathBuf::from(export_dir.read().trim());
                                if dest.as_os_str().is_empty() {
                                    export_msg.set(Some((false, "Choose a destination folder.".into())));
                                    return;
                                }
                                let r = c_exp.act(|a| a.export(&dest));
                                export_msg.set(Some(match r {
                                    Ok(rep) => {
                                        understood.set(false);
                                        let mut m = format!("Exported {} notes to {}.", rep.exported.len(), dest.display());
                                        if !rep.failed.is_empty() {
                                            m.push_str(&format!(" {} notes could not be read and were not exported.", rep.failed.len()));
                                        }
                                        (rep.failed.is_empty(), m)
                                    }
                                    Err(e) => (false, e.user_message()),
                                }));
                            },
                            PathField { id: "export-dir", label: "Destination folder (new or empty)", value: export_dir, title: "Choose the export folder" }
                            label { class: "check",
                                input { r#type: "checkbox", checked: understood(), onchange: move |e| understood.set(e.checked()) }
                                span { "I understand the exported files are not encrypted." }
                            }
                            if let Some((ok, m)) = export_msg() {
                                p { class: if ok { "hint" } else { "error" }, role: if ok { "status" } else { "alert" }, "{m}" }
                            }
                            div { class: "actions", button { r#type: "submit", disabled: !understood(), "Export" } }
                        }
                    }

                    section {
                        h2 { "Import notes" }
                        p { class: "hint", "Import every .md file in a folder (including subfolders). Existing notes are never overwritten." }
                        form {
                            onsubmit: move |e| {
                                e.prevent_default();
                                let src = PathBuf::from(import_dir.read().trim());
                                if src.as_os_str().is_empty() {
                                    import_msg.set(Some((false, "Choose a folder to import.".into())));
                                    return;
                                }
                                let r = c_imp.act(|a| a.import(&src));
                                import_msg.set(Some(match r {
                                    Ok(rep) => (
                                        rep.conflicts.is_empty(),
                                        format!(
                                            "Imported {} notes; {} already present; {} id conflicts (imported as new notes); {} files skipped.",
                                            rep.imported.len(),
                                            rep.already_present.len(),
                                            rep.conflicts.len(),
                                            rep.skipped.len()
                                        ),
                                    ),
                                    Err(e) => (false, e.user_message()),
                                }));
                            },
                            PathField { id: "import-dir", label: "Folder to import", value: import_dir, title: "Choose the folder to import" }
                            if let Some((ok, m)) = import_msg() {
                                p { class: if ok { "hint" } else { "error" }, role: if ok { "status" } else { "alert" }, "{m}" }
                            }
                            div { class: "actions", button { r#type: "submit", "Import" } }
                        }
                    }
                }

                div { class: "actions",
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let mut s = ui.settings;
                            s.set(false);
                        },
                        "Close"
                    }
                }
            }
        }
    }
}
