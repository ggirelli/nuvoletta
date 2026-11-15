# Lost or stolen device

## What a lost device still has

A device that was **locked** holds only encrypted files. Whoever has it can try to guess your
passphrase offline. A strong passphrase (a long phrase of random words) makes that impractical; a
weak one does not. Full-disk encryption on the device adds a second layer.

A device that was **unlocked** (or whose memory or screen could be read) may have exposed whatever
was open at the time.

!!! danger "Devices cannot be revoked"
    Every device of a vault shares the same data key. There is **no way to remove one device** from
    a vault in this version. A device that keeps your passphrase (or its vault folder and a way to
    guess the passphrase) can still decrypt every note it has, and everything in the sync location
    it can still reach, **including notes written later**, as long as it can read that location.
    Changing the passphrase does not change the data key, so it does **not** re-encrypt old notes.
    A future version may add key rotation; this one does not.

## What to do when a device is lost

Do these in order. They limit the damage; they do not undo it.

1. **Cut off its access to the sync location.**
   - *WebDAV:* change the password (or revoke the app password) of the account the vault uses,
     create a new one and enter it on your remaining devices. The lost device can then no longer read
     or add files.
   - *Synced folder:* remove the lost device from your sync tool (Syncthing device list, Dropbox or
     Nextcloud authorised devices, iCloud sign-out). Revoke its access.
2. **Change the vault passphrase** on a remaining device (*Settings*). This protects the future
   use of your passphrase, but it does **not** change the data key: someone who already holds the old
   passphrase and a copy of the vault can still open that copy.
3. **Replace the recovery key** (*Settings*), because the old one may be known if it was stored on
   the lost device.
4. If the notes are very sensitive and the passphrase was weak or the device was unlocked: treat what
   it held as exposed. The only complete fix is to **create a new vault with a new passphrase, import
   your notes into it, and stop using the old vault and sync location**, and delete the old
   sync location's files.

Other devices that were already enrolled see a notice that a newer header exists and keep working
with the old one until you move them over (see
[Recovery key](recovery-key.md#after-a-passphrase-change-on-another-device)).

## Joining a new device

You need the passphrase (or recovery key) and access to the sync location. You do not need the old
device.

1. On the new device, set up the same sync location (the folder with your sync tool, or the WebDAV
   address and account).
2. Get the **vault header**: it is the file `h/<generation>.nvhdr` in the sync location with the
   highest number (for WebDAV, download it with the server's web interface; for a folder it is just
   there). Create an empty folder for the new vault, inside it a folder named `.nuvoletta`, and copy
   the header into it under the name **`vault-header`**.
3. In nuvoletta choose *Open an existing vault* on that folder and unlock with the passphrase.
4. Choose the sync location (*Sync*) and sync. All notes arrive. This is tested end to end for both a
   synced folder and WebDAV.

!!! note "Header copy"
    There is no one-click "join" button yet; the header copy is a manual step. The header contains
    no note content, only the parameters and wrapped keys that the passphrase opens.

If you lose every device **and** the sync location, restore from a [backup](backup-and-restore.md).
If you lose the sync location but still have a device, set up a fresh location: the device re-sends
everything.
