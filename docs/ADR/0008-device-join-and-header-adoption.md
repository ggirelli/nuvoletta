# ADR 0008: In-app device join and vault header adoption

Status: Proposed (2026-11; needs owner approval before any code)
Date: 2026-11-17

## Context

Recovery testing (`crates/nuvoletta-app-core/tests/recovery.rs`, PLAN.md Phase 7) found two gaps that
both involve the vault header and therefore key handling:

1. **Join.** A new device (or a restored one with no vault) has no vault directory. Today the user
   copies the newest file of the sync folder's `h/` to `.nuvoletta/vault-header` by hand, then unlocks
   and configures sync.
2. **Adoption.** After a passphrase (or recovery key) change on device A, device B sees a newer
   header in `h/` (PR #30, finding M2: a remote header is counted only if its MAC verifies with this
   vault's keys) but only warns. The user copies the file by hand.

Facts this design relies on (all existing, none changed here):

- The header is 236 bytes, fixed layout (`CRYPTO_FORMAT.md` section 2). The DEK never changes; a
  passphrase change writes a new `salt`, `pw_nonce`, `pw_wrap`, `header_gen + 1` and a new
  `header_mac`. A recovery-key rotation also bumps `header_gen`.
- `pw_wrap` AAD covers `bytes[0..60]` (version, generation, vault id, KDF params, salt), so KDF
  parameters are authenticated before the DEK is trusted. `header_mac` is keyed by `K_header`, which is
  derived from the DEK, so it is only checkable after a successful unwrap. Parsing enforces the KDF
  ranges (`m_kib` 8192..=1048576, `t` 1..=16, `p` 1..=8) before Argon2 runs.
- Remote headers are immutable per-generation files `h/<gen 8 hex>.nvhdr` (`SYNC_PROTOCOL.md`
  section 2). The file name is a hint; authority is the parsed `header_gen`.
- A device records the highest MAC-verified generation in the K_local-sealed `verified-header-gen`
  file. It does not exist on a device that has not joined yet.
- Per-device logs, objects and `peers`/`notes` state are all sealed with keys derived from the DEK,
  which does not change, so neither flow needs to touch them.

Constraint from the owner: no new cryptography and no on-disk or on-wire format change. Where one
seems useful it is listed as an open question, not decided.

## Decision

### A. Common rule: a header is never trusted until a user secret opened it

A header read from a provider is untrusted bytes. It is accepted only if **all** hold:

1. Exact length, magic, supported `format_version`, `header_gen >= 1`, KDF parameters in range
   (existing parser; runs before Argon2).
2. `vault_id` equals the local vault's (adoption) or is the one the user confirmed (join, see B).
3. The secret the user typed unwraps the DEK (`pw_wrap` with its AAD, or `rk_wrap`).
4. `header_mac` verifies under `K_header` derived from that DEK.
5. Adoption only: the unwrapped DEK equals the DEK already held by this device (constant-time
   comparison). This is the strongest binding to "the same vault": a forged header cannot carry a
   different DEK, because the device's own keys would no longer match its notes.
6. Generation rule (below).

Nothing is written to the vault directory before every check passed. Failure shows the same generic
message as a wrong passphrase (existing unlock-error policy); detail goes to a log without secrets.

### B. Join (new device)

**Entry point.** In the first-run screen, next to "Create vault": "Join an existing vault". The user
picks the sync location (folder, or WebDAV address and credentials, which already run the
create-only credential check). Android picks a folder through the existing flow.

**What is read.** Only `list("h/")` and `get` of header files. For WebDAV and a folder this is the
same provider trait calls the engine already uses (no new provider capability). Files are read with
the existing size cap (236 bytes; anything else is ignored). No log, object or note is read before
the vault is unlocked.

**Choosing the header.**
- Parse every candidate; drop malformed ones and those whose name generation disagrees with the
  parsed one (kept as a diagnostic).
- Group by `vault_id`. More than one group: the user must pick, showing the vault fingerprint (below);
  this means a shared folder holds two vaults or an attacker added one.
- Within the chosen group, try the **highest** generation first. A header whose name and content
  disagree or whose MAC fails is skipped with a notice; the next generation is not silently used
  (see rollback below), the user is told and may choose "use an older header" explicitly.

**What the user types.** The passphrase of the *current* header (the one the user last set), or the
recovery key. Unlock derives KEK, unwraps, checks the MAC (rule A.1 to A.4). A recovery-key join
forces setting a new passphrase afterwards (existing recovery flow, new header generation, published
to `h/`), because the joining device is then the one that changed it.

**Vault fingerprint.** After a successful unwrap, show a short, human-comparable fingerprint of
`vault_id` (first 10 bytes of `SHA-256("nuvoletta vault id" || vault_id)` as grouped hex) and the
generation. The user can compare it with the one shown in the settings of an existing device. This
defends against joining the wrong vault in a folder shared by several, and against a provider
substituting a vault the attacker knows the passphrase of (only possible if the attacker knows the
passphrase, so it is a defence in depth against mistakes more than against crypto attacks). No new
primitive: it is a display of existing data. Whether `SHA-256` over a label is acceptable as "not
new cryptography" is an open question (O3).

**What is stored locally.** In order, each step atomic (`write_atomic`):
1. create the vault directory layout (`.nuvoletta/`, existing `Vault` constructor for an existing
   header);
2. write `.nuvoletta/vault-header` (the exact bytes that verified);
3. unlock, then write `device-state` sync settings (folder or sealed WebDAV credentials, existing
   code) and `verified-header-gen = <generation>` (the MAC-verified generation);
4. let the normal first sync run: it generates a fresh `device-id` (existing behaviour at first run),
   builds `peers`/`notes` from the remote logs and downloads notes.

If the process dies between 2 and 3 the directory is a valid vault without sync settings; the user
repeats join or configures sync normally. No state is half-trusted.

**Interaction with logs.** None: the join device has no log. Its first sync creates its own
`d/<device_id>/`. The DEK is the same, so all existing logs and objects authenticate. The first sync
verifies every foreign log from sequence 1 as for any new device. If the logs on the provider do not
open with this DEK (rollback to another vault, wrong group), the sync fails with the existing
verification error and the vault directory can be removed by the app ("Cancel join", only before any
local note exists).

**Downgrade and rollback at join.** A new device has no memory, so a provider (X3) can serve any
*genuine* old header: it still unwraps with the old passphrase and passes the MAC. This is
unpreventable without remembered state and is the existing residual risk of T7. Mitigations that need
no format change: (a) always pick the highest generation present; (b) show the generation and ask the
user to compare it with an existing device's generation (displayed next to the fingerprint); (c) after
the first sync, `peers`/checkpoint data from other devices is checked as usual, and a header older
than one the logs imply is not detectable (logs carry no header generation: O2). After a
passphrase change made because the old one was compromised, a rolled-back header makes the *old*
passphrase work on a new device; the user is told this in the join screen text.

### C. Header adoption (existing device, locked or unlocked)

**Trigger.** The sync cycle already compares remote generations with `verified-header-gen` and
reports `HeaderNewer` (after the MAC check, M2). Adoption turns that notice into an action:
"Passphrase or recovery key changed on another device. Enter the new passphrase to use it here."

**What is read.** The remote header files for generations greater than the local one (already
fetched by the sync cycle). The highest candidate that parses is offered.

**What the user types.** The new passphrase (or the new recovery key). Only the unlocked case is
supported by default: the device is unlocked with the old passphrase, holds the DEK in memory, and so
can check rule A.5 and re-encrypt nothing. Locked case: see O4.

**Checks (all of A, plus):**
- `header_gen` strictly greater than the local header's and `>= verified-header-gen`. Equal or lower
  is "already current / older": refused, shown as `HeaderRollback` if lower than seen before.
- Same `vault_id`; same unwrapped DEK (A.5).
- **KDF downgrade.** If the new `m_kib`, `t` or `p` is lower than the local header's, or below the
  vault-creation default (64 MiB, t=3, p=1), the app shows "this header makes brute-forcing the
  passphrase cheaper" and requires an explicit confirmation. The parameters are authenticated, so this
  can only come from a device that held the DEK (a deliberate user choice on another device, or a
  compromised one); it is warned, not forbidden. Upper bounds are the parser's (DoS guard); in
  addition the adoption flow refuses a cost above what this device can compute within a bound
  (O5), because unwrapping a header whose cost exceeds device memory fails otherwise.
- Skipping generations is allowed (A may have changed the passphrase twice): the jump size is shown,
  not limited; each intermediate header is immutable and not needed.

**What is stored locally.** In order: write the new header to `.nuvoletta/vault-header` with
`write_atomic` (a crash leaves either the old or the new file; both open the vault: the old header
with the old passphrase, the new one with the new), then update `verified-header-gen`, then clear the
`HeaderNewer` notice. The in-memory session keeps the DEK; K_local and every other derived key are
unchanged because the DEK is unchanged, so no state file is re-sealed.

**Forged or older header refusal.** An attacker without the DEK cannot produce a header that passes
A.3 to A.5. An attacker with a genuine old header can replay it: it fails the generation check (older
than local), is reported as `HeaderRollback`, and is never written. An attacker who forges the file
*name* of a high generation is ignored as in M2: the generation counts only from the MAC-verified
content. A header for another vault fails A.2. Two different valid headers of the same generation
(both MAC-verify: only possible if two devices changed the passphrase concurrently, or the
attacker holds the DEK) are a **diverging header**: the existing `HeaderInvalid` path; the app shows
both generations' fingerprints and asks the user to pick (O6), never choosing silently.

**Concurrent changes.** If device A and device C both change the passphrase from generation 4, they
create two different `h/00000005.nvhdr` files. Header files are create-only (`put_new`), so the
second writer finds the name taken, fetches the winner, and shows "another device changed the
passphrase at the same time": it must adopt the winner first and then change again from there. This
is existing publication behaviour; the ADR only requires the app to surface it (O6 asks whether that
is already the case).

### D. UI flow

Join: Start screen, "Join an existing vault", sync location, "Looking for the vault header",
vault choice if several, passphrase or recovery key, confirm the fingerprint and generation, then the
normal first sync progress. Adoption: sync view notice (`header-newer`) gains a button, "Use the new
passphrase", opening a modal with one password field, the generation, and the KDF warning if it
applies. Both screens never show the typed secret, zeroize it after use, and report failure with the
generic unlock error. The user guide's manual steps move to a "if the app cannot do it" appendix.

### E. Threat-model deltas

- **T7 (stale header)**: adoption closes the loop that made old-passphrase-forever possible on a
  device that missed the change. Residual risk unchanged for a *new* device (join cannot detect a
  rolled-back genuine header). Add: join shows generation and fingerprint for comparison.
- **T1 (offline guessing)**: unchanged; the header is already in the folder. A join screen that tells
  users the header exposes their passphrase to guessing is helpful, not new.
- **New: forged header offered to a joining device (X3)**: cannot succeed without the passphrase (A.3,
  A.4). A malicious provider can make joining fail (denial of service); not preventable.
- **New: KDF downgrade through adoption**: authenticated; requires a DEK holder; user confirmation.
- **New: shoulder-surfing or malware reading the typed secret**: same as unlock; not worse.
- **Wrong vault in a shared folder**: fingerprint comparison and the vault choice step.

## Consequences

- Both gaps close without any change to on-disk or on-wire formats and without new cryptography:
  the flows use the existing parser, Argon2id, wraps, MAC and subkey derivation.
- Join needs a new first-run screen on desktop and Android and a reduced provider read path
  (`h/` only) before the vault exists; adoption needs one action in the sync view.
- App-core gains: `join_vault(provider, passphrase|recovery_key)` (read, verify, write header,
  settings), `adopt_header(passphrase|recovery_key)` and a fingerprint helper. The crypto crate gains
  at most a function that verifies a candidate header against an existing DEK (a comparison of
  existing outputs).
- Tests required (regression and negative): forged name, wrong vault, wrong passphrase, same-generation
  divergence, older header, KDF downgrade, crash between steps (each step order above), join then
  sync equals the recovery test's manual flow.

## Alternatives considered

- **Keep the manual copy**: no code risk, but error-prone (copying the wrong file silently weakens
  the vault) and already the cause of the recovery-test gap.
- **Adopt automatically without a secret**: impossible; the new header needs the new passphrase to
  unwrap, and adopting without checking the MAC would reopen M2.
- **Store a header fingerprint on the provider outside `h/`**: a new on-wire object; rejected here.

## Open questions for the owner

- **O1. Locked-device adoption.** If B was never unlocked after the change and the user only
  remembers the new passphrase, the old header cannot open the vault, so rule A.5 cannot compare the
  DEK. Option: unlock with the candidate remote header, then verify the DEK by opening one local
  K_local-sealed file or note envelope (existing data), and only then replace the header. Accept?
- **O2. Header generation in the logs (format change).** Recording `header_gen` in checkpoints would
  let a new device detect a rolled-back header after its first sync. That is a wire-format change
  (breaking, `!`) and is **not** proposed unless the owner wants it.
- **O3. Fingerprint construction.** Is a truncated SHA-256 of `vault_id` with a label acceptable, or
  should the fingerprint be the raw `vault_id` in grouped hex (nothing new, longer to compare)?
- **O4. Join with the recovery key.** Forcing a new passphrase right away publishes a new header
  generation from a brand-new device before it has synced. Is that acceptable, or should it defer
  until after the first successful sync?
- **O5. Upper bound on KDF cost at adoption/join.** The parser allows 1 GiB and t up to 16; a phone
  may not manage it. Propose: refuse above 256 MiB on Android with a clear message. Pick the numbers.
- **O6. Diverging same-generation headers.** Present both and let the user choose, or refuse and
  require another device to bump the generation? Also confirm that the app already surfaces a lost
  publication race on `h/<gen>` (to be verified in code before implementation).
- **O7. Cancelling a join.** Remove the empty vault directory automatically on failure, or leave it
  for the user? (Proposed: remove only while it holds no notes.)
