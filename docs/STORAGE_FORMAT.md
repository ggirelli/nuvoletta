# Nuvoletta Local Storage Format (v1)

Status: Phase 1 implementation (`nuvoletta-storage`). Byte formats are defined in
`CRYPTO_FORMAT.md`; this document only describes how the vault directory is laid out and
operated on. Sync state is out of scope here (`SYNC_PROTOCOL.md`, Phase 4).

## 1. Layout

```text
<vault>/
  .nuvoletta/
    vault-header            236-byte vault header (CRYPTO_FORMAT section 2)
    device-state/           reserved (Phase 4), created when first needed
    sync-state/             reserved (Phase 4): device-id, journal, peers, ... (envelope type 4)
    cache/                  reserved: rebuildable caches (envelope type 4)
  notes/
    <note_id 32 lower-case hex>.nvnote    latest revision of the note: one envelope (type 1)
```

- A note file is exactly the envelope bytes that sync will later upload as
  `o/<object_id[0..2]>/<object_id>.nvobj` (`SYNC_PROTOCOL.md` section 4). The local name is the
  `note_id`, the remote name is the revision `object_id`; both are random and opaque.
- Titles, paths, tags and bodies exist only inside the envelope. Nothing but ciphertext, the
  header and random ids is ever written; temporary files hold ciphertext only.
- Only the latest revision of each note is kept locally (local history is post-MVP).

## 2. Operations

| Operation | Behaviour |
|-----------|-----------|
| create vault | creates `.nuvoletta/` and `notes/`, writes the header atomically, returns the recovery key once; fails if a header exists |
| open | validates the header (size, magic, version, KDF range), removes leftover `.tmp-*` files, derives no key |
| unlock | passphrase or recovery key; scans `notes/`, authenticates every note and builds the in-memory index (metadata only) |
| lock | drops and zeroizes keys and index; returns a locked handle |
| note create / update / delete | create fails on an existing id; update seals a new revision (fresh `object_id` and nonce) and replaces the file; delete unlinks it |
| change passphrase | new header via the crypto crate, verified to unlock, then atomically replaces the old one; notes untouched |
| rotate recovery key | needs the current passphrase; same mechanics |

Atomic write: create `.tmp-<random>` in the destination directory (`create_new`), write, `fsync`
the file, `rename` over the destination, `fsync` the directory (Unix; directories cannot be
opened for sync on Windows). A crash leaves either the old file or the new one, plus at worst a
`.tmp-*` file, which is removed at the next open.

## 3. Hostile and odd filesystems

- Only names matching `^[0-9a-f]{32}\.nvnote$` are notes (lower-case only, so case-insensitive
  filesystems cannot make two names equal; no Unicode in names, so normalisation is irrelevant;
  note paths are virtual and never used as file names).
- Symlinks are never followed: a symlinked header, `.nuvoletta/`, `notes/` or note file is
  refused or reported. Names never come from untrusted input, so path traversal is impossible.
- Identity comes from the authenticated envelope, not the name: a file whose `scope_id` or
  payload `note_id` differs from its name is corrupt. A copy under another name (for example
  `<id> (1).nvnote`) is stray and ignored, so a duplicate id can never shadow the real note.
- Stray, symlinked and corrupt files are reported by `Unlocked::issues()`; they are never read
  as truth and never deleted. A corrupt note does not prevent unlocking.

## Implementation notes

Ambiguities resolved while implementing Phase 1 (none changes a byte format):

1. **Local file name.** PLAN section 4 and `SYNC_PROTOCOL.md` do not fix the local note file
   name. `note_id` is used (one file per note, replaced in place), flat directory, no sharding.
2. **Delete has no tombstone yet.** Deletes are log entries in the sync protocol; Phase 1 just
   unlinks the file. Phase 4 must journal the delete at the storage API boundary.
3. **Stricter path rule.** `.` segments are rejected in `NotePath` in addition to `..`, empty
   segments, a leading `/`, non-NFC input (normalised on parse) and control characters.
   `nuvoletta-crypto` itself only checks the spec'd rules, so other writers could store `.`; such
   a note is reported corrupt by storage.
4. **Temporary files vs "never delete strays".** `.tmp-*` files are the writer's own artefacts
   (PLAN section 4 requires cleanup) and are removed at open; other strays are never touched.
   An in-flight `.tmp-*` of another process is skipped during the scan.
5. **Header changes re-unlock.** The crypto `Vault` does not refresh its header after
   `change_passphrase`/`rotate_recovery_key`, so storage verifies the new header by unlocking it
   (one extra Argon2 run), writes it, and adopts the resulting vault. Before writing, the header
   on disk must equal the one this session unlocked with (`Error::HeaderChanged`), a cheap guard
   against lost updates between two processes. Rollback detection (`max-header-gen`) is Phase 4.
6. **Recovery-key rotation needs the passphrase** (crypto implementation note 1).
7. **Recovery confirmation.** `create` returns the key; making the user re-enter part of it
   before the vault is considered usable is a UI concern.
8. **Read cap.** Note files above 32 MiB are refused (16 MiB body limit plus padding overhead).
9. **Timestamps** are supplied by the caller; storage does not touch `created_at`/`modified_at`.
