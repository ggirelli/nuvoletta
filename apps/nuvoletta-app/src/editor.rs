//! The workspace: note bar, editor, live preview and link panels.

use crate::bridge;
use crate::core::Core;
use crate::dialogs::ConfirmDelete;
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{App, NoteInfo, parse_tags};

/// Editor + preview + links for the open note.
#[component]
pub fn Workspace() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();

    // The editor view lives in `#cm` and is owned by JS; create it once, destroy it on lock.
    use_effect(bridge::mount);
    use_drop(bridge::destroy);

    // Push the open note into the editor whenever the open note changes.
    let c = core.clone();
    let open_id = use_memo(move || c.read(|a| a.open_info().map(|i| i.id)));
    let c = core.clone();
    use_effect(move || match open_id() {
        Some(id) => {
            let text = c.peek(|a| a.buffer().unwrap_or_default().to_owned());
            bridge::set_doc(&id.to_string(), &text);
        }
        None => bridge::clear(),
    });

    // The preview re-renders on edits and on index changes, without re-rendering the rest.
    let c = core.clone();
    let html = use_memo(move || {
        let _ = (ui.edit_tick)();
        c.read(App::preview_html)
    });

    let info = core.read(App::open_info);
    let show_preview = *ui.preview.read();
    let confirm = *ui.confirm_delete.read();

    rsx! {
        main { class: "workspace",
            if let Some(info) = info.clone() {
                NoteBar { key: "{info.id}", info }
                if let Some(err) = core.read(|a| a.open_info().and_then(|i| i.save_error)) {
                    div { class: "banner", role: "alert",
                        span { "This note could not be saved: {err}. Your edits are kept in the editor and saving is retried." }
                    }
                }
            }
            div { class: if show_preview { "panes show-preview" } else { "panes no-preview" },
                div {
                    id: "cm",
                    class: "pane editor",
                    hidden: info.is_none(),
                }
                if info.is_none() {
                    div { class: "empty",
                        p { "Select a note, or press Ctrl+N to create one." }
                    }
                }
                div {
                    id: "preview",
                    class: "pane preview",
                    role: "region",
                    "aria-label": "Preview",
                    hidden: info.is_none(),
                    dangerous_inner_html: "{html}",
                }
            }
            if info.is_some() {
                LinksPanel {}
            }
        }
        if let Some(id) = confirm {
            ConfirmDelete { id }
        }
    }
}

/// Name, folder and tags of the open note, and delete.
#[component]
fn NoteBar(info: NoteInfo) -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let folder = info.path.rsplit_once('/').map_or("", |(f, _)| f).to_owned();
    let mut name = use_signal(|| info.title.clone());
    let mut folder_s = use_signal(|| folder);
    let mut tags = use_signal(|| info.tags.join(", "));
    let id = info.id;
    let (c1, c2, c3) = (core.clone(), core.clone(), core.clone());

    rsx! {
        div { class: "notebar",
            label { r#for: "note-name", "Name" }
            input {
                id: "note-name",
                class: "name",
                r#type: "text",
                value: "{name}",
                oninput: move |e| name.set(e.value()),
                onchange: move |_| {
                    if let Err(e) = c1.act(|a| a.rename_note(id, &name.read())) {
                        ui.error(e.user_message());
                    }
                },
            }
            label { r#for: "note-folder", "Folder" }
            input {
                id: "note-folder",
                class: "name",
                r#type: "text",
                placeholder: "(top level)",
                value: "{folder_s}",
                oninput: move |e| folder_s.set(e.value()),
                onchange: move |_| {
                    if let Err(e) = c2.act(|a| a.move_note(id, &folder_s.read())) {
                        ui.error(e.user_message());
                    }
                },
            }
            label { r#for: "note-tags", "Tags" }
            input {
                id: "note-tags",
                class: "tags",
                r#type: "text",
                placeholder: "comma separated",
                value: "{tags}",
                oninput: move |e| tags.set(e.value()),
                onchange: move |_| {
                    let parsed = parse_tags(&tags.read());
                    if let Err(e) = c3.act(|a| a.set_tags(id, parsed)) {
                        ui.error(e.user_message());
                    }
                },
            }
            button {
                class: "danger",
                onclick: move |_| {
                    let mut d = ui.confirm_delete;
                    d.set(Some(id));
                },
                "Delete"
            }
        }
    }
}

/// Backlinks and unresolved links.
#[component]
fn LinksPanel() -> Element {
    let core = use_context::<Core>();
    let (backlinks, unresolved, vault_unresolved) = core.read(|a| {
        let id = a.open_info().map(|i| i.id);
        (
            id.map(|id| a.backlinks(id)).unwrap_or_default(),
            a.unresolved_in_open(),
            a.unresolved().len(),
        )
    });
    let c = core.clone();
    let ui = use_context::<Ui>();
    rsx! {
        aside { class: "links", "aria-label": "Links",
            section {
                h2 { "Backlinks" }
                if backlinks.is_empty() {
                    p { class: "hint", "No note links here yet." }
                } else {
                    ul {
                        for b in backlinks {
                            li { key: "{b.id}",
                                button {
                                    class: "link",
                                    onclick: {
                                        let c = c.clone();
                                        move |_| {
                                            if let Err(e) = c.act(|a| a.open_note(b.id)) {
                                                ui.error(e.user_message());
                                            }
                                        }
                                    },
                                    "{b.title}"
                                }
                            }
                        }
                    }
                }
            }
            section {
                h2 { "Unresolved links" }
                if unresolved.is_empty() {
                    p { class: "hint", "Every link in this note points to a note." }
                } else {
                    ul {
                        for t in unresolved {
                            li { "[[{t}]]" }
                        }
                    }
                }
                if vault_unresolved > 0 {
                    p { class: "hint", "{vault_unresolved} unresolved in the whole vault." }
                }
            }
        }
    }
}
