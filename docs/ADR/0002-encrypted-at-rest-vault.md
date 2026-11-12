# ADR 0002: Encrypted-at-rest vault with Markdown export/import for portability

Status: Proposed (Phase 0, not reviewed)
Date: 2026-11-12

## Context

Two product goals pull against each other: notes should be private even when a device or sync location is exposed, and users should never be locked in. Obsidian-style plaintext folders give portability and tool interoperability but leave notes readable on disk and on the sync provider. PLAN.md principles 1, 3 and 4 require encryption before anything is written to a sync destination, database-free storage, and provider independence.

## Decision

1. The vault is an ordinary directory of **encrypted files** (versioned envelopes, one per note revision), encrypted at rest locally and remotely with the same format. It is not readable by other tools; the Markdown text lives only inside the encrypted payload.
2. Portability is provided by an explicit, user-initiated **plaintext Markdown export** (a folder of `.md` files) and **import** of a folder of `.md` files. Export is a warned, one-way copy; the app never keeps an auto-updating plaintext mirror.
3. No database in the MVP. An in-memory index built at unlock plus an optional rebuildable cache encrypted under a derived local key.
4. Cryptography follows `CRYPTO_FORMAT.md`: Argon2id passphrase KEK, random DEK, recovery-key second wrap, HKDF-separated subkeys, XChaCha20-Poly1305 envelopes with Padmé padding.
5. Metadata (title, path, tags, timestamps) is inside each note's envelope; remote names are opaque random ids.

## Consequences

- Positive: lost or stolen locked devices and curious providers see only ciphertext and padded sizes; one format for local storage and sync (objects are uploaded as-is); no plaintext temp files by design.
- Negative: external editors, grep and backup tools cannot work on the vault directly; users who want that use export. Search and linking require unlocking and an in-memory index, with a performance target to validate (about 10k notes on mid-range Android).
- Negative: losing both the passphrase and the recovery key loses the vault by design.
- Risk: export copies are plaintext and unprotected; documented and warned.
- Risk: users may rely on export as backup; documentation must also describe backing up the encrypted vault plus the recovery key.
- A bug in the envelope format has high cost, so the format is versioned, test-vectored and reviewed before implementation (Phase 0 exit criteria).

## Alternatives considered

- Plaintext Markdown folder with file-level encryption by a third-party tool (for example Cryptomator): good interoperability, but weaker control of metadata, conflict handling and key management; rejected for the core product.
- SQLite/SQLCipher: convenient queries, but adds a database, whole-file sync conflicts, and a larger dependency surface; deferred pending evidence.
- Relying on full-disk encryption only: does not protect against sync providers; rejected.
