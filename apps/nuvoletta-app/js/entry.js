// Nuvoletta editor bridge, bundled with CodeMirror 6 into ../assets/editor.js (see README.md).
//
// Rust talks to this file in two ways only:
//   Rust -> JS: window.nv.<method>(...) called from short eval snippets.
//   JS -> Rust: window.nv.emit(message) which is wired to the eval channel by nv.connect(send).
// Messages: {t:"edit", id, text} | {t:"link", href} | {t:"key", name} | {t:"act"}.
// Note content only ever travels through these two paths; nothing is fetched or stored.
import { EditorView, basicSetup } from "codemirror";
import { EditorState } from "@codemirror/state";
import { markdown } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";

// Colours come from CSS variables defined by the app (brand palette, light and dark).
const theme = EditorView.theme({
  "&": { backgroundColor: "var(--nv-surface)", color: "var(--nv-text)", height: "100%" },
  ".cm-content": { caretColor: "var(--nv-text)", fontFamily: "var(--nv-mono)", padding: "12px 0" },
  ".cm-cursor": { borderLeftColor: "var(--nv-text)" },
  "&.cm-focused": { outline: "none" }, // the pane draws the focus ring (main.css)
  ".cm-gutters": { backgroundColor: "var(--nv-surface)", color: "var(--nv-muted)", border: "none" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--nv-hover)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--nv-select) !important",
  },
  ".cm-scroller": { overflow: "auto" },
});

const highlight = HighlightStyle.define([
  { tag: t.heading, color: "var(--nv-accent-strong)", fontWeight: "bold" },
  { tag: t.strong, fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: [t.link, t.url], color: "var(--nv-accent)" },
  { tag: t.monospace, color: "var(--nv-accent-strong)" },
  { tag: t.quote, color: "var(--nv-muted)" },
  { tag: [t.processingInstruction, t.contentSeparator], color: "var(--nv-muted)" },
]);

const nv = {
  view: null,
  id: null, // id of the note whose text is in the editor
  silent: false, // true while Rust is setting the text (no echo)
  send: null,
  emit(msg) { if (this.send) this.send(msg); },
  connect(send) { this.send = send; },

  mount(el) {
    if (this.view) this.view.destroy();
    this.view = new EditorView({
      parent: el,
      state: this.newState(""),
    });
  },
  newState(text) {
    return EditorState.create({
      doc: text,
      extensions: [
        basicSetup,
        markdown(),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ "aria-label": "Note text (Markdown)" }),
        theme,
        syntaxHighlighting(highlight),
        EditorView.updateListener.of((u) => {
          if (u.docChanged && !this.silent && this.id) {
            this.emit({ t: "edit", id: this.id, text: u.state.doc.toString() });
          }
        }),
      ],
    });
  },
  // Replace the editor content with a note's text; resets the undo history.
  setDoc(id, text) {
    if (!this.view) return;
    this.silent = true;
    this.id = id;
    this.view.setState(this.newState(text));
    this.silent = false;
  },
  // Drop the content (lock, close note): a fresh empty state without history.
  clear() {
    this.id = null;
    if (this.view) {
      this.silent = true;
      this.view.setState(this.newState(""));
      this.silent = false;
    }
  },
  destroy() {
    this.clear();
    if (this.view) this.view.destroy();
    this.view = null;
  },
  focus() { if (this.view && this.id) this.view.focus(); },
};

// Keyboard shortcuts, in the capture phase so the editor cannot swallow them.
const SHORTCUTS = {
  n: "new", k: "search", l: "lock", s: "save", p: "preview",
};
document.addEventListener(
  "keydown",
  (e) => {
    const mod = e.ctrlKey || e.metaKey;
    const name = mod && !e.altKey ? SHORTCUTS[e.key.toLowerCase()] : null;
    if (!name) return;
    if (name === "preview" && !e.shiftKey) return; // Ctrl+P (print) is left alone
    if (name !== "preview" && e.shiftKey) return;
    e.preventDefault();
    e.stopPropagation();
    nv.emit({ t: "key", name });
  },
  true,
);

// Activity for the inactivity timer, throttled.
let lastAct = 0;
for (const ev of ["keydown", "mousedown", "mousemove", "wheel", "touchstart"]) {
  document.addEventListener(
    ev,
    () => {
      const now = Date.now();
      if (now - lastAct > 2000) { lastAct = now; nv.emit({ t: "act" }); }
    },
    { capture: true, passive: true },
  );
}

// Links in the preview never navigate the webview. Rust decides what to do with the URL.
document.addEventListener(
  "click",
  (e) => {
    const a = e.target instanceof Element ? e.target.closest("#preview a") : null;
    if (!a) return;
    e.preventDefault();
    e.stopPropagation();
    const href = a.getAttribute("href") || "";
    if (href.startsWith("#")) {
      // Footnote anchors: scroll within the preview, nothing else.
      const target = document.getElementById(href.slice(1));
      if (target) target.scrollIntoView();
      return;
    }
    nv.emit({ t: "link", href });
  },
  true,
);
// Middle-click and drag of links are not navigations either.
document.addEventListener("auxclick", (e) => {
  if (e.target instanceof Element && e.target.closest("#preview a")) e.preventDefault();
}, true);
document.addEventListener("dragstart", (e) => {
  if (e.target instanceof Element && e.target.closest("#preview a")) e.preventDefault();
}, true);

window.nv = nv;
