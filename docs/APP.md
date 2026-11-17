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

1. **Pick a vault.** Create a new vault in an empty folder, open an existing one, or [join](#joining-an-existing-vault) one that is already synced from another device. The folder can
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

## Joining an existing vault

A new device, or one restored with no vault, joins from the sync location instead of copying
anything by hand (`docs/ADR/0008-device-join-and-header-adoption.md`). On the first screen choose
*Join an existing vault*:

1. **Sync location.** A synced folder (desktop) or a WebDAV address, user name and password
   (desktop and Android). A WebDAV account goes through the usual check first and is refused if it
   can delete or overwrite. Only the `h/` folder is listed and only header-sized files are read;
   no log, object or note is read before the vault is unlocked, and nothing is written.
2. **Vault.** If the location holds headers of more than one vault (a shared folder, or a file
   someone added), the vault fingerprint (the vault id in groups of four hex digits) tells them
   apart; compare it with *Settings, This vault* on a device that has the vault.
3. **Passphrase or recovery key.** Type the passphrase of the **current** header, or the recovery
   key. The header is opened in memory: KDF parameters range-checked, the data key unwrapped, the
   header MAC verified. A header whose name, size or content is wrong, or that your secret does not
   open, is never used, and the error is the same generic one as for a wrong passphrase. Only the
   newest generation is tried; a header that does not open is not silently replaced by an older
   one. If the newest does not open, the user may allow an older header explicitly (the screen
   warns that the passphrase typed is then not the current one). If the folder holds two different
   headers of the same generation, the one the secret opens is used and a warning names the other.
   On Android a header that asks for more than 256 MiB of memory or more than 8 passes is refused
   for a passphrase with a clear message (the recovery key does not run Argon2 and still works,
   and the forced new passphrase after the first sync has the standard cost); the desktop accepts
   everything the format allows.
4. **Confirm.** The screen shows the fingerprint and the header generation. A lower generation than
   on your other device means the location offers an older passphrase setting (a provider can serve
   any genuine old header; nothing in the logs records the generation, so a new device cannot
   detect it, see the threat model T7). Only now is the vault created: the header bytes that
   verified, an empty `notes/`, the sync location (the WebDAV credentials sealed with the vault's
   local key) and the verified generation.
5. **First sync.** A join screen shows progress. It ends when a cycle succeeds, and the vault then
   opens like any other. A join with the **recovery key** asks for a new passphrase only after
   that first sync (the device has published nothing before it has synced). If the first sync
   fails, *Try again* repeats it and *Cancel the join* removes the new vault directory, but only
   while it holds no notes (none in `notes/`, none in the local trash); otherwise the directory is
   left in place and its path is shown.

## Using a new passphrase on another device

After the passphrase or recovery key is changed on one device, the others see the warning "The
passphrase or recovery key was changed elsewhere" (a header file of this vault that authenticates
under this vault's keys). The warning has a **Use the new passphrase** button
(`docs/ADR/0008-device-join-and-header-adoption.md`, part C); a lost race (two devices changed it
at once) has its own warning, "The passphrase was changed on two devices at the same time", with the
same button. The dialog lists `h/` of the sync location, keeps the headers of this vault that are not
older than the one held here, and asks for the new passphrase or recovery key.

- **Checks, in order.** Same vault id; generation not lower than the one held here; the typed secret
  opens it (KDF parameters range-checked, wrap, header MAC); on an unlocked device the unwrapped DEK
  equals the one in memory (constant-time comparison of derived keys); a generation lower than the
  highest MAC-verified one recorded on this device (`verified-header-gen`) is a rollback and is
  refused, whatever the file name says. Nothing is written before all of them passed.
- **Weaker KDF.** A header with lower memory, passes or lanes than the one held here, or than a new
  vault uses (64 MiB, t 3, p 1), is shown with the numbers and needs an explicit confirmation. On
  Android a passphrase header above 256 MiB or t 8 is refused with a message (the recovery key still
  works).
- **Diverging headers.** Of two different headers of the generation, the one the secret opens is
  adopted and a warning (also kept as a sync notice) names the other. Nothing is chosen silently.
- **Locked devices** (never unlocked since the change): the unlock screen has "Passphrase changed on
  another device?". The candidate is opened with the typed secret and its keys must open existing
  sealed local data (a note, or a `K_local` state file) before the header is replaced; with nothing
  to open the header is refused. A locked folder-synced device reads its saved folder; a locked
  WebDAV device asks for the address and account again.
- **Stored.** The header file is replaced atomically (a crash leaves the old or the new one, and both
  open the vault with their own passphrase), the verified generation is recorded, and the notices
  about the newer header are cleared. The data key is unchanged, so no other file is re-sealed and
  the session keeps its keys.

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
   withheld or corrupted, a newer vault header exists, with a button to use it) are shown prominently with what they mean and
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
- **Permission.** The app declares only `INTERNET`. Android requires it for the local channel
  between the app and its webview (loopback only), and it is also what WebDAV sync uses to reach
  your server. With sync off (the default) the app makes no network connections.
- **Locking.** The vault is **saved and locked as soon as the app leaves the screen** (home, app
  switcher, screen off, a call). Coming back asks for the passphrase. If the save fails, nothing is
  locked or discarded and the app tells you when you return. The inactivity timer still applies
  while the app is open.
- **Screenshots.** Screenshots, screen recording and the recent-apps preview show nothing of the
  app (`FLAG_SECURE`).
- **Small screens.** The note list and the open note take turns (*‹ Notes* goes back), the top bar
  wraps, *Details* shows folder, tags and delete, and *Preview* swaps the editor for the preview.
- **Sync.** WebDAV sync is available (see [WebDAV setup](WEBDAV.md)). Syncing through a folder kept
  by an external app is not (it needs the Storage Access Framework; see ADR 0006).
- **Not available yet:** export and import (they need folder access).
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
- **Sync settings** (the sync folder path and the backend) are stored unencrypted in the vault's
  `.nuvoletta/device-state/sync.json` on this device and are never uploaded; they hold no keys or
  note content. Someone who can write that file can repoint the device at another folder (it then
  looks like an empty or rolled-back remote); see the threat model, T24.
- **Sync warnings** (integrity warnings you have not acknowledged) are sealed with a key derived
  from your vault key in `device-state/sync-notices` and never uploaded. They name devices by a
  short id and sequence numbers, which are visible in the sync folder's file names anyway. A file
  that has been replaced or damaged is reported; a deleted one cannot be told from a fresh start.
- **Export never goes to the sync folder.** Export refuses a destination inside or containing the
  configured sync folder (also while sync is off), because it is uploaded by whatever syncs it.
- **One window per vault.** A vault is open in one Nuvoletta window or process at a time; a second
  one is told it is already open (an advisory lock on `.nuvoletta/lock`).
- **Sync and the webview.** Sync adds no network access to the page: the sync worker thread (a
  local folder, or the HTTPS client for WebDAV) runs in the app process, never in the webview, and
  the Content-Security-Policy is unchanged. The conflict view
  shows note text, so it has the same plaintext-in-the-webview limit as the editor.
- **Not available yet:** removing deleted notes' last versions from the sync folder, editing a merge directly inside the conflict view (use *Use version N and edit*), a keyboard
  shortcut for *Sync now* (use the *Sync now* button) and providers other than a synced folder and WebDAV.
