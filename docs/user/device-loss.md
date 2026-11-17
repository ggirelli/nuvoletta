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
with the old one until you press **Use the new passphrase** on them (see
[Recovery key](recovery-key.md#after-a-passphrase-change-on-another-device)).

## Joining a new device

You need the passphrase (or recovery key) and access to the sync location. You do not need the old
device.

1. On the first screen choose **Join an existing vault**.
2. Choose where the vault is synced: the folder your sync tool keeps (desktop), or the WebDAV
   address and account (desktop and Android). Wait until your sync tool has finished bringing the
   folder's `h` folder to this device. If the location holds more than one vault, pick yours by its
   **fingerprint**, which you can read in *Settings* on a device that has the vault.
3. Type the vault's **current** passphrase, or its recovery key. The app opens the vault header
   with it before anything is created.
4. Compare the **fingerprint** and **header generation** shown with *Settings, This vault* on
   another device, then choose where the vault goes on this device and confirm. A *lower*
   generation than on your other device means the sync location offers an older passphrase setting
   (see below): do not continue if you changed the passphrase because the old one may have leaked.
5. The app fetches your notes (the first sync). If you joined with the recovery key, it then asks
   you for a new passphrase. If the first sync fails you can try again or cancel; cancelling removes
   the new vault from this device unless it already holds notes, in which case it is left in place
   and the app tells you where.

!!! warning "A provider can serve an old header"
    A device that has never seen the vault cannot tell a current header from an old genuine one: a
    storage provider (or anyone who controls the location) could offer the header from before you
    changed your passphrase, and then the **old** passphrase opens the vault on the new device. The
    generation number you compare in step 4 is how you notice. Changing the passphrase does not
    change the data key, so this is also why a leaked passphrase plus a copy of the vault stays
    dangerous (see the steps above).

!!! note "If the app cannot do it"
    Joining by hand still works: create an empty folder for the vault with a folder named
    `.nuvoletta` inside, copy the sync location's `h/<generation>.nvhdr` with the highest number into
    it under the name `vault-header`, choose *Open an existing vault* on that folder, unlock, then
    choose the sync location in *Sync*. The header contains no note content, only the parameters and
    wrapped keys that the passphrase opens.

If you lose every device **and** the sync location, restore from a [backup](backup-and-restore.md).
If you lose the sync location but still have a device, set up a fresh location: the device re-sends
everything.
