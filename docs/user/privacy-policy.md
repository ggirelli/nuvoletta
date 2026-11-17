# Privacy policy

nuvoletta is free, local-first software. This page states what it does with data.

## Summary

- **No Nuvoletta servers.** The project operates no server, account system, sync service or
  analytics service, and the app never contacts one.
- **No telemetry.** No usage statistics, crash reports, identifiers or advertising are collected or
  sent. There are no third-party SDKs.
- **Your data stays on your devices**, in the vault folder. Notes are stored encrypted. The only data
  that ever leaves your device is **encrypted** data, sent to a sync location **you** choose and
  configure.
- **No update checks.** The app does not contact the internet to look for updates.

## The only network traffic

We checked the source: the only code that opens a network connection is the WebDAV backend
(`nuvoletta-webdav`), and it connects only to the **address you enter** in the WebDAV setup, and only
while WebDAV sync is on and the vault is unlocked. It uses HTTPS only. A password is sent only to
that address (redirects to other hosts are refused). Everything else is local:

- The note preview never loads remote content, and the page runs under a strict content policy.
- Clicking a web link in a note opens it in your **system browser** (that is your browser's traffic,
  started by your click, not nuvoletta's).
- The synced-folder option does not use the network in the app at all; your sync tool does.
- On Android the app requests the `INTERNET` permission for WebDAV, and because Android requires it for the app's own loopback channel to its web view (127.0.0.1 only, nothing leaves the device).

## What a WebDAV provider (or your sync tool's provider) sees

- Your IP address and the account name you use, as for any WebDAV client.
- Encrypted files with opaque names: how many, how large (padded), and when they change.
- How many devices you use and when each syncs.
- The vault header (encrypted key material and password-hashing parameters).
- It does **not** see note text, titles, paths, tags or your passphrase or keys. See
  [Security and privacy](security-and-privacy.md) for the limits.

Your provider's own policy governs what it does with that data and its logs.

## What is stored on your device

- The vault folder: encrypted notes, the header, sync state, a local trash of recently deleted
  notes (encrypted), all inside the vault folder.
- App settings (auto-lock time, last vault folder) and per-device sync settings (sync folder path,
  warnings you have not acknowledged) in plain text, containing no keys and no note content.
- WebDAV credentials, encrypted with a key derived from your passphrase, readable only while the
  vault is unlocked, never synced.
- Plaintext **only** if you export.

## Your control

You can delete the vault folder, the sync location and any exports at any time; nothing remains
elsewhere. Because there is no central service there is no account to close and nothing for the
project to delete on your behalf.

## Changes

If this changes (for instance if an optional online feature were ever added), it will be stated
here and in the release notes before it ships, and it will be opt-in.
