# Backup and restore

A vault is an ordinary folder. Backups are copies of it; nuvoletta needs no special tool.

## What to back up

| Back up | Why |
|---|---|
| The **vault folder** (the whole directory you chose, including the hidden `.nuvoletta` folder) | It holds every note (encrypted) and the vault header, which is needed to unlock. |
| The **recovery key** (on paper or in a password manager), kept **apart** from the vault backup | With the header it opens the vault if you forget the passphrase, so anyone holding both can read your notes. Keep it apart from the backup. |
| Your **passphrase** (in your head or a password manager) | Nothing else can open the vault. |

The backup is as private as the vault: everything in it is ciphertext, so it is safe on an external
drive or cloud storage, provided your passphrase is strong (an attacker with a copy can try to guess
it offline; see [Security and privacy](security-and-privacy.md)).

Do the copy while the vault is **locked** (or the app is closed), so that no file is half-written.
Writes are atomic, so a copy taken while unlocked is normally fine too.

## What not to sync or back up carelessly

- **Plaintext exports.** *Export* writes unencrypted Markdown. Keep it out of any synced or backed-up
  location, and delete it when you are done.
- **The vault folder through a file-sync tool.** Do not put the vault folder itself inside Dropbox,
  Syncthing, a Nextcloud client folder and so on. Sync uses a *separate* sync folder that holds only
  encrypted protocol files. Syncing the vault folder as well would make two sync mechanisms fight.
- **The sync folder is not a backup of your vault folder**, though it holds every note version. Back
  up the vault folder as above.
- **Do not run a cloud backup of the app's data on two devices and expect them to merge.** Restoring
  an old copy is supported (below) but is a restore, not a sync.

## Restore from a backup

1. Copy the backup folder to where you want the vault (a new, empty location is best).
2. In nuvoletta, choose *Open an existing vault* and pick it, then unlock with the passphrase (or the
   recovery key if you forgot it).
3. If the vault used sync, choose the sync folder again (*Sync*, *Use this folder*). Settings that
   belong to a device, such as the sync folder path, are stored in the backup as they were.

### Restoring while the original keeps running (clones)

A restored copy has the same internal device ID as the vault it was copied from. If both then
write to the same sync folder, nuvoletta detects that "this device ID was used twice" and fixes it
without losing anything: the restored copy continues under a new device ID and re-sends every note
version that the folder does not have yet. Notes edited differently on both sides appear as ordinary
conflicts with both versions kept (see the [app guide](../APP.md)). You may see a warning about the
duplicated device ID; read it and acknowledge it. This is tested end to end.

If you restored an **old** copy and the sync folder holds newer versions, the first sync brings them
in. If the folder is empty or older than what this device has seen, you get a rollback warning
and nothing is overwritten.

## Restore from a plaintext export

If you only have an export (a folder of `.md` files), create a **new** vault and use *Import*. This
gives a fresh vault with new keys; notes keep their paths, tags and text, and importing never
overwrites an existing note. It cannot carry over the sync history or earlier versions. Delete the
export afterwards. Format details: [export format](../EXPORT_FORMAT.md).

## If something is damaged

- **A note file is corrupt:** the vault still opens, the file is listed as a problem and is never
  deleted. Restore the file from a backup, or fetch it again from the sync folder.
- **Local sync bookkeeping is lost or damaged** (the `.nuvoletta/sync-state` folder): it is rebuilt
  from the sync folder on the next sync, so you lose nothing. An edit made while that state was gone
  shows up as a conflict with both versions rather than silently replacing one. If the
  list of unsent changes (`journal`) is damaged, the app keeps the damaged file as
  `.nuvoletta/sync-state/journal.corrupt-<time>`, starts a fresh one, shows a notice in the sync view
  and continues: unsent edits are found again from your notes and sent, so nothing is lost. Delete
  the set-aside file once you have checked that your recent edits arrived.
- **The local trash is lost** (`.nuvoletta/trash`): notes synced before deletion can still be
  restored from *Recently deleted* through the sync history. Without sync, deleted notes that were
  only in the trash are gone.
- **Device settings are damaged** (`.nuvoletta/device-state`): the vault opens; sync is off until you
  choose the folder or WebDAV again.
