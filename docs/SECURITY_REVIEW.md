# Internal security review (AI-assisted)

!!! warning "This is not an independent audit"
    This page summarises a code review done by an AI assistant (Claude) working from the
    repository, with small proof-of-concept tests, and the fixes made in response. It is **one
    reviewer's reading, time-boxed, with no fuzzing campaign beyond the repository's own targets, no
    testing against real WebDAV servers, sync tools or devices, and no supply-chain review**. It does
    not show that Nuvoletta is secure. Nuvoletta has not been independently audited (see
    [threat model](THREAT_MODEL.md), N9) and must not be described as audited. An independent human
    review remains a Phase 7 milestone before relying on it for highly sensitive material.

Scope: the state of `main` at commit `c7e15e8` (Phase 7, part B). No critical or high findings were
reported; the cryptographic core (header, key derivation, envelope, AAD, padding, recovery key) was
read against `CRYPTO_FORMAT.md` and matched it. The medium findings were about *detecting*
tampering and about conflict handling, not about confidentiality. No on-disk or on-wire format
changed while addressing the findings.

Status words: **fixed** (code change with a regression test), **documented** (a limit that is now
written down), **deferred** (not done; reason given).

## Medium

| Id | Finding | Status |
|----|---------|--------|
| M1 | One unauthenticated file under a segment name suppressed the rollback warning | Fixed: only verified segments count toward a device's remote maximum |
| M2 | Unauthenticated remote `h/` files raised "passphrase changed" and poisoned the remembered header generation | Fixed: a remote header counts only after its MAC verifies with this vault's keys |
| M3 | Typing during a cycle that updated the open note let autosave silently replace the remote edit | Fixed: the buffer is saved as a sibling of the revision it was based on, so it shows as a conflict |
| M4 | A checkpoint claiming `seen` sequence 0 panicked every device's sync cycle | Fixed: such entries are invalid and the claim checks cannot panic |

## Low

| Id | Finding | Status |
|----|---------|--------|
| L1 | A newest segment that never authenticates was never reported | Fixed: persistent unreadable segments are reported |
| L2 | Deleting the same note id twice overwrote the older copy in the local trash | Fixed: the older copy is kept as "(earlier version)"; identical copies are not duplicated |
| L3 | No lock on a vault: two instances could lose an edit | Fixed: advisory OS lock on `.nuvoletta/lock`, second opener gets "already open" (see `STORAGE_FORMAT.md` section 5) |
| L4 | Export did not refuse the configured sync folder | Fixed: destinations inside or containing the sync folder are refused, also while sync is off. A folder synced by a tool Nuvoletta does not know about is still the user's responsibility |
| L5 | The WebDAV permission probe does not cover sub-folders created later, nor folder-level delete or move | Documented (`WEBDAV.md`, threat model T25). Not fixed: a proper probe needs to create a folder the account owns, which leaves folders that a create-only account can never remove, and needs testing against real servers with per-folder rules. Planned improvement |
| L6 | Symlink check and open were separate steps (race) | Fixed: files are opened with `O_NOFOLLOW` first on Unix and the handle is inspected |
| L7 | `sync-notices.json` and `sync.json` are unauthenticated | Partly fixed: the notices are sealed with the device key and a replaced file is reported. Deleting the sealed file, and the plaintext `sync.json` (folder choice), are documented limits (threat model T24) |

## Info

| Id | Finding | Status |
|----|---------|--------|
| I1 | The "passphrase changed elsewhere" message told users to do something that does not work | Fixed: the message now describes the manual header copy that does work. Adopting a newer header in the app is a separate follow-up, not done |
| I2 | Stale Android and "no network connections" text | Fixed in `APP.md`, the manifest comment, ADR 0006 and the privacy policy |
| I3 | Android lock waits for a running WebDAV cycle | Deferred: needs cancellation support in the sync worker |
| I4 | CSP residuals (`unsafe-inline`, `unsafe-eval`, loopback WebSocket) | Accepted; already documented in ADR 0004. Keep the renderer fuzzed |
| I5 | Integer overflow on a hostile `getlastmodified` year | Fixed |
| I6 | `seq + 1` overflow at `u64::MAX` | Fixed |
| I7 | Spec items not implemented (`mlock`, cross-implementation vectors, snapshots, revocation) | Already documented; unchanged |
| I8 | Trash retention trusts the local clock | Documented in `STORAGE_FORMAT.md` section 3.1 |
| I9 | Secret copies outside zeroizing buffers | Accepted; the zeroization limit is already documented |

## Not covered

Everything in the review's "limits" section still applies: not every line was read with equal
care, no real-world server, sync-tool or device testing, dependencies assumed correct, memory
residue not measured. Adopting a newer vault header and in-app device join are separate, unapproved
follow-ups and are not part of these fixes.
