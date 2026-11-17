# Recovery key

The recovery key is a long random code shown once when you create the vault. It opens the vault
if you forget the passphrase. It is the **only** other way in. If you lose both the passphrase and
the recovery key, **the vault cannot be opened by anyone**, including the author of the app.

## Keeping it

- Write it on paper and store it somewhere safe and separate from your devices and vault backups (a
  drawer, a safe), or keep it in a password manager that you trust.
- Do not photograph it into a cloud photo library, email it, or keep it in a note inside the vault
  (that would be locked behind the thing it is meant to recover).
- Anyone who has the key **and** a copy of the vault header can open the vault. Treat it like the
  passphrase.

## Using it

1. Choose the vault, and unlock with the **recovery key** instead of the passphrase.
2. The app then requires a **new passphrase** before anything else. Your notes are not changed.
3. Replace the recovery key afterwards (*Settings*; it asks for the new passphrase and shows a new
   key once). The old key stops working.

## After a passphrase change on another device

Changing the passphrase or the recovery key writes a **new vault header generation** to the sync
location. Your other devices continue to work and sync; their notes are untouched (the data key is
the same). They show the notice "The passphrase or recovery key was changed elsewhere". Until a device
adopts the new header it still unlocks with the **old** passphrase, which is why a lost device's
old passphrase remains useful to a thief of that device (see [device loss](device-loss.md)).

To move another device to the new passphrase, use the app:

1. On the device that still has the old passphrase, open *Sync* and, under the warning "The passphrase
   or recovery key was changed elsewhere", press **Use the new passphrase**.
2. Type the **new** passphrase (or the new recovery key). The app fetches the new header from the sync
   location, opens it with what you typed, checks that it belongs to this vault and that its keys are
   the ones this device already holds, and only then replaces the header here. Your notes are not
   touched.
3. If the new header makes guessing the passphrase cheaper than before (lower memory or passes), the
   app says so and asks you to confirm.

If the device was **locked and never unlocked since the change**, you only know the new passphrase and
the old header cannot open it: on the unlock screen press **Passphrase changed on another device?**,
enter the new passphrase, and the app checks the new header against the notes and sync data on this
device before it replaces the header. A device that holds no notes and no sync data cannot do that
check and refuses; join the vault again instead. A device that syncs through WebDAV and is locked asks
for the address and account again (the saved ones are readable only while unlocked).

The app refuses a header older than the newest one this device has seen (an old genuine header would
bring back an old passphrase), a header of another vault, and one whose keys do not match this
device's notes. If the passphrase was changed on two devices at the same moment, the app uses the
header your typed passphrase opens and warns you about the other.

!!! note "If the app cannot do it"
    The manual way still works: close the vault (*Close vault*, not just *Lock*), copy the newest
    `h/<generation>.nvhdr` of the sync location over `.nuvoletta/vault-header` in that device's vault
    folder, open the vault again and unlock with the **new** passphrase.

This is tested end to end, including that the old recovery key stops working.
