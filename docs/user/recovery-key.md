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

To move another device to the new passphrase:

1. Close the vault in nuvoletta (*Close vault*, not just *Lock*: the header is read when the vault is
   opened).
2. Copy the newest `h/<generation>.nvhdr` from the sync location over `.nuvoletta/vault-header` in that
   device's vault folder.
3. Open the vault again and unlock with the **new** passphrase.

This is tested end to end, including that the old recovery key stops working.
