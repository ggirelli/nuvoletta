# Dioxus feasibility spike (Phase 0)

Crate: `spikes/dioxus-editor/` (standalone, own `[workspace]`). Dioxus 0.7.10, dx 0.7.10, cargo 1.99.

## Results

| Item | Result |
|---|---|
| Desktop `cargo build`, `dx build --platform desktop` | OK (~37 s cold) |
| Desktop launch (`timeout 12 cargo run`, DISPLAY=:0 / wayland present) | Ran 12 s with no panic/stderr. **A GUI was not visible to me**, so the CodeMirror mount, eval round-trip, and layout are **unverified visually**. |
| `dx build --platform android --target aarch64-linux-android --no-default-features --features mobile` | OK first try. 70 MB debug APK (241 MB debug `libmain.so`), contains `editor-*.js` and CSS assets. **Not run on an emulator/device** (no attempt made). |
| Markdown preview, raw HTML escaped | OK. Unit test `raw_html_is_escaped` passes: `<script>`, `<img onerror>`, inline `<b onclick>` are emitted as escaped text. |
| keyring (Linux Secret Service) set/get/delete | OK (`cargo test keyring_and_file -- --nocapture`, keyring 3 `sync-secret-service` + `crypto-rust`). Needs a running Secret Service (gnome-keyring/KWallet); headless/CI hosts will fail, handle `NoEntry`/`PlatformFailure`. |
| File in app-private dir | OK on desktop (`~/.local/share/nuvoletta-spike/note.md`). Android path is hardcoded and **untested**; real app must get `Context.getFilesDir()` via JNI (or dx/wry helpers). |

## Commands

```
cd spikes/dioxus-editor
(cd js && npm install && npm run build)        # rebuilds assets/editor.js (esbuild, minified IIFE)
cargo test -- --nocapture
dx build --platform desktop
dx serve --platform desktop
ANDROID_HOME=~/Android/Sdk dx build --platform android --target aarch64-linux-android --no-default-features --features mobile
```

## Editor approach

CodeMirror 6 (`codemirror` + `@codemirror/lang-markdown`) bundled with esbuild into one 599 KB minified file, vendored at `assets/editor.js` and loaded via `asset!()` + `document::Script` (no CDN; works offline). Rust <-> JS: `document::eval` with `dioxus.send(text)` on each change, Rust `ev.recv::<String>()` updates a signal that feeds `pulldown-cmark`. Rust -> JS (e.g. load note, `setText`) would use `eval` calling `window.nvEditor.setText`; written in the JS API but not exercised in Rust.

Known weaknesses of the bridge: full-document string sent on each keystroke (fine for notes, wasteful for huge ones; debounce or send diffs); mount is guarded by polling for the bundle; reloading a different note needs explicit `setText`.

### Security implications
- Note plaintext lives in the webview JS heap (CodeMirror state, eval IPC strings) and the webview process (WebKitGTK / Android System WebView) memory. It cannot be zeroized; the Rust-side "zeroize secrets" goal in the plan does not extend to the editor buffer. Locking the vault can only drop references and destroy the view, not guarantee scrubbing.
- Preview is rendered by Rust with raw HTML converted to text, then injected with `dangerous_inner_html`. That is safe only because the pulldown-cmark event stream is filtered. Link/image URLs are NOT filtered yet: `[x](javascript:alert(1))` produces an `href`; add a scheme allow-list (and strip/proxy remote images) before shipping. Consider a CSP on the webview too.
- JS supply chain: 4 direct deps, ~100 packages in the tree under `npm ls --all` (mostly CodeMirror/lezer/@marijn scoped); runtime bundle is a single vendored file, so exposure is at build time. Pin with `package-lock.json`, review on update, consider committing the bundle plus a build hash.
- Alternative without JS: plain `<textarea>` (no extra JS, still in webview memory) or a native Rust editor (not available in Dioxus today). Textarea is a weaker editing UX (no syntax highlighting, poor mobile selection handling in some webviews).

## Keyring

- Linux: works via Secret Service (D-Bus). Dependency-light with `crypto-rust`; no libsecret needed with `sync-secret-service`.
- Android: `keyring` has no Android backend (the crate is excluded via cfg in this spike). Needed: Android Keystore via JNI (`jni` crate, already in the tree through wry/tao) or a small Kotlin/Java helper class compiled into the APK. Keystore cannot store arbitrary secrets directly: generate a non-exportable AES-GCM key in Keystore (optionally with `setUserAuthenticationRequired` for biometric/PIN), and use it to wrap the vault key blob stored in app-private files. Dioxus 0.7 has no first-class Java source support in the dx Android template, so a custom Gradle/Java patch step is likely required. Not prototyped.

## Recommendation: GO with caveats

Dioxus builds cleanly for desktop and Android arm64 and the Rust-side pieces (pulldown-cmark, keyring, file IO) work. Caveats:
1. Visual/interactive behaviour (CodeMirror mounting, eval bridge, mobile soft-keyboard editing) is unverified; run on a real display and an emulator before committing. Mobile editing in WebView with CodeMirror 6 has known IME/selection quirks.
2. Android Keystore needs custom JNI/Java glue; Android app-private path needs JNI too.
3. Plaintext in webview memory is inherent to this architecture; decide whether acceptable for the threat model.
4. Large debug artifacts (70 MB APK); release size not measured.
5. Add URL scheme sanitization in the preview.

## Open questions for the owner
- Is plaintext in the webview process acceptable for the "encrypted by default" threat model (e.g., memory dumps on a rooted device, WebView crash reports)?
- Is a hard requirement on Android Keystore / biometric unlock, or is passphrase-only fine for MVP?
- Is a ~600 KB vendored JS editor bundle plus npm build step acceptable, or prefer a textarea-only MVP?
- Which Android minimum API / emulator target? (Only arm64 built here.)
- Should remote images in notes be blocked entirely (privacy leak via image fetch)?
