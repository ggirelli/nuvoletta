# Sync setup

Sync is optional. nuvoletta only ever puts **encrypted** files in the sync location; the notes, titles,
paths and tags are unreadable there.

## Option 1: a synced folder (desktop)

1. Create a folder that your sync tool already carries between your devices (Syncthing, a Nextcloud or
   Dropbox client folder, iCloud Drive, a network drive). It must be **separate from the vault
   folder** and must exist.
2. In nuvoletta open *Sync*, pick the folder and press *Use this folder*.
3. Repeat on each device, using the same synced folder. The app reads and writes files there; your
   tool moves them. Wait for your tool to finish before expecting changes.

Details and behaviour (conflicts, warnings, clean-up): the [app guide](../APP.md) and the
[sync protocol](../SYNC_PROTOCOL.md).

## Option 2: WebDAV (desktop and Android)

Use a Nextcloud, ownCloud, NAS or other WebDAV server over HTTPS. The account you give nuvoletta must
be able to **read and create files only**, not delete or overwrite. This is checked when you save and
at the start of every session; an account with more rights is refused, so that a stolen password
cannot destroy your synced copy. Step by step: [WebDAV setup](../WEBDAV.md).

## Adding another device

A new device needs the vault **header** and your passphrase (or recovery key). See
[Lost or stolen device](device-loss.md#joining-a-new-device) for the steps.

## What sync does not do

- It is **not a backup**. A deletion or a damaged vault syncs too (deleted notes can be restored from
  *Recently deleted* for a while). Keep [backups](backup-and-restore.md).
- It cannot **remove** a device you have lost (see [device loss](device-loss.md)).
- The provider can see sizes, counts, timing and how many devices you have; see
  [Security and privacy](security-and-privacy.md).
