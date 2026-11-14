//! The bridge between Rust and the CodeMirror editor (`js/entry.js`).
//!
//! JS to Rust: one long-lived `document::eval` channel carrying `{t, ...}` messages (edits,
//! shortcuts, link clicks, activity). Rust to JS: short eval snippets calling `window.nv.*`.
//! Plaintext crosses this boundary by design (PLAN.md Phase 0, `docs/THREAT_MODEL.md`).

use dioxus::document;

/// Run a snippet once `window.nv` (the editor bundle) is loaded.
fn run(body: &str) {
    let _ = document::eval(&format!(
        "while (!window.nv) await new Promise(r => setTimeout(r, 20)); {body}"
    ));
}

fn lit(s: &str) -> String {
    // A JSON string is a valid JS string literal.
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Create the editor inside `#cm`.
pub fn mount() {
    run(
        "while (!document.getElementById('cm')) await new Promise(r => setTimeout(r, 20)); \
         window.nv.mount(document.getElementById('cm'));",
    );
}

/// Show a note's text in the editor.
pub fn set_doc(id: &str, text: &str) {
    run(&format!(
        "while (!window.nv.view) await new Promise(r => setTimeout(r, 20)); window.nv.setDoc({}, {});",
        lit(id),
        lit(text)
    ));
}

/// Empty the editor and its undo history.
pub fn clear() {
    let _ = document::eval("if (window.nv) window.nv.clear();");
}

/// Destroy the editor view (and clear it first).
pub fn destroy() {
    let _ = document::eval("if (window.nv) window.nv.destroy();");
}

/// Move focus into the editor.
pub fn focus_editor() {
    run("window.nv.focus();");
}

/// Move focus to an element by id.
pub fn focus_id(id: &str) {
    let _ = document::eval(&format!(
        "const e = document.getElementById({}); if (e) {{ e.focus(); if (e.select) e.select(); }}",
        lit(id)
    ));
}

/// Start the message channel; returns the receiving end.
pub fn connect() -> document::Eval {
    document::eval(
        "while (!window.nv) await new Promise(r => setTimeout(r, 20)); \
         window.nv.connect((m) => dioxus.send(m)); \
         await new Promise(() => {});",
    )
}
