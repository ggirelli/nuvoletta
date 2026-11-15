# Desktop and Android app

The app (`apps/nuvoletta-app`) is a [Dioxus](https://dioxuslabs.com/) application for Linux,
macOS and Windows, and for Android (see [Android](#android)). It is a thin view over `crates/nuvoletta-app-core`, which holds all behaviour
(session, autosave, auto-lock, search, export and import, sync scheduling and conflict handling) and
is tested without a webview.

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

## WebDAV sync

The *Sync* dialog has a *WebDAV* section (the only sync option on Android). Enter the folder address
(`https://…`; plain `http://` is refused), user name and password (an app password where the server
offers them) and press *Check account and save*. The check runs in the background and tries what the
account may do. If it can **delete or overwrite** files, or its rights cannot be determined, it is
**refused and not saved**, with an explanation: a stolen sync password could otherwise wipe out your
synced notes; create an account or share that can read and create files but not change or delete
them ([setup guide](WEBDAV.md)). If the server's certificate is not signed by a known authority (a
NAS), the dialog shows its SHA-256 fingerprint without having sent your password; after you confirm
that it matches what the server shows, exactly that certificate is trusted for the address. The
address, user name and password are stored encrypted on this device only
([storage format](STORAGE_FORMAT.md)), are not synced, and are readable only while the vault is
unlocked; enter them on each device. The account is checked again at the first sync of every
session. *Forget these credentials* removes them from this device. On WebDAV, *Clean up old versions*
is unavailable (the account cannot delete), and an upload cut off by a network failure that leaves a
damaged file is worked around automatically after ten minutes (a notice explains it).

## Sync

Sync is optional and off until you choose a folder (desktop) or set up WebDAV (desktop and Android). The app only reads and writes files in that
folder; your sync tool (Syncthing, Dropbox, iCloud, a Nextcloud client, …) moves them between
devices. Only encrypted files are written there. The protocol is in
[SYNC_PROTOCOL.md](SYNC_PROTOCOL.md).

1. **Choose the folder.** Open *Sync* in the top bar, pick the folder (typed or *Browse…*) and press
   *Use this folder*. It must exist and be separate from the vault folder. The choice is stored on
   this device only (see [storage format](STORAGE_FORMAT.md)), never in the vault's synced data, and
   *Turn sync off* keeps the folder but stops syncing. Changing to a different folder asks for a
   confirmation: a folder that is empty or older than what this device has seen is reported as a
   rollback warning.
2. **When it runs.** *Sync now*, automatically about 5 seconds after a save, when the vault is
   unlocked, and every 5 minutes while unlocked. Never while locked (the keys are needed), and not
   while you have unsaved typing. The top bar shows *Syncing…*, *Synced 3 min ago*, *Not synced yet*,
   *Sync: off*, or *Sync: problem* / *N conflicts* / *N warnings*.
3. **It does not block the window.** A cycle runs on a worker thread that has the vault to itself
   (otherwise it could overwrite an edit). Meanwhile you can type, scroll the note list, search and
   preview; actions that write notes (new note, rename, delete, switching notes) say "try again in a
   moment", and autosave simply waits until the cycle ends. If a sync brings a new version of the
   note you have open, the editor reloads it; if you were typing, your text is kept and the app says
   so (saving then replaces the synced version, which stays in the sync history). If another device
   deleted the open note while you were editing it, the note is restored from your text and shows up
   as a conflict at the next sync.
4. **Locking during a cycle.** *Lock* (or the inactivity timer) while a cycle runs waits for it to
   finish, then saves and locks; nothing is discarded. Closing the window waits for the cycle too.
   Cycles are safe to repeat, so a crash in the middle loses nothing.
5. **Needs attention.** The *Sync* dialog lists notes with conflicting versions and notes that
   need attention for other reasons (history frozen after a device ID was used twice; the newest
   version has not arrived yet).
6. **Resolving a conflict.** *Review* shows every version in a fixed order (the same on all
   devices), who wrote it, and the changed lines compared with this device's version. *Use version N*
   makes it the note; *Use version N and edit* also opens it in the editor, which is how you merge by
   hand (the edit then syncs as an ordinary change). With exactly two edited versions, *Keep version
   N, save the other as a copy* keeps both (the copy is titled "… (conflict copy)"). If one side
   deleted the note, choose *Keep the note* or *Delete the note*. Every choice keeps the other
   versions in the sync history, so nothing is lost.
7. **Integrity warnings** (the folder went back in time, a device ID was used twice, entries may be
   withheld or corrupted, a newer vault header exists) are shown prominently with what they mean and
   what to do. They are stored on this device and stay, across cycles and restarts, until you press
   *Acknowledge*; the app never dismisses them on its own and never changes data because of them.
8. **Diagnostics.** The *Sync* dialog lists files in the sync folder that are not Nuvoletta files
   (including copies made by your sync tool) and invalid entries. This is report only: nothing is
   deleted or modified.
9. **Clean up old versions.** The *Sync* dialog has a *Clean up old versions* section. *Preview
   clean-up* shows what would be deleted from the sync folder (counts and sizes) without deleting
   anything; *Delete old versions* needs an explicit tick of "I understand that deleted versions
   cannot be brought back". Afterwards the result is shown. Only versions this device wrote can be
   deleted, only old ones that every device active in the last 90 days has seen, at least 30 days
   after the newer version was written; the newest 5 versions of every note, the current versions and
   what they were based on, and everything in a conflict are always kept (details in
   [SYNC_PROTOCOL.md](SYNC_PROTOCOL.md) section 9). Log files are never deleted. It is manual, works
   from what the last sync learned, and is unavailable while a sync cycle runs, while the vault is
   locked, without a sync folder, and while a device ID is reported as used twice. A clean-up that is
   interrupted is safe to repeat.
8. **Recently deleted.** The sidebar's *Recently deleted* button lists deleted notes, newest first,
   with title, path and when (and, for synced notes, by which device, short id). Two sources are
   merged and deduplicated by note id. A note that sync has seen is deleted through a tombstone;
   *Restore* writes a new version of its last content whose parent is the deletion (the same
   mechanism as *Keep the note* for delete-versus-edit), so every device gets the note back after
   its next sync. If the content is not on this device yet the entry says so and Restore is
   disabled until a sync brings it. A note that sync has not seen (a vault without sync, or a note
   deleted before any sync scan) sits in the vault's local trash (`STORAGE_FORMAT.md` section 3.1):
   *Restore* moves the file back; if a live note already has that id the old version returns as a
   separate copy titled "... (restored)", never overwriting. A sync-tracked note is never restored
   by moving a file back. Local trash entries are removed automatically 30 days after deletion
   (checked at unlock and when the view opens); there is no "empty trash" and no per-item
   delete, and synced content follows the cleanup rules above. The dialog text says so. The logic
   is `App::recently_deleted` and `App::restore_note` in app-core, on `Unlocked::list_trash` /
   `restore_from_trash` and `Syncer::deleted_notes` / `restore_note`.

## Android

The same app builds for Android (minimum Android 9, API 28) with `just android`; see
[Development](development.md) for the toolchain. **This version has been built but not yet run on a
device**; the list of what needs testing is in [PLAN.md](plan.md) (Phase 6). The design is in
[ADR 0006](ADR/0006-android-app.md).

- **Storage.** The vault lives in the app's private storage, which other apps cannot read; there is no
  folder to choose and no storage permission. It is excluded from Android backups. The first start
  asks you to create the vault; later starts go straight to unlocking it.
- **Permission.** The app declares only `INTERNET`, which Android requires for the local channel
  between the app and its webview. It makes no network connections.
- **Locking.** The vault is **saved and locked as soon as the app leaves the screen** (home, app
  switcher, screen off, a call). Coming back asks for the passphrase. If the save fails, nothing is
  locked or discarded and the app tells you when you return. The inactivity timer still applies
  while the app is open.
- **Screenshots.** Screenshots, screen recording and the recent-apps preview show nothing of the
  app (`FLAG_SECURE`).
- **Small screens.** The note list and the open note take turns (*‹ Notes* goes back), the top bar
  wraps, *Details* shows folder, tags and delete, and *Preview* swaps the editor for the preview.
- **Not available yet:** sync (Android needs a folder an external sync app can reach, which needs the
  Storage Access Framework; see ADR 0006) and export and import (they need folder access). Until
  sync exists an Android vault is stand-alone.
- **Limits.** The webview holds plaintext as on desktop. Your keyboard app may learn what you type
  in the editor (use a keyboard with an incognito mode if that matters), and the Android clipboard is
  shared with other apps.

## Keyboard

| Shortcut | Action |
|---|---|
| ++ctrl+n++ (++cmd+n++) | New note |
| ++ctrl+k++ (++cmd+k++) | Focus the search box |
| ++ctrl+s++ (++cmd+s++) | Save now |
| ++ctrl+shift+p++ (++cmd+shift+p++) | Show or hide the preview |
| ++ctrl+l++ (++cmd+l++) | Lock the vault |
| ++tab++ | Move through the controls in reading order; a *Skip to editor* link comes first |
| ++esc++ | Close the settings or sync dialog (or go back from a conflict to the list) |

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
- **Sync settings and warnings** (the sync folder path, and the integrity warnings you have not
  acknowledged) are stored unencrypted in the vault's `.nuvoletta/device-state/` on this device, are
  never uploaded, and contain no keys or note content (warnings name devices by a short id and
  sequence numbers, which are visible in the sync folder's file names anyway).
- **Sync and the webview.** Sync adds no network access to the page: the worker thread only reads
  and writes the chosen local folder, and the Content-Security-Policy is unchanged. The conflict view
  shows note text, so it has the same plaintext-in-the-webview limit as the editor.
- **Not available yet:** removing deleted notes' last versions from the sync folder, editing a merge directly inside the conflict view (use *Use version N and edit*), a keyboard
  shortcut for *Sync now* (use the *Sync now* button) and providers other than a synced folder and WebDAV.
