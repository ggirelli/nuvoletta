# ADR 0006: Android application

Status: Proposed (Phase 6; builds verified, not yet run on a device)
Date: 2026-11-15

## Context

Phase 6 brings the desktop app (ADR 0004) to Android with the same vault format and no weaker
security model (PLAN.md sections 10 and 11: minimum API 28, passphrase-only unlock, no Keystore or
biometrics in the MVP). The sync MVP has a single backend, an externally synced folder (SYNC_PROTOCOL
section 8), so Android sync needs a user-chosen folder.

## Decision

1. **Same app, mobile shell.** `apps/nuvoletta-app` builds for Android with `--features mobile`
   (`dioxus::mobile` is the same wry/tao shell as desktop). All behaviour stays in
   `nuvoletta-app-core`; the platform glue is `apps/nuvoletta-app/src/platform.rs`. Built with `dx`
   (`just android`). Application id `dev.nuvoletta.app`, `minSdk` 28, `targetSdk` and `compileSdk`
   34.
2. **Storage.** The vault and `settings.json` live in the app-private `files` directory
   (`/data/user/<n>/dev.nuvoletta.app/files`), unreadable to other apps, with no storage permission.
   The path is derived from the process name and uid (pure functions in `nuvoletta-app-core`,
   tested), not through JNI, so no `unsafe` is needed. The vault location is fixed: there is no
   folder picker, and an existing vault is opened at start-up. `android:allowBackup="false"` keeps
   the vault out of Google and adb backups and device-to-device transfer.
3. **Permissions: INTERNET only, and it is not for the network.** Dioxus 0.7 sends every UI update
   to the webview over a WebSocket on `127.0.0.1` (`dioxus-desktop/src/edits.rs`; it replaced
   long-polling because of wry bugs on Android). Android refuses to create any IP socket, loopback
   included, without `INTERNET`, so the app cannot start without it. What limits it: the page's
   Content-Security-Policy allows `connect-src` to `ws://127.0.0.1:*` only (ADR 0004), the
   generated network security config allows cleartext to `127.0.0.1` only, the app contains no
   network code, and the sync backend is a local folder. The loopback listener is reachable by other
   apps on the device; Dioxus protects it with a random per-run key in the URL, as on desktop.
   Declared permissions are checked in review (`aapt2 dump badging`, see `docs/development.md`).
   Removing the need for `INTERNET` would require Dioxus to use an in-process channel on Android;
   that is an upstream change.
4. **Lifecycle: save, then lock, as soon as the app leaves the foreground.** On tao's `Suspended`
   event (Android `onPause`: home, app switch, screen off, incoming call) the app calls
   `App::on_background()`: unsaved edits are saved, then the vault is locked and the editor is
   destroyed. This is deliberately stricter than the inactivity timer, because a backgrounded
   process can be frozen or killed at any time, timers do not run reliably, and the monotonic clock
   used for the timer does not advance in deep sleep. The cost is that switching to another app
   (for example to copy something) means entering the passphrase again; with passphrase-only unlock
   this is the safe default and a grace period can be added later as an explicit setting. If the
   save fails, nothing is locked and nothing is discarded: the vault stays unlocked and the
   "could not lock" dialog is shown on return (ADR 0004, decision 2). Sync (when it exists on
   Android) is safe to interrupt by design (SYNC_PROTOCOL 6.3); a cycle in progress finishes its
   step and the lock follows when the vault is handed back.
5. **Screenshots and recents.** A custom `MainActivity` (Kotlin, `apps/nuvoletta-app/android/`,
   wired through `Dioxus.toml`) sets `FLAG_SECURE` before anything is shown, hides the note content
   from screenshots, screen recordings and the recents thumbnail, and keeps the WebView away from
   the platform autofill framework and the HTTP cache. Kotlin is the only way to reach these window
   flags without `unsafe` JNI in our crates; the file is short and replaces dx's generated one.
6. **Small screens.** One layout, CSS-driven: below 760 px the note list and the open note take
   turns (a *Notes* back button), the top bar wraps with the search on its own row, folder, tags and
   delete sit behind a *Details* toggle, and editor and preview take turns. On touch screens
   (`pointer: coarse`) controls are at least 44 px and inputs 16 px (no zoom on focus). The only
   state is a UI flag; all logic stays in app-core.
7. **Sync is disabled on Android in this version** and the UI says why (see below). Export and
   import are also hidden: they need folder access, and a plaintext export to an arbitrary path
   should not be offered where the user cannot see where it goes.

## Sync on Android: assessment

The synced-folder backend needs a folder shared with an external sync app (Syncthing, a Nextcloud
or Dropbox client, ...). On Android a folder outside the app's private storage is reachable
only through the Storage Access Framework (SAF), or with the all-files permission
(`MANAGE_EXTERNAL_STORAGE`), which is far too broad and is refused on the Play Store for this kind of
app.

Can SAF meet the folder provider's guarantees (SYNC_PROTOCOL section 8 and `Provider`)?

| Requirement | SAF (`DocumentsContract`) |
|---|---|
| Listing with sizes, regular files only | Yes: `COLUMN_SIZE`, `COLUMN_MIME_TYPE` (directory or not) from a children query. No recursive listing and no path lookup: each directory is a separate query, and a name is resolved by listing every level. Needs a document-id cache. |
| Create-only | **No exclusive create.** `createDocument` on an existing name usually succeeds with a renamed file (`name (1)`), so a collision is detected afterwards by comparing the returned display name, then deleting the stray. The protocol already allows this (`create_only` false is "best effort", SYNC_PROTOCOL 1.6), but the guarantee is weaker than the Unix hard-link path. |
| Atomic publish (temp, then rename) | `renameDocument` is optional per provider (`FLAG_SUPPORTS_RENAME`); where present it is the temp-then-rename the protocol wants, with the same auto-rename caveat on collision. A provider without rename cannot be used safely and would have to be refused. |
| No symlink escape | SAF exposes no link information; the provider decides what a document is. The folder backend's explicit refusal of symlinks cannot be reproduced; trust moves to the DocumentsProvider. |
| Case handling | Names are reported as stored; whether `A` and `a` collide depends on the volume. The protocol's names are lowercase hex, so this is manageable, but a case-insensitive volume must be treated as equal names. |
| Sync-app side | The sync app needs its own access to the same directory (its own SAF grant or all-files permission). That is a property of the sync app, not of Nuvoletta. |

**Conclusion: SAF can in principle satisfy the protocol** (which tolerates best-effort create-only
and requires rename or equivalent), **but it was not implemented in this change**, for these
reasons:

1. It needs Rust to call the Android Java API (`ContentResolver`, `DocumentsContract`, cursors,
   streams). The `jni` crate is in the tree (wry, tao) but obtaining the `JavaVM` and the activity
   context needs `unsafe`, which is forbidden in our code. Using a wrapper dependency that confines
   the `unsafe` (for example `robius-android-env`) is acceptable under the project rules but is a
   new dependency whose review and `cargo deny` footprint has not been done.
2. Choosing the folder needs `ACTION_OPEN_DOCUMENT_TREE`, a result callback in the activity, and
   `takePersistableUriPermission`: Kotlin glue in the custom `MainActivity` and a channel back to
   Rust. The hook now exists but the code does not.
3. Nothing of it can be tested here. There is no emulator or device, and no off-device fake of a
   `DocumentsProvider`. Code on the sync integrity path that has never run is worse than no code.
4. The per-level listing makes a naive `Provider` slow on a large folder (the `o/` object tree has
   up to 256 shards); a document-id cache is part of the design, not an afterthought.

Recommended next step (a separate change, with a device): put the provider logic (name to document
resolution with a cache, create then verify the returned name, temp then rename, listing filter)
behind a small Rust trait that models a document tree, unit-test it against an in-memory fake
(collisions, rename refusal, case-insensitive volumes, interrupted steps), and bind the trait to
Kotlin through one confined JNI dependency. Alternatives, in the owner's hands:
WebDAV (**since implemented**, ADR 0007: Android sync uses it; it makes real use of `INTERNET`, from
the Rust side only, the page CSP is unchanged), or a build variant with all-files access for sideloaded use only (broad permission, not
recommended).

Until then the Android vault is a stand-alone vault. The format is identical, so a vault can be put
in place with `adb` (`docs/development.md`), but there is no in-app way to move notes between a
phone and a desktop.

## iOS assessment (no implementation)

Assessed from documentation only; nothing was built (no macOS or Xcode here).

- **Framework:** Dioxus mobile uses wry on `WKWebView` for iOS, so the same app should build with `dx
  --platform ios`, on a Mac with Xcode only. A nightly `macos-latest` job could check that it builds
  for the simulator; running it needs a Mac or device.
- **Storage and lifecycle:** the app sandbox (`Application Support`, with data protection) is the
  equivalent of Android's private files directory. The same save-then-lock policy applies on
  backgrounding (tao reports `Suspended`). There is no `FLAG_SECURE`: hiding the app-switcher
  snapshot needs a cover view added from Swift or Objective-C glue.
- **Sync is the weak point.** iOS has no shared-folder model for third-party sync tools. Folders
  outside the sandbox come from Files providers through the document picker with security-scoped
  bookmarks, access goes through file coordination, and cloud files can be placeholders that are not
  downloaded yet; there are no hard links and no exclusive create. An iCloud Drive container would
  sync only between Apple devices, which breaks the cross-platform goal. With WebDAV deferred there
  is no viable iOS sync backend today.
- **Other:** Keychain would allow stored credentials later; distribution needs the Apple developer
  programme and export-compliance answers for the encryption; the Android SAF design above would
  need a parallel Files-provider layer (and again cannot be tested without a device).
- **Recommendation:** do not start iOS before the WebDAV backend exists (or an owner decision to
  accept a stand-alone iOS vault). Revisit then, with a Mac available for testing.

## Not done (Phase 6 note on credentials)

No Android Keystore or biometric unlock: passphrase-only, per PLAN.md section 11. There are no
stored credentials to protect (no network backend).

## Consequences

- Positive: one code base and one test suite; no `unsafe` in our code; minimal permissions;
  conservative lifecycle.
- Negative: `INTERNET` is declared for Dioxus's own IPC; the webview holds plaintext as on desktop
  (ADR 0003, THREAT_MODEL N6); the keyboard app may learn what is typed in the editor (Android
  offers no per-field control from a WebView without a custom input connection; use a keyboard
  with incognito mode if this matters); the clipboard is system-wide and shared with other apps;
  no sync until SAF lands.
- Unverified until run on a device: the lifecycle event, `FLAG_SECURE`, the private path, soft
  keyboard behaviour with CodeMirror, performance, and that no plaintext reaches logcat.
