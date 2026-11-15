# Security and privacy

!!! warning "Not audited"
    nuvoletta has **not** been independently audited and must not be described as secure or audited.
    It is pre-alpha. The design is documented and tested, including hostile-input and fault tests,
    but that is not a substitute for an independent review. Do not rely on it for highly sensitive
    material until that review has happened. If you find a problem, see
    [SECURITY.md](https://github.com/ggirelli/nuvoletta/blob/main/SECURITY.md).

The detailed analysis is in the [threat model](../THREAT_MODEL.md), the [crypto format](../CRYPTO_FORMAT.md)
and the [sync protocol](../SYNC_PROTOCOL.md). This page is the short version.

## What is encrypted

Everything you write: note **bodies, titles, paths (folders) and tags**, and the notes' timestamps.
Each note is an authenticated encrypted file under a key derived from your passphrase (Argon2id,
XChaCha20-Poly1305). The sync log, which says which note changed and when, is encrypted too. Files are
also checked for tampering: a modified, truncated or swapped file is detected and refused.

## What is not hidden: metadata

Whoever holds the encrypted files (the sync service, your file host, someone with a copy) can see:

- **How many** files there are, so roughly how many notes you have and how often you edit.
- **Sizes**, blurred by *Padmé* padding (sizes are rounded up into buckets, with a minimum of 256
  bytes), but not hidden: a very large note is visibly large.
- **Timing**: when files appear or change, so when you write and sync.
- **How many devices** you use (each device has its own folder of log files) and when each is active.
- **Directory layout and names**: file names are random and opaque, but the structure (an `h`, `d`,
  `o` layout, one folder per device) shows that the data is a nuvoletta vault.
- The vault header sits in the sync location so that new devices can join. It lets someone try to
  guess your passphrase offline. The cost of each guess is deliberately high, and a long passphrase
  makes it impractical, but a weak one will fall.

## What nuvoletta cannot promise

- **Weak passphrases.** Offline guessing against a stolen copy or header is the main risk. Use a long
  passphrase.
- **Rollback and withholding by the storage provider.** A provider can consistently serve an older
  version of everything, hide the newest changes of a device you have not seen yet, hide a device you
  have never seen, or show two devices different histories. Devices cross-check each other and warn
  when they can tell, but they cannot detect a consistent rollback of everything they have not seen
  past. (Design limits 10.3 in the sync protocol.)
- **A hostile enrolled device.** All devices share one data key. A device that holds it can read
  everything and can forge changes in another device's name. There is no per-device revocation; see
  [device loss](device-loss.md).
- **An unlocked device.** While unlocked, plaintext exists in memory: the app's, and the web view
  that shows the editor, which cannot be scrubbed. Malware, a person with access to the unlocked
  machine, screenshots on desktop, and crash dumps can capture it. Locking drops the keys and the
  editor content, but memory is not guaranteed to be wiped. Swap files and hibernation on desktops are
  not controlled by the app.
- **The clipboard.** Copied text lands in the system clipboard, and clipboard managers or other apps
  may keep it. The app does not clear it automatically.
- **Your keyboard** (Android) may learn what you type.
- **No automatic updates.** nuvoletta never updates itself and never checks for updates. You get new
  versions (and security fixes) by installing them yourself from signed releases or your store, so
  keep an eye on the releases page.
- **Exports** are plaintext files on your disk, outside any protection.
- **Lost passphrase and recovery key** cannot be recovered.
- **Sync is not backup**; keep [backups](backup-and-restore.md).

## What the app does to limit risk

Notes render without running scripts: raw HTML is shown as text, remote images are not loaded, only a
short list of link types can be opened and a strict content policy blocks other network access from
the page. Vault files are written atomically, so a crash does not corrupt them, and conflicting edits
are kept as versions rather than overwritten. For a WebDAV server, the account must be unable to delete
or overwrite files, so a stolen password cannot destroy the server copy.
