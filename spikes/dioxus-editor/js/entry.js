import { EditorView, basicSetup } from "codemirror";
import { markdown } from "@codemirror/lang-markdown";

// Minimal bridge: window.nvEditor.mount(el, text, onChange) / getText / setText.
window.nvEditor = {
  view: null,
  mount(el, text, onChange) {
    this.view = new EditorView({
      doc: text,
      parent: el,
      extensions: [
        basicSetup,
        markdown(),
        EditorView.lineWrapping,
        EditorView.updateListener.of((u) => {
          if (u.docChanged) onChange(u.state.doc.toString());
        }),
      ],
    });
  },
  getText() { return this.view.state.doc.toString(); },
  setText(t) {
    this.view.dispatch({ changes: { from: 0, to: this.view.state.doc.length, insert: t } });
  },
};
