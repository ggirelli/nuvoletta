//! "Recently deleted": deleted notes, each with a Restore button. There is no purge action:
//! notes that never synced are kept 30 days in a local trash, synced ones follow the sync cleanup
//! rules.

use crate::core::Core;
use crate::sync::ago;
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{App, DeletedItem};

fn close(ui: Ui) {
    let mut t = ui.trash;
    t.set(false);
}

fn load(core: &Core) -> Result<Vec<DeletedItem>, String> {
    core.quiet(App::recently_deleted)
        .map_err(|e| e.user_message())
}

/// The Recently deleted dialog.
#[component]
pub fn TrashDialog() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let now = *ui.clock_ms.read();
    // Loaded on open and after each restore; `quiet` because listing records pending deletions.
    let mut items = use_signal({
        let core = core.clone();
        move || load(&core)
    });
    let list: Result<Vec<DeletedItem>, String> = items.read().clone();
    rsx! {
        div { class: "scrim",
            div { class: "dialog wide", role: "dialog", "aria-modal": "true", "aria-labelledby": "trash-title",
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        close(ui);
                    }
                },
                h1 { id: "trash-title", "Recently deleted" }
                p { class: "hint", "Deleted notes are kept for 30 days on this device and then removed for good; they cannot be emptied or removed from here. With sync set up, restoring brings a note back on all your devices, and synced notes follow the sync cleanup rules. A restored note whose id is already in use comes back as a separate copy." }
                match list {
                    Err(e) => rsx! { p { class: "error", role: "alert", "{e}" } },
                    Ok(list) if list.is_empty() => rsx! { p { class: "hint", role: "status", "No deleted notes." } },
                    Ok(list) => rsx! {
                        ul { class: "trash-list", "aria-label": "Deleted notes",
                            for d in list {
                                li { key: "{d.note}", class: "trash-item",
                                    div {
                                        strong { "{d.title.clone().unwrap_or_else(|| \"Untitled (content not downloaded yet)\".to_owned())}" }
                                        if let Some(p) = d.path.clone() {
                                            span { class: "hint", " {p}" }
                                        }
                                        div { class: "hint",
                                            "Deleted {ago(d.deleted_ms, now)}"
                                            if let Some(dev) = d.device.clone() { " by device {dev}" }
                                            if !d.can_restore { ". Sync, then try again." }
                                        }
                                    }
                                    button {
                                        disabled: !d.can_restore,
                                        "aria-label": "Restore {d.title.clone().unwrap_or_default()}",
                                        onclick: {
                                            let core = core.clone();
                                            let note = d.note;
                                            move |_| {
                                                match core.act(|a| a.restore_note(note)) {
                                                    Ok(_) => ui.info("Note restored."),
                                                    Err(e) => ui.error(e.user_message()),
                                                }
                                                items.set(load(&core));
                                            }
                                        },
                                        "Restore"
                                    }
                                }
                            }
                        }
                    },
                }
                div { class: "actions", button { class: "primary", autofocus: true, onclick: move |_| close(ui), "Close" } }
            }
        }
    }
}
