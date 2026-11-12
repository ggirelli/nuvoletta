# Nuvoletta Cryptographic Format (v1 proposal)

Status: Draft — Phase 0, owner decisions applied, pending cross-review

Only standard constructions are used: Argon2id (RFC 9106), HKDF-SHA256 (RFC 5869), HMAC-SHA256, XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha, libsodium-compatible). No custom primitives or protocols. All integers are big-endian unless stated. "‖" is concatenation.

## 1. Key hierarchy

```
passphrase --Argon2id(salt, m,t,p)--> KEK_pw (32 B) --+
recovery key RK (32 B) --HKDF(vault_id)--> KEK_rk ----+--> unwrap --> DEK (32 B, random)
DEK --HKDF-SHA256--> K_header | K_note | K_log | K_local   (32 B each)
```

- DEK: 32 random bytes from the OS CSPRNG at vault creation. Never stored in the clear.
- HKDF-SHA256: `ikm = DEK`, `salt = vault_id` (16 B), `info = "nuvoletta/v1/" ‖ label`, L = 32.

| Subkey | Label | Used for |
|--------|-------|----------|
| K_header | `header-auth` | Header MAC |
| K_note | `note-content` | Note revision envelopes (type 1) |
| K_log | `device-log` | Log segment envelopes (type 2), snapshots (type 3) |
| K_local | `local-cache` | Local-only state, caches, snapshots (type 4) |

Distinct labels give independent keys; a key for one context is never used in another. Adding a label never changes existing keys.

## 2. Vault header (`vault header`, 236 bytes, fixed layout)

| Off | Len | Field | Notes |
|-----|-----|-------|-------|
| 0 | 8 | magic | `4E 56 4C 54 48 44 52 00` ("NVLTHDR\0") |
| 8 | 2 | format_version | `0x0001` |
| 10 | 4 | header_gen | u32, starts at 1, +1 on every header rewrite (rollback hint, see 9) |
| 14 | 16 | vault_id | random, immutable for the vault lifetime |
| 30 | 1 | kdf_id | `0x01` = Argon2id, version 0x13 |
| 31 | 4 | m_kib | memory in KiB |
| 35 | 4 | t | iterations |
| 39 | 4 | p | parallelism |
| 43 | 16 | salt | random, regenerated on every passphrase change |
| 59 | 1 | wrap_alg | `0x01` = XChaCha20-Poly1305 |
| 60 | 24 | pw_nonce | random |
| 84 | 48 | pw_wrap | AEAD(KEK_pw, DEK) = 32 ct + 16 tag |
| 132 | 24 | rk_nonce | random |
| 156 | 48 | rk_wrap | AEAD(KEK_rk, DEK) |
| 204 | 32 | header_mac | HMAC-SHA256(K_header, bytes[0..204]) |

- Wrap AAD: `bytes[0..60] ‖ "pw"` for the passphrase wrap, `bytes[0..60] ‖ "rk"` for the recovery wrap. The KDF parameters, salt, vault_id and generation are therefore authenticated before the DEK is trusted; tampering with Argon2 parameters (a cost-downgrade or memory-exhaustion attack) makes unwrap fail.
- `header_mac` is checked after unwrap. It binds both wraps together (a provider cannot splice wraps from two headers) and, because it is keyed by a DEK-derived key, also commits the wrap to the DEK (see 7).
- Parsers must enforce: exact length, magic, supported version, `m_kib` in [8192, 1048576], `t` in [1, 16], `p` in [1, 8], before running Argon2 (DoS guard).
- Stored locally at `.nuvoletta/vault-header`; stored remotely as an immutable per-generation file (`SYNC_PROTOCOL.md` section 2).

### 2.1 Argon2id defaults

- **Decided:** Argon2id `m = 65536 KiB (64 MiB)`, `t = 3`, `p = 1`, output 32 B, salt 16 B, calibrated on mid-range Android (target 0.5 to 1.5 s; raise `t`/`m` on desktop-created vaults only if the slowest enrolled device stays under about 2 s). This is RFC 9106's constrained profile with `p=1` so mid-range Android behaves predictably. Parameters are per-vault (header), so an upgrade is a passphrase re-wrap (section 8).
- Passphrase input is UTF-8, NFKC-normalised, no trimming. **Decided:** NFKC normalisation; minimum length 12 characters with a zxcvbn-style strength warning, no hard composition rule.

## 3. Recovery key

- 32 random bytes (256 bits), generated once, shown once, never stored by the app.
- Printable encoding: `payload = RK (32 B) ‖ check (3 B)` where `check = SHA-256("nuvoletta/v1/recovery-check" ‖ RK)[0..3]`. The 35 bytes (280 bits) encode to exactly 56 Crockford Base32 characters, printed as 14 groups of 4 separated by `-`, e.g. `ABCD-EFGH-...`. Decoding is case-insensitive, ignores `-` and whitespace, maps `O→0`, `I,L→1`; a wrong checksum is reported as "typo", not as a wrong key.
- KEK: `KEK_rk = HKDF-SHA256(ikm = RK, salt = vault_id, info = "nuvoletta/v1/recovery-kek")`. No Argon2 is needed: RK has 256 bits of entropy.
- **Decided:** 256-bit key plus 24-bit checksum, Crockford base32 in 14 groups of 4, KEK via HKDF. Also offer a QR rendering of the same string and a "re-print / rotate recovery key" action (section 8).
- Exactly one recovery wrap exists in MVP.

## 4. Object envelope (all vault objects)

Fixed 80-byte header, followed by ciphertext ‖ 16-byte tag.

| Off | Len | Field | Notes |
|-----|-----|-------|-------|
| 0 | 4 | magic | `4E 56 4F 42` ("NVOB") |
| 4 | 1 | format_version | `0x01` |
| 5 | 1 | alg_id | `0x01` = XChaCha20-Poly1305 with padded plaintext |
| 6 | 1 | obj_type | 1 note revision, 2 log segment, 3 snapshot (reserved), 4 local-only state |
| 7 | 1 | flags | must be 0 in v1; nonzero is rejected |
| 8 | 16 | vault_id | must equal the header's |
| 24 | 16 | object_id | random per object; the remote file name stem; revision id for notes |
| 40 | 16 | scope_id | note_id for type 1, device_id for types 2/3, zero for type 4 |
| 56 | 24 | nonce | fresh random 192-bit per encryption |
| 80 | n | ciphertext ‖ tag | n = padded_len + 16 |

- **AAD = bytes[0..80]** (every header field including the nonce). Any header change fails authentication. Key = K_note for type 1, K_log for 2 and 3, K_local for 4.
- A reader checks, in order: length ≥ 96, magic, version, alg_id, obj_type known, flags = 0, vault_id; then AEAD; then (only after success) parses the plaintext and checks `scope_id`/`object_id` against the context (file name, log entry reference). Authentication failure returns an error with no plaintext.
- Nonces are random; with 192 bits the collision probability is negligible for any realistic number of objects per key (about 2^-96 at 2^48 objects). A fresh nonce is generated for every encryption, including re-encryption of identical content.
- Notes are encrypted whole; there is no chunking in v1 (attachments, post-MVP, will define a chunked object type).

## 5. Padding (Padmé)

Plaintext fed to the AEAD is `u32_le(len) ‖ payload ‖ zeros`, total length `L' = padme(max(len + 4, 256))`.

```
padme(L): E = floor(log2 L); S = floor(log2 E) + 1; z = E - S
          mask = (1 << z) - 1;  return (L + mask) & ~mask
```

- Padmé (Nikitin et al., PETS 2019) bounds overhead to about 12 percent (less for large objects) and leaks O(log log L) bits of length. 256 B floor hides tiny-note sizes.
- On decrypt, the zero padding is verified (reject if non-zero) after authentication, and `len + 4 ≤ L'`.
- **Decided:** Padmé with a 256 B floor and no upper cap, applied to every object type including log segments. Overhead is accepted because notes are small.

## 6. Payloads

### 6.1 Serialization choice

**CBOR (RFC 8949)** via the `minicbor` crate, with integer map keys. Justification: self-describing, so fields can be added without a version bump and old readers skip unknown keys; a stable IETF standard with independent implementations (useful for the test vectors and future non-Rust tools); strict decoding limits (depth, length) are available for hostile input. `postcard` is more compact but not self-describing, which makes additive evolution fragile. Compactness is irrelevant after padding. **Decided:** CBOR via `minicbor`; encoders must emit definite-length items and sorted integer keys (deterministic enough for hashing within one implementation; hashes are always computed over the stored bytes, never re-encoded).

### 6.2 Note revision payload (type 1)

CBOR map:

| Key | Type | Field |
|-----|------|-------|
| 0 | uint | schema_version (1) |
| 1 | bstr(16) | note_id (must equal envelope scope_id) |
| 2 | tstr | title |
| 3 | tstr | path (virtual, `/`-separated, validated: no `..`, no empty segment, no leading `/`, NFC) |
| 4 | array of tstr | tags |
| 5 | int | created_at (ms since Unix epoch, UTC, informational) |
| 6 | int | modified_at (ms, informational, never used for conflict resolution) |
| 7 | tstr | body (Markdown, UTF-8) |

Readers ignore unknown keys, reject schema_version greater than supported, and enforce limits (body ≤ 16 MiB, title ≤ 1 KiB, ≤ 256 tags). There is no tombstone payload: deletes are log entries (`SYNC_PROTOCOL.md`).

### 6.3 Other payloads

Log segment (type 2) and local state (type 4) payloads are CBOR defined in `SYNC_PROTOCOL.md`. They use the same envelope.

## 7. Key commitment

XChaCha20-Poly1305 (like AES-GCM) is not key-committing: a ciphertext can be crafted that authenticates under two different keys. This matters in "partitioning oracle" settings where an attacker submits chosen ciphertexts and observes accept/reject across many candidate keys (e.g. a server testing guessed passwords), and in multi-recipient or franking schemes. Here:

- Object envelopes have exactly one key per type derived from one random DEK; nobody tests candidate keys against attacker-supplied objects. Not exploitable.
- The wraps are the only place passwords are tested. An attacker can craft a wrap that opens under two guessed passphrases, but gains nothing: the user's app only accepts a wrap if the result also validates `header_mac`, which is keyed from the unwrapped DEK, so a forged wrap does not verify. This is the explicit key commitment for the header.
- **Decided:** no per-object key-commitment tag in v1; the header MAC commits the wraps. Revisit if multi-key (sharing, co-existing DEKs) is introduced, then add a committing construction under a new `alg_id`.

## 8. Operations

- **Create vault**: generate `vault_id`, DEK, salt; derive KEK_pw; generate RK; produce header with both wraps and MAC; show RK; require the user to re-enter part of the RK (confirmation) before the vault is usable.
- **Unlock**: parse and validate header; Argon2id; unwrap with AAD; verify `header_mac`; derive subkeys. Wrong passphrase and corrupted header give the same generic error to the UI (with distinct logs without secrets).
- **Passphrase change**: unlock (or recovery-unlock), new salt, new KEK_pw, new nonce, new `pw_wrap`, optionally new KDF params, `header_gen += 1`, recompute MAC, atomic replace; sync the new header. DEK and all objects unchanged. Old header copies (backups, provider history) still open with the old passphrase; document it.
- **Recovery flow**: decode RK, verify checksum, derive KEK_rk, unwrap, verify MAC; force setting a new passphrase (header rewrite as above). Offer rotating RK: new RK, new `rk_wrap`, same DEK.
- **Recovery key rotation**: as above, `header_gen += 1`. Old RK remains valid against old header copies.
- DEK rotation and revocation are out of scope; the header version and `alg_id` leave room.

## 9. Format evolution rules

1. Every byte-level change bumps `format_version` (header) or `format_version` (envelope); the two are independent but the vault header version gates which envelope versions may exist.
2. Readers must reject unknown versions, unknown `alg_id`, unknown `obj_type`, and non-zero `flags`; never ignore-and-continue. No silent downgrade or algorithm fallback.
3. Within a version only additive CBOR payload keys are allowed (unknown keys ignored, never required).
4. Writers always write the latest version they support; migration is lazy (rewrite on next edit) except where a security fix requires an eager pass, announced in release notes.
5. An older client opening a newer vault refuses with "update Nuvoletta" and must not modify any file.
6. `header_gen` is monotonic; a device records the highest it has seen and warns on a lower header.
7. New AEAD or KDF ids are added as new numeric ids; deprecated ids stay readable only for migration.
8. Format changes require an ADR and updated test vectors.

## 10. Rust crates

| Need | Crate | Rationale |
|------|-------|-----------|
| Argon2id | `argon2` (RustCrypto) | Pure Rust, RFC 9106 vectors, no_std capable. |
| AEAD | `chacha20poly1305` (`XChaCha20Poly1305`) | Audited-by-ecosystem RustCrypto, constant-time, widely used. |
| KDF | `hkdf`, `sha2`, `hmac` | Standard RFC 5869 and RFC 2104 on SHA-256. |
| Zeroization | `zeroize` (with `ZeroizeOnDrop`) | Wipes keys and plaintext buffers; best effort only. |
| RNG | `getrandom` (via `rand_core::OsRng`) | Direct OS CSPRNG; no userland PRNG. |
| Constant-time compare | `subtle` | MAC and checksum comparisons. |
| CBOR | `minicbor` | Small, strict, no-serde. |
| Base32 | own ~40-line Crockford codec with tests | Avoids a dependency for secret material; fuzz-tested. |

Keys live in `Zeroizing<[u8;32]>` types that do not implement `Debug`/`Clone`/`Serialize`. `unsafe` is forbidden in `nuvoletta-crypto`. Dependencies pinned with `Cargo.lock` and reviewed on update.

## 11. Test vectors and test plan

- Vectors committed under `crates/nuvoletta-crypto/tests/vectors/` as JSON plus binary golden files, generated by a reference script and verified by an independent implementation (e.g. Python `argon2-cffi` + `cryptography`/`pynacl`) to catch single-implementation mistakes.
- Cases: Argon2id with low parameters (fast in CI) and one full-cost vector; HKDF subkeys for a fixed DEK/vault_id; full header (both wraps, MAC); envelope for each obj_type; Padmé length table for L in [1, 2^20]; recovery key encode/decode including typo, wrong checksum, lowercase and grouping variants.
- Deterministic RNG injection (test-only feature) to make fixtures reproducible; production code cannot select a non-OS RNG.
- Negative suite: flip every header byte and every 64th ciphertext byte, truncate at every offset, wrong passphrase, wrong RK, swapped wraps, header params tampering, object copied between vaults or between scope_ids, non-zero padding, oversized params.
- Property and fuzz tests: parse(serialize(x)) = x; arbitrary bytes never panic or allocate beyond the limits; nonce uniqueness over N generated encryptions.
- **Decided:** a one-time full-cost (64 MiB) Argon2 vector is committed and run in a nightly CI job on all platforms including the Android emulator.
