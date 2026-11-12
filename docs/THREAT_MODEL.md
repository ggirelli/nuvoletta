# Nuvoletta Threat Model

Status: Draft — Phase 0, owner decisions applied, pending cross-review

Scope: MVP (PLAN.md rev 2): one vault per install, one passphrase plus one recovery key, per-note encrypted envelopes, per-device append-only sync logs over an externally synced folder (later WebDAV), desktop and Android. Companion docs: `CRYPTO_FORMAT.md`, `SYNC_PROTOCOL.md`.

## 1. Assets

| ID | Asset | Sensitivity |
|----|-------|-------------|
| A1 | Note plaintext (body) | Critical |
| A2 | Note metadata: title, path, tags, timestamps, link graph | High |
| A3 | Passphrase, recovery key | Critical |
| A4 | Passphrase-derived KEK, vault DEK, derived subkeys | Critical |
| A5 | Provider credentials (WebDAV password, later OAuth tokens) | High |
| A6 | Integrity and availability of the user's notes (no silent loss, no silent rollback) | High |
| A7 | Application binary and update channel | High |
| A8 | Leakable metadata: object sizes, counts, timing, device count, edit frequency | Medium |

## 2. Actors and capabilities

| ID | Actor | Capabilities assumed |
|----|-------|----------------------|
| X1 | Thief with a **locked** device | Full physical access, can image storage, cannot unlock the OS or app. May run offline brute force against the vault header. |
| X2 | Provider or NAS administrator (honest-but-curious) | Reads every remote file, name, size, timestamp, access pattern. |
| X3 | Malicious or compromised sync provider / network attacker | X2 plus: modify, truncate, delete, withhold, reorder, delay, replay old valid files, serve different views to different devices, inject files. Cannot break the primitives. |
| X4 | Attacker on a device while the vault is **unlocked** | Malware or an interactive user with OS-level access to process memory, screen, clipboard, files. |
| X5 | Malicious content author | Crafts a note, an imported `.md` file or an imported folder (hostile Markdown, huge files, path tricks, link targets). |
| X6 | Token thief | Obtains provider credentials from backups, logs or a compromised endpoint. Equivalent to X3 for the stolen credentials' scope. |
| X7 | Supply-chain attacker | Malicious update, compromised crate, compromised JS editor dependency, compromised build. |
| X8 | Local forensic reader | Reads swap, hibernation file, crash dumps, logs, OS backups, temp files, thumbnails, clipboard history of a device later seized. |
| X9 | Removed or lost enrolled device holder | Holds the DEK from a device that was lost or retired (no revocation in MVP). |

## 3. Security goals

- G1. Confidentiality of A1 and A2 against X1, X2, X3, X6 given a strong passphrase or the recovery key. X2, X3 and X6 can read the synced vault header and so can mount the same offline attack as X1 (see T1).
- G2. Integrity: any modified, truncated, substituted or cross-object-swapped ciphertext is detected and never yields partial plaintext.
- G3. No silent data loss: concurrent edits and delete-vs-edit are preserved as conflicts; interrupted writes recover.
- G4. Detect (not prevent) deletion, rollback and equivocation by X3 to the extent the per-device logs allow (see `SYNC_PROTOCOL.md` section 10).
- G5. Safe rendering: note content never executes script or loads active or remote content.
- G6. Minimal plaintext footprint: no plaintext note data persisted outside the encrypted vault except explicit user export.
- G7. Secrets are never logged, never sent to the provider, never stored in the vault.
- G8. Passphrase change and recovery do not require re-encrypting notes.

## 4. Non-goals (explicit)

- N1. Protection against X4 while unlocked (malware, keyloggers, screen capture, memory dump). Locking shortens the window only.
- N2. Hiding that Nuvoletta is in use, the number of devices, approximate edit times, and approximate note counts and sizes from X2/X3. Padding blurs size only.
- N3. Device revocation or DEK rotation (MVP). X9 retains access to everything it synced, and to anything it can still read later.
- N4. Preventing a provider from rolling back everything consistently to a state a device has not seen past, or withholding data (availability). Only detection with stated limits.
- N5. Defence against a hostile device that holds the DEK forging another device's log entries (all devices share one DEK; no per-device signatures in MVP).
- N6. Zeroizing plaintext in the webview JS heap, or the guarantee that Rust zeroization removes every copy.
- N7. Multi-user sharing, plugin sandboxing, attachments (post-MVP).
- N8. Protection of user data against a weak or reused passphrase beyond Argon2id cost; the recovery key is the strong fallback.
- N9. Independent audit status: none. The product must not claim audited status.

## 5. Threats, mitigations, residual risk

| # | Threat (actor) | Mitigation | Residual risk |
|---|----------------|------------|---------------|
| T1 | Offline brute force of passphrase from stolen device or header (X1; also X2 if the header is in the synced folder) | Argon2id with calibrated high cost, 128-bit random salt, params authenticated, strength meter and minimum length, recovery key is 256-bit (not guessable) | Weak passphrases fall to a well-resourced attacker. Header is in the sync folder, so X2/X3 also get the offline-attack opportunity. |
| T2 | Provider reads notes or titles (X2) | Per-object AEAD; titles, paths, tags inside the envelope; random opaque object names; Padmé padding | Sizes (bucketed), counts, timing, device IDs, directory structure leak. |
| T3 | Provider modifies, truncates or swaps objects (X3) | XChaCha20-Poly1305 with AAD binding vault, object type, object id, scope id; log entries hash-chained; log entries carry the content hash | Denial of service (undecryptable object). |
| T4 | Replay of an older valid object or log segment (X3) | Version DAG records parents; per-peer last-seen seq and hash remembered locally; checkpoint vectors cross-attest progress | Consistent rollback to a state a device has not yet seen past is undetectable. |
| T5 | Deletion or withholding of objects or whole logs (X3) | Log entries reference content hash; missing referenced object is reported as "dangling", never silently dropped; seq gap detection; checkpoint vectors reveal a missing peer log | Newest entries of an unseen log and logs of never-seen devices can be hidden. |
| T6 | Equivocation: different views to different devices (X3) | Checkpoint vectors in later entries expose divergence once the devices see each other's logs | Detected only after a later exchange, and only if the provider relays at least one cross-reference. |
| T7 | Stale header served: old passphrase still unlocks, or a downgraded KDF cost (X3) | Header MAC and AAD cover KDF params; monotonic `header_gen` remembered per device and compared | A device that has never seen a newer header accepts the old one. After a passphrase change due to compromise, rotate trust by assuming the old passphrase plus old header remain valid (document). |
| T8 | Lost locked device (X1) | OS app-private storage, full-disk encryption recommended, vault at rest is ciphertext, local caches encrypted under a derived subkey | Needs strong passphrase; OS-level weaknesses. |
| T9 | Compromised unlocked device (X4) | Auto-lock, manual lock, zeroize keys on lock, avoid persisting plaintext, Android `FLAG_SECURE` | Not defended (N1). Whatever was open is lost. |
| T10 | Plaintext in webview JS memory (X4, X8) | Minimise lifetime by clearing editor state on lock; document limit | Cannot be zeroized (N6). Crash dumps of the webview process may contain note text. |
| T11 | Malicious note or imported Markdown (X5) | Raw HTML disabled or strictly sanitised; no scripts, no remote loads, CSP on the webview, link scheme allowlist, size and depth limits in parser, import path sanitisation (no `..`, symlinks, absolute paths) | Parser bugs; fuzzing in Phase 2 and 7. |
| T12 | Malicious or corrupt envelope or log (X3, X5) | Bounded, strict parsers; length checks before allocation; decode only after AEAD verification; fuzzing; reject unknown versions | Parser bugs before authentication (header only). |
| T13 | Provider token theft (X6) | Tokens in OS credential store, never in the vault or synced data, narrowest scope; tokens give ciphertext access only | Thief acts as X3 for that account. |
| T14 | Malicious update or dependency (X7) | Pinned and reviewed crates (`cargo-vet`/`cargo-deny`), vendored JS editor with hash, reproducible builds, signed updates (Phase 7) | A compromised signing key or build wins everything. |
| T15 | Swap, hibernation, temp files, logs, crash dumps, OS backups (X8) | No plaintext temp files; secrets never logged; atomic writes via encrypted temp in vault dir; `mlock` best-effort on desktop; disable crash reporters that capture memory; local app data excluded from cloud backup where the OS allows | Swap and hibernation on desktops not controlled; OS clipboard history. |
| T16 | Decryption oracle / key misuse across contexts | HKDF domain-separated subkeys; AAD binds object type | Implementation mistakes; mitigated by test vectors. |
| T17 | Nonce reuse | XChaCha20 192-bit random nonces from OS CSPRNG | Broken RNG at boot on exotic devices. |
| T18 | Removed device still reads (X9) | Documented limitation (N3) | Full residual. Future DEK rotation. |
| T19 | Forged log entries by an enrolled but hostile device (X9, N5) | None in MVP | Full residual; per-device signing keys are future work. |
| T20 | Data loss from sync bugs or crashes | Conflict-preserving merge, immutable objects, atomic writes, local snapshots | Bugs; tested via fault injection. |
| T21 | Lost passphrase and recovery key | By design unrecoverable | User loses vault. Onboarding must make the recovery key step unmissable. |
| T22 | Plaintext export left on disk | Explicit export action with warning; no auto-export | User responsibility. |
| T23 | Side channels, shoulder-surfing, screenshots | `FLAG_SECURE` on Android; constant-time comparisons via `subtle` | Out of scope on desktop. |

## 6. Trust boundaries

1. Rust core and OS CSPRNG: trusted.
2. Webview (editor, preview): untrusted for isolation from note content; holds plaintext.
3. Sync folder or WebDAV server: fully untrusted, sees only ciphertext.
4. Other enrolled devices: trusted with the DEK (N3, N5).

## 7. Open points

- **Decided:** T9/T23: Android `FLAG_SECURE` is on by default (screenshots and app-switcher thumbnails disabled); user-toggleable on desktop where supported.
- **Decided:** T15: no automatic clipboard clearing in MVP; the limit is documented to users.
- **Decided:** T1/T7: the vault header lives in the synced folder (new devices need it). X2 can mount offline guessing; mitigated by Argon2 cost and a 12-character minimum passphrase with a strength warning.
- **Decided:** T14: no in-app auto-updater in MVP; rely on signed store packages and signed release artifacts.
- **Decided:** Webview CSP and loopback server: the Dioxus spike result is GO. Release is blocked if any localhost HTTP server exposes plaintext to other local processes. Plaintext in webview memory is accepted and documented.
- **Decided:** Editor: CodeMirror 6 as a vendored bundle (~600 KB, no CDN); its JS dependency tree is under supply-chain review.
- **Decided:** MVP unlock is passphrase-only on all platforms; Android Keystore/biometric unlock is post-MVP. Android minimum API level is 28.
- **Decided:** Preview: raw HTML is escaped; link URL scheme allow-list is http, https, mailto and internal wikilinks (`javascript:` and all others blocked); remote images are blocked by default.
