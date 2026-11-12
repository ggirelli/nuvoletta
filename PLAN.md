# Nuvoletta — Project Plan

> **Working tagline:** Private thoughts. Safely kept.  
> **Status:** Revised plan (rev 2)  
> **Primary implementation:** Rust + Dioxus  
> **License:** MIT (open source)  
> **Product direction:** Local-first, encrypted at rest, provider-independent, portable via Markdown export/import

Nuvoletta is a personal, privacy-focused note-taking and knowledge-management application. It takes inspiration from the Markdown-centric approach of Obsidian and the security philosophy of Standard Notes, while maintaining its own identity, implementation, interface, and product decisions.

The goal is not to reproduce either application. The goal is to build a pleasant Markdown writing environment with strong encryption, reliable cross-device synchronization, and an architecture that can grow through extensions.

## 1. Product principles

1. **Privacy by design.** Notes are encrypted on the device before they are written to a sync destination. A storage provider must never need access to note plaintext or vault encryption keys.
2. **Local-first.** Reading, writing, searching, and organizing notes must work offline. Sync is asynchronous and must never be required for normal use.
3. **Simple, database-free storage.** The vault is a directory of encrypted files, not a database, and is not directly readable by other tools. Markdown is the canonical note format inside the encrypted records; indexes and caches are disposable and rebuildable.
4. **Portable and provider-independent.** Portability comes from plaintext Markdown export and import. Users choose where synchronized ciphertext is stored. Nuvoletta should not require a Nuvoletta-operated cloud service.
5. **Cross-platform.** Share core logic across desktop and mobile while isolating platform-specific filesystem, credential-store, and lifecycle behavior.
6. **Extensible, but secure.** Design clean extension points early. Defer third-party plugin execution until the core application and security model are stable.
7. **Boring, reviewable cryptography.** Use established libraries and constructions. Do not invent cryptographic primitives or protocols.

## 2. MVP scope

The MVP consists of four capabilities:

### 2.1 Encrypted vault

- Create, open, lock, and unlock a vault.
- Use a user passphrase to derive a key-encryption key with Argon2id.
- Generate a random vault data-encryption key (DEK); wrap it with the derived key.
- Encrypt each note independently using XChaCha20-Poly1305 with random 192-bit nonces.
- Use cryptographically secure operating-system randomness for keys and nonces.
- Authenticate encrypted records and reject modified or truncated ciphertext.
- Keep plaintext and key material in memory only while needed; minimize copies and zeroize secret buffers where practical.
- Support changing the passphrase by re-wrapping the DEK rather than re-encrypting every note.
- Recovery key: a high-entropy random key shown to the user once (printable) and used as a second wrap of the DEK. A new device is enrolled by entering the passphrase.
- Keep local encrypted version history (snapshots) of notes as protection against sync bugs, if cheap; otherwise early post-MVP.
- Lock manually and automatically after a configurable period of inactivity.

### 2.2 Markdown writing

- Create, rename, move, edit, and delete notes.
- Markdown editor with a live preview.
- Standard Markdown as the baseline, using a documented and deliberately small extension set.
- Autosave with atomic file replacement and recovery from interrupted writes.
- Wikilinks such as `[[Another Note]]`.
- Backlinks and unresolved-link indication.
- Plaintext Markdown export of the vault, and import of a folder of `.md` files. This is the portability mechanism.
- Basic tags and note metadata, if they can be implemented without compromising the writing experience.
- Safe rendering: raw HTML disabled or strictly sanitized; no scripts or active content from notes.
- Keyboard-friendly desktop editing and a usable mobile editing experience.

### 2.3 Cross-device synchronization

- Sync encrypted vault objects through a user-selected storage provider.
- Work offline and synchronize when connectivity returns.
- Detect concurrent edits and preserve both versions; never silently discard user content.
- Handle retries, interrupted uploads/downloads, provider throttling, and partial failures.
- Provide clear sync status and actionable error messages.
- Keep provider credentials in the platform credential store where available, not in the vault or plaintext config.

**Provider targets:** the first real backend is an **externally synced folder**: a local folder that the user syncs with Syncthing, Dropbox, iCloud, a Nextcloud client, or similar. Nuvoletta only reads and writes files there. WebDAV is second. Google Drive and Dropbox (API) backends are post-MVP. Keep the provider interface generic so other backends can be added later. A plain local-folder backend remains useful for tests.

The sync protocol must tolerate a dumb file-sync layer: no atomicity across files, delayed or partial propagation, files appearing out of order, and sync-tool conflict copies (which must be recognized and handled, not treated as corruption). More generally, do not assume that providers offer identical atomicity, revision history, conditional writes, or reliable change feeds. The sync engine must work correctly even when it has to list and compare remote objects.

### 2.4 Cross-platform application

- Dioxus UI with shared Rust domain logic.
- Initial target: desktop (Linux, macOS, Windows) and Android.
- Keep iOS feasible, but verify Dioxus and required platform APIs before committing to an iOS release target.
- Platform adapters for file access, secure credential storage, notifications if needed, and application lifecycle.
- A vault must remain usable without network access.

## 3. Explicitly out of scope for the first MVP

- Third-party plugin marketplace or arbitrary native plugins.
- WASM plugin runtime and fine-grained plugin permissions.
- Real-time collaborative editing.
- A Nuvoletta-hosted synchronization service.
- Complex graph visualization.
- Attachments and rich media (post-MVP).
- Device revocation and DEK rotation.
- Google Drive and Dropbox API backends.
- End-to-end encrypted sharing between different users.
- Importers for every note-taking application.
- Browser-based editing.

Keep extension points in the architecture, but do not let speculative plugin requirements delay the secure note and sync foundations.

## 4. Storage and vault layout

Use an ordinary directory as the vault boundary. The exact on-disk format is a security-sensitive design decision and must be specified before implementation.

A conceptual layout:

```text
MyVault/
├── .nuvoletta/
│   ├── vault-header
│   ├── device-state/
│   ├── sync-state/
│   └── cache/
├── notes/
│   └── <opaque-note-id>.nvnote
└── (attachments/ — post-MVP)
```

Requirements:

- Do not expose note titles or paths to the remote provider by default. Use opaque identifiers for remote object names.
- Store each note's title, path, and tags inside that note's own encrypted envelope. There is no global encrypted index object, so a rename touches exactly one object. The in-memory index is built at unlock.
- Store the canonical note content inside the encrypted record. The `.nvnote` extension is illustrative; define a versioned binary envelope.
- Store local-only caches separately and make them rebuildable.
- Use stable note IDs independent of filenames and titles.
- Avoid depending on filesystem modification times as the source of truth.
- Use atomic writes: write a temporary file, flush as appropriate, then replace the destination.
- Define behavior for case sensitivity, Unicode filenames, path traversal, symlinks, and duplicate IDs.
- Do not store plaintext temporary files unless there is a documented, tightly controlled reason. Clean up temporary artifacts on normal shutdown and recovery paths.

The user-facing vault should not require SQLite. An in-memory index and optional rebuildable on-disk cache are sufficient for the initial scope. Performance target to validate: unlock plus index of ~10k notes in acceptable time on a mid-range Android device. If performance later demonstrates a need for a database, treat that as a separate, evidence-based decision.

## 5. Cryptographic architecture

Cryptographic details must be reviewed before they are treated as final. The following is the proposed starting point, not a claim that the design is already audited.

### 5.1 Key hierarchy

- Generate a random vault DEK using the operating system CSPRNG.
- Derive a key-encryption key from the passphrase with Argon2id and a unique random salt.
- Wrap the DEK with an authenticated-encryption construction.
- Store the salt, KDF parameters, format version, and wrapped DEK in the vault header.
- Keep the passphrase-derived key and DEK out of persistent storage in plaintext.
- Use a separate, documented key-derivation scheme for distinct purposes if the design requires multiple keys. Do not reuse one key directly across unrelated protocols.

### 5.2 Per-object encryption

- Encrypt notes independently with XChaCha20-Poly1305.
- Nonce strategy: XChaCha20-Poly1305 with a fresh random 192-bit nonce for every encryption; state this explicitly in the format specification.
- Pad plaintext to a bucketed length (e.g. Padmé) inside the envelope to reduce note-length leakage; decide the scheme in Phase 0.
- Authenticate relevant immutable envelope fields as associated data, including format version and object identity.
- Include a format version and algorithm identifiers so the format can evolve.
- Apply equivalent protection to sensitive metadata (and to attachments when they are added).
- Treat authentication failure as corruption or tampering; never return partially decrypted plaintext.

### 5.3 Threat model

Write a threat model before implementing the crypto crate. At minimum, consider:

- A lost or stolen device while locked.
- A cloud provider or NAS administrator attempting to read or modify stored data.
- A malicious or compromised sync provider.
- Replay, deletion, rollback, and substitution of remote objects.
- A compromised device while the vault is unlocked.
- Malicious note content and malicious attachments.
- Theft of provider OAuth tokens.
- Lost devices (revocation is out of MVP scope; see 6.3).
- Decrypted plaintext in webview JS memory, which cannot be zeroized.
- Backups, crash reports, logs, swap, temporary files, and diagnostic output.
- Malicious or compromised application updates.

Be precise about limitations. Encryption at rest cannot protect notes from malware or an attacker controlling a device while the vault is unlocked. Metadata such as access times, object sizes, and synchronization patterns may still leak unless specifically mitigated.

### 5.4 Security engineering rules

- Use established Rust cryptography crates; do not implement primitives.
- Pin and review dependency updates, and minimize the cryptographic dependency surface.
- Never log passphrases, keys, plaintext note contents, OAuth tokens, or decrypted metadata.
- Use `zeroize` or equivalent protections where appropriate, while documenting that zeroization cannot guarantee removal of every memory copy.
- Add fuzzing and property-based tests for parsers, envelopes, and sync state transitions.
- Include tampering, truncation, wrong-password, malformed-input, and corruption tests.
- Make security assumptions and format changes explicit in documentation.
- Obtain independent human security review before relying on Nuvoletta for highly sensitive material (Phase 7).
- The editor and any JS dependency fall under the same supply-chain rules (pinned, reviewed, vendored where practical). Note rendering must stay sandboxed: no script execution from note content, even though the app's own scripts run.

## 6. Synchronization design

Separate the sync engine from provider-specific transport. Design rules:

- Each device writes only its own append-only log/manifest objects; there is no shared mutable manifest. This avoids write-write races on providers without conditional writes. Devices merge the other devices' logs.
- Note content objects are written under unique (e.g. content-derived or versioned) names where possible, and become visible to others only when referenced from the writer's own log.

Conceptual layers:

```text
Application / Vault
        |
Encrypted object store
        |
Sync engine
  - local change journal
  - remote inventory
  - version comparison
  - conflict handling
  - retry and recovery
        |
Storage provider trait
  |             |          |
Synced folder  WebDAV   (later: Google Drive, Dropbox)
```

The provider abstraction should expose only the operations the engine genuinely needs, for example:

- List remote objects and relevant opaque metadata.
- Fetch an object.
- Upload or replace an object.
- Delete an object when the protocol permits it.
- Read provider revision/ETag information when available.
- Report capabilities and provider-specific errors.

Do not force providers into a lowest-common-denominator abstraction that hides important differences. Model capabilities explicitly.

### 6.1 Conflict handling

- Never silently apply last-write-wins to note content.
- Detect when the local and remote versions both diverged from a common ancestor. Store a per-object base version locally (e.g. hash of the last-synced ciphertext).
- Preserve both versions, or create a conflict copy, until the user resolves the conflict.
- Make resolution understandable and safe on mobile as well as desktop.
- Test simultaneous edits, offline edits, renames, deletes, restores, and interrupted sync.
- Treat delete-versus-edit as a conflict, not as an automatic deletion.

### 6.2 Sync integrity and rollback

Authenticated encryption detects unauthorized modification of individual objects, but it does not by itself prevent a provider from replaying an older valid ciphertext or deleting data. Build rollback and deletion detection on the per-device logs (e.g. each device authenticates and chains its own log, and remembers the latest position it has seen of every other device's log). State the limits honestly: without a trusted external checkpoint, a provider can still (a) roll back all objects consistently to an earlier state this device has not yet seen past, (b) hide the newest entries of another device's log that this device has not yet observed, or (c) withhold the log of a device this device has never seen at all.

### 6.3 Authentication and devices

- Use official OAuth flows for providers that require them.
- Request the narrowest provider permissions available.
- Store refresh tokens in the OS credential store.
- Never put provider credentials in synchronized vault data.
- MVP key model: one passphrase plus one recovery key (second wrap of the DEK). Enrolling a device means entering the passphrase on it; each device has a random device ID used to name its logs.
- Device revocation is out of MVP scope. With a single DEK, a removed device keeps access to everything it already synced (and to anything later synced that it can still read). Real revocation requires DEK rotation and re-encryption of all objects; this is future work and must be documented to users.

## 7. Proposed Rust workspace

Keep crate boundaries practical; avoid creating a crate for every small module before there is a demonstrated need.

```text
nuvoletta/
├── Cargo.toml
├── crates/
│   ├── nuvoletta-core/       # Domain types, notes, IDs, links, vault rules
│   ├── nuvoletta-crypto/     # Key hierarchy, encrypted object format
│   ├── nuvoletta-storage/    # Filesystem vault and atomic persistence
│   ├── nuvoletta-markdown/   # Markdown parsing, extensions, safe rendering
│   ├── nuvoletta-sync/       # Sync state machine and conflict handling
│   ├── nuvoletta-provider/   # Provider traits and shared sync types
│   └── nuvoletta-webdav/     # WebDAV backend
│   # Later (post-MVP): nuvoletta-google-drive, nuvoletta-dropbox
│   # The synced-folder and test local-folder backends live in nuvoletta-provider.
├── apps/
│   └── nuvoletta-app/        # Dioxus application
├── docs/
│   ├── THREAT_MODEL.md
│   ├── CRYPTO_FORMAT.md
│   ├── SYNC_PROTOCOL.md
│   └── ADR/
└── tests/
```

Adjust this layout if implementation reveals that some crates should be combined. `nuvoletta-core` must not depend on Dioxus, a cloud provider, or a concrete filesystem implementation. Keep crypto and sync logic testable without launching the UI.

## 8. Implementation phases

### Phase 0 — Product and security design

Deliverables:
- This plan refined into accepted product requirements.
- Threat model with explicit non-goals and attacker capabilities.
- Versioned encrypted-object format proposal.
- Key hierarchy and recovery-key design (including padding scheme).
- Sync consistency and conflict-resolution design, including tolerance of dumb file-sync layers.
- Technical spike (short): Markdown editor with live preview in Dioxus on Linux and Android (likely an embedded JS editor such as CodeMirror in the webview), platform credential-store access, and Android file access. Consequences to accept or mitigate: decrypted plaintext lives in webview JS memory and cannot be zeroized; the JS editor is a supply-chain dependency; note rendering stays sandboxed. Fallback if the spike fails: Tauri.
- Architecture decision records for major choices.

Exit criteria:
- Security-sensitive design docs have been cross-reviewed with AI assistants (at least two different models) and signed off by the owner before implementation begins. This is not an independent audit.
- Open questions and known limitations are documented rather than silently assumed.

### Phase 1 — Core and encrypted local vault

Implement:
- Workspace and domain types.
- Vault creation, unlock, lock, and passphrase change.
- Encrypted object envelope.
- Filesystem persistence and atomic writes.
- Basic note CRUD.
- Recovery-key wrap and recovery behavior; corruption behavior.
- Local encrypted note snapshots, if cheap.

Tests:
- Correct round-trip encryption/decryption.
- Wrong passphrase rejection.
- Tampering and truncation rejection.
- Nonce uniqueness properties and format validation.
- Crash/interruption simulations for writes.
- No plaintext persistence in the vault directory.

Exit criteria:
- A command-line or integration-test harness can create a vault, write notes, close it, reopen it, and verify the contents without a database.

### Phase 2 — Markdown engine (headless)

Implement:
- Markdown baseline and documented extensions.
- Wikilinks, stable note IDs, backlinks, unresolved links.
- In-memory search and rebuildable index.
- Safe rendering and malicious-input tests.
- Plaintext Markdown export and import (folder of `.md` files).

Exit criteria:
- Automated tests cover parsing, wikilink resolution, backlinks, search, safe rendering of malicious input, and export/import round-trips, with no UI.

### Phase 3 — Dioxus desktop application

Implement:
- Vault picker and unlock flow.
- Note navigation, Markdown editor with live preview, autosave and unsaved-change handling.
- Search, tags, and backlinks.
- Locking and inactivity behavior.
- Platform credential-store integration where appropriate.
- Accessibility, keyboard navigation, and error states.

Exit criteria:
- A user can comfortably create, edit, link, find, and preview notes offline.
- Usable desktop builds on Linux, macOS, and Windows, with core workflows covered by automated tests.

### Phase 4 — Sync engine and synced-folder backend

Implement:
- Provider trait and capability model.
- Local-folder backend (used for tests and as the externally synced folder backend).
- Per-device append-only logs, merge of other devices' logs, and remote inventory.
- Per-object base versions, version comparison and conflict preservation.
- Handling of sync-tool conflict copies, partial or delayed propagation.
- Retry and interrupted-sync recovery.
- Device ID generation (no enrollment or revocation flow in MVP).

Exit criteria:
- Two independent local vault instances can synchronize through a folder, including offline edits, conflicting edits, and simulated delayed/partial propagation, without data loss.

### Phase 5 — WebDAV (cloud APIs post-MVP)

Implement WebDAV, including common NAS deployments. Google Drive and Dropbox backends are post-MVP.

For each provider (and the synced-folder setup guides for common sync tools):
- Document required permissions and setup.
- Handle credentials (and OAuth, for later providers) securely.
- Test upload, download, listing, replacement, deletion, throttling, and interrupted operations.
- Test behavior when provider metadata or revision support is missing.
- Verify that remote objects contain ciphertext and opaque names only.

Exit criteria:
- A desktop vault can synchronize with WebDAV and recover safely from common network/provider failures.

### Phase 6 — Mobile application

Start with Android:
- Verify Dioxus mobile support and required platform APIs early.
- Use app-private storage by default.
- Integrate Android secure credential facilities where appropriate.
- Handle background/foreground transitions and interrupted sync.
- Make editing, locking, and conflict resolution practical on a small screen.

Then assess iOS support against actual framework and platform constraints.

Exit criteria:
- The same vault format and sync protocol work across desktop and Android, with no platform-specific weakening of the security model.

### Phase 7 — Hardening and release

- Dependency and supply-chain review.
- Fuzzing and property-based testing.
- Security-focused code review and independent human security audit (milestone before relying on Nuvoletta for sensitive material).
- Backup, restore, device loss, and recovery-key exercises.
- Performance and battery testing.
- Accessibility and usability review.
- Reproducible release process and signed application updates.
- Clear privacy policy and documentation of metadata leakage and security limitations.

Do not describe the product as audited or fully secure unless an appropriate independent review has actually taken place.

## 9. Testing strategy

Testing is a first-class deliverable, not a final cleanup phase.

- Unit tests for domain logic, crypto envelopes, parsers, and provider adapters.
- Integration tests for complete vault lifecycle and sync flows.
- Property tests for serialization, IDs, path handling, and state transitions.
- Fuzz tests for encrypted envelopes, Markdown input, and remote provider responses.
- Cross-platform CI for supported targets.
- Fault injection for network loss, process termination, partial writes, duplicate events, and provider errors.
- Regression tests for every discovered data-loss or security bug.
- Test fixtures must not contain real personal notes, credentials, or production keys.

## 10. Development guidance for Claude Code

Work incrementally. Do not generate the whole application in one pass.

For each phase:
1. Read this plan and the relevant architecture documents.
2. Identify assumptions and unresolved decisions before coding.
3. Propose a small, reviewable implementation step.
4. Implement with tests and documentation.
5. Run formatting, linting, unit tests, and relevant integration tests.
6. Summarize changed files, design decisions, test results, and remaining risks.
7. Stop at the phase exit criteria and request review before moving into the next security-sensitive phase.

Additional rules:
- Do not invent cryptographic protocols or silently change the encrypted file format.
- Do not add a database unless explicitly justified and approved.
- Do not upload plaintext notes, keys, or secrets to any provider.
- Do not silently discard conflicting edits or remote data.
- Do not weaken security to make a platform integration easier; explain the limitation and propose alternatives.
- Prefer well-maintained Rust crates and official provider APIs.
- Keep dependencies and abstractions proportional to the current phase.
- Treat this plan as a living document: update it when a decision changes, and record significant changes as ADRs.

## 11. Initial decisions and open questions

Current direction:
- Product name: **Nuvoletta**. License: MIT, open source. Solo developer; design reviews are AI-assisted (Claude, ChatGPT) plus owner sign-off.
- UI: Rust + Dioxus (Tauri as fallback if the Phase 0 spike fails).
- Canonical note format: Markdown. Portability via plaintext export/import.
- Local persistence: encrypted files in a vault directory, encrypted at rest; no database for the MVP.
- Encryption: proposed Argon2id + XChaCha20-Poly1305 (random 192-bit nonces) with length padding, subject to review.
- Metadata (title, path, tags) lives inside each note's encrypted envelope; no global index object.
- Keys: single passphrase plus one recovery key per vault (second DEK wrap); enrollment by passphrase; no revocation in MVP.
- Vaults: one vault per install for the MVP; multi-vault support may come later, so avoid hard-coding a singleton vault in core APIs.
- Sync: provider-independent engine with per-device append-only logs; first backend is an externally synced folder, then WebDAV; Google Drive and Dropbox post-MVP.
- Attachments: post-MVP. Local note version history: MVP if cheap, else early post-MVP.
- Platforms: desktop and Android first; iOS feasibility to be validated.
- Plugins: architecture should allow extensions later; plugin runtime is not MVP scope.

Resolve during Phase 0:
- Exact encrypted object and per-device log formats; padding scheme.
- Remote rollback/deletion detection guarantees given the per-device logs.
- Whether a vault has one sync destination initially or can configure multiple destinations.
- Exact Markdown dialect and wikilink resolution rules.
- Outcome of the Dioxus spike (editor, credential store, Android file access).
- Is an encrypted on-disk index cache needed to meet the ~10k-note unlock target on mid-range Android?
- Are local snapshots in the MVP or early post-MVP?
- Future DEK rotation / device revocation design.

---

**Guiding principle:** Nuvoletta should make the secure path the normal path, while keeping the user's notes portable, usable offline, and under the user's control.
