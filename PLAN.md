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
- Keep provider credentials out of the vault's synced data and out of plaintext config. **Decided (2026-11, ADR 0007):** WebDAV credentials are stored only in this device's local state (`.nuvoletta/device-state/webdav`), sealed as envelope type 4 with `K_local`, never synced, available only while the vault is unlocked. This is stronger than the Linux and Windows keyrings while the vault is locked (any process of the user can read a keyring), the same on every platform and needs no JNI on Android. Limit: the vault passphrase plus the device files reveal the credential. Mitigation: use a dedicated account or app password that can only read and create (and is therefore also refused if it can delete or overwrite, below), and rotate it with the passphrase.

**Provider targets:** the first real backend is an **externally synced folder**: a local folder that the user syncs with Syncthing, Dropbox, iCloud, a Nextcloud client, or similar. Nuvoletta only reads and writes files there. The second backend is **WebDAV** (Phase 5; it was deferred out of the MVP and was brought back because it is the route to Android sync), used only with an account that can read and create but not delete or overwrite. Google Drive and Dropbox (API) backends are post-MVP. Keep the provider interface generic so other backends can be added later. A plain local-folder backend remains useful for tests.

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
- WebDAV backend: implemented (Phase 5).
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
- Store refresh tokens (OAuth providers, post-MVP) in the OS credential store.
- Never put provider credentials in synchronized vault data.
- **WebDAV credentials (decided 2026-11):** address, user name and password only in the local `device-state/webdav` file, sealed with `K_local` (envelope type 4), never synced, usable only while the vault is unlocked, zeroized in memory where practical, never logged. Why not a keyring: on Linux and Windows any process of the user can read it while the vault is locked; the sealed file is unreadable without the vault passphrase, is identical on all platforms and needs no JNI on Android. Limit: passphrase plus device files reveal the credential; mitigation: dedicated account or app password, rotate with the passphrase.
- **WebDAV accounts must not be able to delete or overwrite** (checked by trying, at setup and every session; unknown is refused), so that a stolen sync password cannot destroy the remote copy (`docs/WEBDAV.md`).
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
│   ├── nuvoletta-app-core/   # UI-independent app logic: session, autosave, auto-lock, queries
│   ├── nuvoletta-sync/       # Sync state machine and conflict handling
│   ├── nuvoletta-provider/   # Provider traits and shared sync types
│   # nuvoletta-webdav (HTTPS client, credential check, WebDAV provider); post-MVP: nuvoletta-google-drive, nuvoletta-dropbox
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
- ~~Platform credential-store integration where appropriate.~~ **Deferred** (owner decision, see section 11): unlock is passphrase-only in the MVP.
- Accessibility, keyboard navigation, and error states.

Decisions made in Phase 3 (details in `docs/ADR/0004-desktop-app-architecture.md` and `docs/APP.md`):
- All non-UI logic lives in `nuvoletta-app-core` and is tested without a webview; `apps/nuvoletta-app` is a thin Dioxus view.
- Auto-lock after inactivity, default 5 minutes, configurable (1 to 1440; no "never"), plus a manual lock shortcut. Locking saves first; if saving fails the vault stays unlocked and nothing is discarded unless the user explicitly forces it.
- The webview gets a strict Content-Security-Policy; the limits Dioxus 0.7 desktop imposes on it are documented in the ADR.
- CI builds the desktop app on Linux in every pull request; a nightly workflow builds and tests it on Linux, macOS and Windows and runs `just test-slow` on Linux.

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

Decisions made in Phase 4 (details in `docs/ADR/0005-sync-protocol.md` and the implementation notes of `docs/SYNC_PROTOCOL.md`):
- New crate `nuvoletta-sync`: `Provider` trait with a capability model (`create_only`, `atomic_rename`, etags, mtimes), the synced-folder backend, strict name grammar and stray-file classification, hash-chained per-device logs, a per-note version DAG with heads, metadata-only 3-way merge, explicit resolution actions (choose head, keep both, keep/delete), encrypted local sync state, checkpoints, rollback/withholding/fork detection and the idempotent, crash-safe sync cycle.
- Local edits are found by scanning the vault (no storage hooks); storage only gained small accessors (`keys`, `header`, `read_revision_bytes`, `install_revision`) and public atomic-write helpers. No on-disk format changed.
- Designated device for merging identical-content heads, fork handling refinements (unpublished divergence versus visible fork) and the other points the protocol text left open are listed in the SYNC_PROTOCOL.md implementation notes.
- Tests: in-process vault instances over a fault-injecting folder (drops, delays, truncation, duplicates, conflicted copies, rollback, outages), scenarios S1 to S5, crash at every cycle step and every provider operation, hostile-file tests and proptest convergence (the long four-device run is `#[ignore]`, `just test-slow`).
- Deferred: snapshots, hourly cap of standalone checkpoints. Garbage collection, cache pruning and gap buffering: **done** (see "Decisions made in the Phase 4 sync follow-ups" below).
- Follow-up (out of scope for this phase): app integration (sync settings, sync status, conflict list and resolution UI, "needs attention" and diagnostics screens, scheduling and the folder-picker flow). **Done** (see "Decisions made in the Phase 4 app integration" below).

Decisions made in the Phase 4 app integration (details in `docs/APP.md`):
- All logic is in `nuvoletta-app-core` (new modules `sync`, `sync_settings`, `sync_messages`, `sync_types`, `diff`); the app only renders it.
- A cycle runs on a worker thread that is handed the unlocked vault and the syncer and returns them over a channel (the engine needs `&mut Unlocked` for the whole cycle, and nothing else may write the vault meanwhile). While away, note list (snapshot), search, preview and typing keep working; vault-writing operations return "sync busy" and autosave waits. Lock and close wait for the cycle (it cannot be cancelled from outside; it is short and crash-safe); a lock requested meanwhile is carried out after the hand-back, saving first.
- Scheduling (clock-based, tested with a manual clock): first cycle after unlock, 5 s after a save, every 5 minutes, never while locked or while the note has unsaved typing; an error is retried on the schedule, "Sync now" always works.
- Settings live in `.nuvoletta/device-state/sync.json` (device-local, unencrypted, no secrets) next to `sync-notices.json` (unacknowledged integrity warnings, which only an explicit user action removes). The sync folder must exist (an unmounted drive is reported, never recreated: the folder backend treats a missing directory as empty) and be separate from the vault.
- Conflicts: list, per-head view with a line diff against the local version, choose head, keep both (exactly two edited heads), keep/delete for delete-versus-edit. Merging by hand is "use this version and edit": the sync API has no "resolve with custom content" action.
- Deferred: resolving with hand-merged content in one step, keep-both for three or more heads, adopting a newer vault header (re-unlock flow), a keyboard shortcut for sync, cancelling a running cycle, status display of sync progress counts, switching sync folders without a rollback warning.

Decisions made in the Phase 4 sync follow-ups (details in the SYNC_PROTOCOL.md implementation notes 8, 16 and 17):
- Manual garbage collection (`Syncer::collect_garbage`, dry run, injected clock; 90-day ack window, 30-day grace, K = 5, 7-day orphans): deletes only content objects this device wrote, never log segments (snapshots are post-MVP), never anything of a note in conflict, never anything while a fork is recorded or the state is rebuilding; keeps heads, head parents and the last K; unknown or silent devices block deletion. "Clean up old versions" in the Sync dialog: preview, explicit confirmation, result; logic in app-core.
- "Recently deleted" view with Restore (app-core `recently_deleted` / `restore_note`): merges sync tombstones (restore = PUT with the tombstone as parent) and a local trash (`.nuvoletta/trash/`, see docs/STORAGE_FORMAT.md section 3.1) so vaults without sync can restore too. Local trash entries expire after 30 days automatically; no manual purge anywhere. Local-only layout addition; no envelope or wire change.
- The local `revs/` cache is pruned after every sync cycle (base, heads, head parents, last K, all of a conflict).
- Segments after a gap are buffered in the encrypted `pending` state (bounded) and no longer refetched. Local, rebuildable state only; no on-disk or on-wire format changed.
- Deferred: purging the last versions of deleted notes, snapshots and own-segment compaction.

Exit criteria:
- Two independent local vault instances can synchronize through a folder, including offline edits, conflicting edits, and simulated delayed/partial propagation, without data loss.

### Phase 5 — WebDAV

Status: implemented (un-deferred 2026-11 as the route to Android sync). Decisions and rationale: `docs/ADR/0007-webdav-backend.md`; user guide: `docs/WEBDAV.md`.

- HTTPS only; rustls with the built-in web PKI roots (no JNI), optional pin of one self-signed certificate by SHA-256 fingerprint after explicit confirmation. `ureq` 3 (rustls, `allow_non_standard_methods`) plus `quick-xml`; pinning uses a small custom TLS connector because ureq has no verifier hook.
- **Credentials** (option A): address, user name and password only in `device-state/webdav`, sealed with `K_local` (section 2.3), never synced.
- **Refuse accounts that can delete or overwrite.** At setup and before the first cycle of every session the account is proven to read and create but not overwrite, copy over, move or delete (probe files `.nuvoletta-probe*`, `oc:permissions` as a fast path, unknown fails closed); otherwise the UI explains and nothing is stored or used. Consequently: uploads are create-only `PUT`s to the final name, no GC ("Clean up old versions" is unavailable), header generations are never deleted.
- **Partial uploads** are handled in the engine without a protocol change: a half-written own object is re-sealed under a new id, a half-written own log segment makes the device continue under a new device id (existing fork recovery), after a grace period and only on providers whose listing is authoritative.
- Android uses WebDAV for sync (the synced-folder option stays desktop-only).

For each provider (and the synced-folder setup guides for common sync tools):
- Document required permissions and setup. **Done:** `docs/WEBDAV.md`.
- Handle credentials (and OAuth, for later providers) securely.
- Test upload, download, listing, replacement, deletion, throttling, and interrupted operations. **Done** against an in-process TLS WebDAV server (`nuvoletta-webdav` feature `test-server`).
- Test behavior when provider metadata or revision support is missing. **Done** (no etags or mtimes, no `If-None-Match`).
- Verify that remote objects contain ciphertext and opaque names only. **Done.**

Needs a real server (not run): Nextcloud with a read+create custom-permission share and an app password; a NAS with a self-signed certificate (pin flow); a server that keeps half-written uploads; Android over mobile data. Steps in the PR description.

Exit criteria:
- A desktop vault can synchronize with WebDAV and recover safely from common network/provider failures. **Status:** met against the test server; real-server checks above are open.

### Phase 6 — Mobile application

Status: Android implemented (this section records what is verified and what needs a device); iOS assessed only. Decisions: `docs/ADR/0006-android-app.md`.

Start with Android:
- Verify Dioxus mobile support and required platform APIs early. **Verified:** `dx build --platform android` builds the app (debug and release, arm64) with `just android`; `aapt2` confirms application id `dev.nuvoletta.app`, minSdk 28 and a single permission (`just android-permissions`).
- Use app-private storage by default. **Done:** vault and settings live in the app-private files directory (path derived without JNI), no storage permission, `allowBackup=false`.
- Permissions: `INTERNET` only, because Dioxus 0.7 delivers UI updates over a loopback WebSocket and Android refuses loopback sockets without it. The CSP, the network security config and the absence of network code keep it from reaching a network (ADR 0006).
- Android syncs through **WebDAV** (Phase 5; sync controls are enabled for it, the synced-folder option stays desktop-only). The synced-folder route would need a user-chosen folder reached through the Storage Access Framework. **Assessed, not implemented:** SAF can in principle meet the provider contract (best-effort create-only, rename where the provider supports it, no symlink information), but it needs Kotlin glue and a confined JNI dependency, is slow without a document-id cache, and cannot be tested without a device. The folder option is hidden on Android with an in-app message; the vault format is unchanged. The recommended design and the alternatives are in ADR 0006. Raised for owner decision before committing to Android sync.
- iOS has no equivalent shared-folder model beyond Files providers; WebDAV is the sync route there too.
- Integrate Android secure credential facilities where appropriate. **Not applicable in the MVP:** passphrase-only unlock (section 11) and no stored credentials.
- Handle background/foreground transitions and interrupted sync. **Done:** leaving the foreground saves and then locks (never discards; a failed save keeps the vault unlocked and says so on return); sync is safe to interrupt by design. `FLAG_SECURE` hides content from screenshots and recents.
- Make editing, locking, and conflict resolution practical on a small screen. **Done in CSS and view state:** list and note take turns, wrapping top bar, 44 px touch targets, detail fields behind a toggle; conflict view and dialogs reuse the responsive dialog.

Then assess iOS support against actual framework and platform constraints. **Assessed (ADR 0006):** the app should build with wry on WKWebView, but there was no viable iOS sync backend before WebDAV; with WebDAV in place iOS can be reassessed.

Needs on-device testing (nothing was run on a device or emulator): launch and create or unlock a vault; the lifecycle lock on home, app switch and screen off; `FLAG_SECURE` (screenshot and recents blank); soft keyboard with the CodeMirror editor (IME, selection, resize); layout at phone sizes; that no note text reaches `adb logcat`; unlock time for a large vault on a mid-range phone.

Exit criteria:
- The same vault format and sync protocol work across desktop and Android, with no platform-specific weakening of the security model. **Status:** the vault format is shared and no security rule was relaxed; the sync half is met by WebDAV (untested on a device).

### Phase 7 — Hardening and release

- Dependency and supply-chain review. Status: done for the first pass (`docs/SUPPLY_CHAIN.md`: direct dependencies with rationale, `unsafe` and build-script footprint, add-a-dependency policy; `deny.toml` tightened: unmaintained and unsound anywhere, OpenSSL/`native-tls` banned except via Dioxus devtools, no git sources; `cargo vet` considered and declined, with reasons; nightly workflow actions pinned to SHAs, `ci.yml`/`docs.yml` still to pin). `cargo deny` covers the `cargo audit` advisory check.
- Fuzzing and property-based testing. Status: fuzzing done (`fuzz/`, 12 `cargo-fuzz` targets with invariants over the header, envelope, payload, log segment, local state, remote names, WebDAV XML and `href`, Markdown renderer/sanitiser, wikilinks, plaintext import and link classifier; nightly CI job, 10 minutes per target, crash reproducers uploaded). Property tests exist per crate since Phases 1 to 5.
- Security-focused code review and independent human security audit (milestone before relying on Nuvoletta for sensitive material).
- Backup, restore, device loss, and recovery-key exercises. **Done** (`crates/nuvoletta-app-core/tests/recovery.rs`; procedures in `docs/user/`). Known gaps found: no in-app join (the vault header is copied by hand from the sync folder's `h/`), no in-app adoption of a newer header after a passphrase change on another device, and a corrupt `journal` stops sync until deleted.
- Performance and battery testing.
- Accessibility and usability review.
- Reproducible release process and signed application updates.
- Clear privacy policy and documentation of metadata leakage and security limitations. **Done** (`docs/user/privacy-policy.md`, `docs/user/security-and-privacy.md`; states that the app is not audited).

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
- UI: Rust + Dioxus (spike passed: GO).
- Canonical note format: Markdown. Portability via plaintext export/import.
- Local persistence: encrypted files in a vault directory, encrypted at rest; no database for the MVP.
- Encryption: proposed Argon2id + XChaCha20-Poly1305 (random 192-bit nonces) with length padding, subject to review.
- Metadata (title, path, tags) lives inside each note's encrypted envelope; no global index object.
- Keys: single passphrase plus one recovery key per vault (second DEK wrap); enrollment by passphrase; no revocation in MVP.
- Vaults: one vault per install for the MVP; multi-vault support may come later, so avoid hard-coding a singleton vault in core APIs.
- Sync: provider-independent engine with per-device append-only logs; backends: an externally synced folder, and WebDAV with create-only credentials (un-deferred 2026-11, ADR 0007); Google Drive and Dropbox post-MVP.
- Attachments: post-MVP. Local note version history: MVP if cheap, else early post-MVP.
- Platforms: desktop and Android first; iOS feasibility to be validated.
- Plugins: architecture should allow extensions later; plugin runtime is not MVP scope.
- Dioxus spike: GO (desktop and Android build verified; Android run verified only as far as a successful APK build, see Phase 6). Editor is a vendored CodeMirror 6 bundle (~600 KB, no CDN), JS dependency tree under supply-chain review.
- Unlock: passphrase-only on all platforms in the MVP; platform credential-store integration (desktop keyrings, Android Keystore/biometric unlock) is deferred to post-MVP. Android minimum API level 28.
- Desktop app: auto-lock after 5 minutes of inactivity by default (configurable); export always warns that it writes plaintext; the webview runs under a strict CSP (ADR 0004).
- Markdown: pulldown-cmark, CommonMark + tables, strikethrough, task lists, footnotes; `[[Target|alias#fragment]]` wikilinks resolved by full path, then last segment or title (NFC, case-insensitive, ambiguity reported), see docs/ADR/0003-markdown-dialect-and-wikilinks.md.
- Preview: raw HTML escaped; link scheme allow-list (http, https, mailto, internal wikilinks); remote images blocked by default.
- Crypto and sync decisions (Argon2id 64 MiB/t=3/p=1, Padme padding, CBOR/minicbor, sharded `o/`, multi-head conflicts, manual GC, snapshots post-MVP) are recorded in docs/CRYPTO_FORMAT.md and docs/SYNC_PROTOCOL.md; local snapshots are post-MVP.

Resolve during Phase 0:
- Whether a vault has one sync destination initially or can configure multiple destinations.
- Is an encrypted on-disk index cache needed to meet the ~10k-note unlock target on mid-range Android?
- Future DEK rotation / device revocation design (post-MVP).

---

**Guiding principle:** Nuvoletta should make the secure path the normal path, while keeping the user's notes portable, usable offline, and under the user's control.
