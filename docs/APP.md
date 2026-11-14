# Desktop app

The desktop app (`apps/nuvoletta-app`) is a [Dioxus](https://dioxuslabs.com/) application for Linux,
macOS and Windows. It is a thin view over `crates/nuvoletta-app-core`, which holds all behaviour
(session, autosave, auto-lock, search, export and import) and is tested without a webview.

## Running

```sh
just app          # cargo run -p nuvoletta-app
```

On Linux the build needs GTK 3 and WebKitGTK 4.1 development packages (Debian and Ubuntu:
`libwebkit2gtk-4.1-dev libgtk-3-dev libxdo-dev libayatana-appindicator3-dev`). `just test-core` and
`just build-core` skip the app and need none of them. Everything the page needs (styles, the editor
bundle, brand images) is compiled into the binary; there are no files to install next to it.

## Using it

1. **Pick a vault.** Create a new vault in an empty folder, or open an existing one. The folder can
   be typed or chosen with *Browse…* (which needs a desktop file dialog; the text field always works).
2. **Write down the recovery key.** A new vault shows its recovery key once. It is the only way in
   if the passphrase is lost, and nobody can recover it. The app does not continue until you confirm
   you have written it down. *Settings* can replace it later.
3. **Unlock** with the passphrase, or with the recovery key; after a recovery-key unlock the app asks
   for a new passphrase before anything else.
4. **Write.** Notes live in a tree by virtual path (folders are the `/`-separated part of the
   path). The editor is on the left and a live preview on the right. Notes are saved automatically
   about a second after you stop typing; the top bar shows *Saved*, *Unsaved changes* or *Not
   saved*.
5. **Link and find.** `[[Another note]]` links notes; clicking a link in the preview opens the note.
   The panel under the editor lists backlinks and unresolved links; the sidebar lists tags; the search
   box finds text in titles, paths, tags and bodies.

## Keyboard

| Shortcut | Action |
|---|---|
| ++ctrl+n++ (++cmd+n++) | New note |
| ++ctrl+k++ (++cmd+k++) | Focus the search box |
| ++ctrl+s++ (++cmd+s++) | Save now |
| ++ctrl+shift+p++ (++cmd+shift+p++) | Show or hide the preview |
| ++ctrl+l++ (++cmd+l++) | Lock the vault |
| ++tab++ | Move through the controls in reading order; a *Skip to editor* link comes first |
| ++esc++ | Close the settings dialog |

Controls have text labels, dialogs are announced as dialogs, errors use `role="alert"`, and
colours follow the system light or dark theme with the contrast ratios of the brand palette.

## Locking

The vault locks when you press *Lock* or ++ctrl+l++, and after a period of inactivity (default 5
minutes; *Settings* sets 1 to 1440 minutes; there is deliberately no "never"). Locking saves unsaved
edits first, then drops the keys, the search index and the editor content.

If the save fails (for example the disk is full), **the vault is not locked and nothing is
discarded**. A dialog explains why and offers to retry, keep working, or lock and discard the unsaved
edits, which only happens if you choose it. Closing the window also saves first, on a best-effort
basis.

## Export and import

*Settings* can export all notes as plain Markdown files. **Exported files are not encrypted** and
stay on disk after the vault is locked; the app asks you to confirm before exporting. Import reads a
folder of `.md` files and never overwrites an existing note. See the
[export format](EXPORT_FORMAT.md).

## Security notes

- **Plaintext is in the webview.** The editor is CodeMirror 6 running in the system webview, so the
  text you are editing, the rendered preview and anything you type into a form (including the
  passphrase) exist in the webview's memory. Rust-side secrets and notes are zeroized on drop, but
  the webview's copies cannot be. Locking destroys the editor and clears its history; it cannot
  promise the memory is scrubbed. See the [threat model](THREAT_MODEL.md).
- **Preview.** Notes are rendered by `nuvoletta-markdown`: raw HTML is shown as text, only `http`,
  `https` and `mailto` links are allowed, and remote images are replaced by a placeholder, so opening
  a note never makes a network request. Links never navigate the app: a click on a note link opens
  the note, a click on an `http(s)` or `mailto` link opens the system browser, and the webview itself
  refuses every other navigation.
- **Content-Security-Policy.** The page ships a strict policy (`default-src 'none'`, images only from
  the app and `data:`, no remote hosts, no frames, forms, plugins or `<base>`). Verified in
  WebKitGTK: remote `fetch`, images, scripts, frames and WebSockets are blocked, and inline event
  handlers are blocked. What Dioxus 0.7 desktop forces us to allow, and what that means, is in
  [ADR 0004](ADR/0004-desktop-app-architecture.md).
- **No credential store.** Unlock is passphrase-only; platform credential-store integration is
  deferred (PLAN.md section 11).
- **Settings** (auto-lock period and the last vault folder) are stored unencrypted in the user's
  configuration directory. They contain no secrets.
