//! Sidebar: note tree (or search results, or a tag filter) and the tags panel.

use crate::core::Core;
use crate::ui::Ui;
use dioxus::prelude::*;
use nuvoletta_app_core::{App, NoteRef, TreeRow};
use nuvoletta_core::NoteId;

/// The note navigation sidebar.
#[component]
pub fn Sidebar() -> Element {
    let core = use_context::<Core>();
    let ui = use_context::<Ui>();
    let query = ui.query.read().trim().to_owned();
    let tag = ui.tag.read().clone();
    let open = core.read(|a| a.open_info().map(|i| i.id));
    let c = core.clone();
    let open_note = move |id: NoteId| match c.act(|a| a.open_note(id)) {
        Ok(_) => {
            let mut l = ui.list_open;
            l.set(false);
        }
        Err(e) => ui.error(e.user_message()),
    };

    let body = if !query.is_empty() {
        let hits = core.read(|a| a.search(&query, 50));
        rsx! {
            h2 { "Results" }
            if hits.is_empty() {
                p { class: "hint", "No notes match." }
            }
            ul {
                for h in hits {
                    li { key: "{h.note.id}",
                        div { class: "tree-item",
                            button {
                                class: "note",
                                "aria-current": if open == Some(h.note.id) { "true" } else { "false" },
                                onclick: {
                                    let f = open_note.clone();
                                    move |_| f(h.note.id)
                                },
                                "{h.note.title}"
                                if let Some(s) = h.snippet.clone() {
                                    span { class: "snippet", "{s}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(tag) = tag.clone() {
        let notes: Vec<NoteRef> = core.read(|a| a.notes_with_tag(&tag));
        rsx! {
            h2 { "Tag: {tag}" }
            button { class: "link", onclick: move |_| {
                let mut t = ui.tag;
                t.set(None);
            }, "Show all notes" }
            ul {
                for n in notes {
                    li { key: "{n.id}",
                        div { class: "tree-item",
                            button {
                                class: "note",
                                "aria-current": if open == Some(n.id) { "true" } else { "false" },
                                onclick: {
                                    let f = open_note.clone();
                                    move |_| f(n.id)
                                },
                                "{n.title}"
                            }
                        }
                    }
                }
            }
        }
    } else {
        let rows = core.read(App::tree);
        let c2 = core.clone();
        rsx! {
            h2 { "Notes" }
            if rows.is_empty() {
                p { class: "hint", "No notes yet. Use New note to write the first one." }
            }
            ul { role: "tree", "aria-label": "Notes by folder",
                for (i, row) in rows.into_iter().enumerate() {
                    match row {
                        TreeRow::Folder { depth, name, path } => rsx! {
                            li { key: "f{i}", role: "treeitem", class: "folder depth-{depth.min(8)}",
                                span { "{name}/" }
                                button {
                                    class: "link",
                                    "aria-label": "New note in {path}",
                                    onclick: {
                                        let c = c2.clone();
                                        let path = path.clone();
                                        move |_| crate::root::new_note(&c, ui, &path)
                                    },
                                    "+ note"
                                }
                            }
                        },
                        TreeRow::Note { depth, label, id } => rsx! {
                            li { key: "n{id}", role: "treeitem", class: "depth-{depth.min(8)}",
                                div { class: "tree-item",
                                    button {
                                        class: "note",
                                        "aria-current": if open == Some(id) { "true" } else { "false" },
                                        onclick: {
                                            let f = open_note.clone();
                                            move |_| f(id)
                                        },
                                        "{label}"
                                    }
                                }
                            }
                        },
                    }
                }
            }
        }
    };

    let tags = core.read(App::tags);
    rsx! {
        nav { class: "sidebar", "aria-label": "Notes",
            div { {body} }
            button {
                class: "link",
                onclick: move |_| {
                    let mut t = ui.trash;
                    t.set(true);
                },
                "Recently deleted"
            }
            if !tags.is_empty() {
                section { "aria-label": "Tags",
                    h2 { "Tags" }
                    div {
                        for t in tags {
                            span { class: "tag", key: "{t.tag}",
                                button {
                                    "aria-pressed": if tag.as_deref() == Some(t.tag.as_str()) { "true" } else { "false" },
                                    onclick: {
                                        let name = t.tag.clone();
                                        move |_| {
                                            let mut s = ui.tag;
                                            if s.peek().as_deref() == Some(name.as_str()) {
                                                s.set(None);
                                            } else {
                                                s.set(Some(name.clone()));
                                            }
                                        }
                                    },
                                    "#{t.tag} ({t.count})"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
