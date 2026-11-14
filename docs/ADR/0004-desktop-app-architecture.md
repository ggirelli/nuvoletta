# ADR 0004: Desktop app architecture and webview hardening

Status: Proposed (Phase 3, owner decisions recorded, not yet reviewed)
Date: 2026-11-14

## Context

Phase 3 delivers the Dioxus desktop app. It must be testable without a webview, must not weaken the
vault's guarantees (PLAN.md section 10), and runs untrusted note content inside a webview that also
holds decrypted plaintext (ADR 0003).

## Decision

1. **Split.** `crates/nuvoletta-app-core` holds all non-UI logic (session state machine, open-note
   buffer with dirty tracking, autosave debounce, inactivity lock, index upkeep, queries, preview,
   export/import, settings, link classification, the CSP string). Time comes from an injectable
   `Clock`, so tests run without sleeping. `apps/nuvoletta-app` only renders it and forwards input.
2. **Locking never discards.** Locking and navigation save first. If saving fails the vault stays
   unlocked, the buffer is kept, the error is shown, and only an explicit user choice may lock and
   discard. Auto-lock follows the same rule (it reports instead of locking).
3. **Stale editor messages are ignored.** Every edit message carries the note id; the core drops edits
   for a note that is not open, so a late message cannot overwrite another note.
4. **Everything embedded.** Styles, the vendored editor bundle and the brand images are compiled into
   the binary and inlined into the document head (`data:` URIs for images). The app does not use `dx`
   asset serving, so `cargo run` and a plain `cargo build` produce a working binary, and the page loads
   nothing by URL.
5. **Content-Security-Policy** (`nuvoletta_app_core::CONTENT_SECURITY_POLICY`, delivered as a meta
   tag first in `<head>`): `default-src 'none'`; images `'self' data:`; `connect-src 'self'
   ws://127.0.0.1:*`; no frames, workers, media, objects, forms or `<base>`; `script-src-attr 'none'`.
   Constraints of Dioxus 0.7 desktop that force exceptions:
    - `script-src-elem 'unsafe-inline'`: Dioxus injects an inline bootstrap script containing a
      per-run key (not pin-able by hash or nonce).
    - `script-src 'unsafe-eval'`: `document::eval`, the channel between Rust and the editor, runs its
      snippets through the `AsyncFunction` constructor.
    - `connect-src ws://127.0.0.1:*`: Dioxus streams UI edits over a loopback WebSocket.
    - `style-src 'unsafe-inline'`: CodeMirror and Dioxus insert `<style>` elements.
   Measured in WebKitGTK with a probe page: remote fetch, image, script, frame and WebSocket and
   inline event handlers are blocked; `javascript:` URLs are **not** blocked by CSP there. The layers
   that cover it: the renderer never emits such links, and the app cancels every click on a preview
   link before the webview acts and handles it in Rust. WebView2 and WKWebView were not tested (no
   such hosts here); the nightly CI builds them but cannot check CSP behaviour.
6. **Links.** `nuvoletta://note/<id>` opens the note in the app; `http`, `https` and `mailto` open the
   system browser only after a click; everything else is ignored. A navigation handler that refuses
   all non-app navigation is a backstop.
7. **Unlock is passphrase-only**; platform credential-store integration is deferred (PLAN.md section
   11). Settings hold no secrets.
8. **Dependencies.** The Dioxus desktop tree adds MPL-2.0 (webview stack, `dirs`) and NCSA licences
   and three "unmaintained" advisories (`fxhash`, `paste`, `proc-macro-error`), all build-time or
   webview-side; `deny.toml` records each exception. No crate that handles keys or notes is affected.

## Consequences

- Positive: the core workflows are covered by fast, deterministic tests; the binary is self-contained.
- Negative: the CSP is weaker than ideal on script execution (above); the 600 KB editor bundle is
  inlined on every page load; the editor and form text live in the webview's memory.
- Negative: no `dx` asset hashing, so resources are not content-addressed (irrelevant offline).

## Alternatives considered

- `dx` asset pipeline: needs `dx bundle` to run at all, so a plain build does not start.
- Avoiding `document::eval` (to drop `'unsafe-eval'`): would need raw `ipc.postMessage` messages that
  Dioxus does not route to application code.
- A `<textarea>` editor: no extra JS, but a much weaker editing experience.
