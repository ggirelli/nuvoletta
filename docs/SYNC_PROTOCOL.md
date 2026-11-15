# Nuvoletta Sync Protocol (v1)

Status: Accepted (rev 2). Implemented in `nuvoletta-sync` (Phase 4); the decision is recorded in `ADR/0005-sync-protocol.md`; implementation choices for points the text leaves open are listed in the Implementation notes at the end

Goal: converge devices through a "dumb" shared store (externally synced folder first, WebDAV second) with no atomicity across files, no ordering, delayed or partial propagation, and no conditional writes. Envelope and key details are in `CRYPTO_FORMAT.md`; threats in `THREAT_MODEL.md`.

## 1. Principles

1. Single writer per file: every remote file is written by exactly one device and is **immutable** after creation. No file is ever overwritten by the protocol. This removes write-write races.
2. Names are opaque and random; authority is the authenticated header inside the file, never the file name or modification time.
3. A device changes the shared state only by (a) uploading an immutable content object and then (b) appending an entry to its **own** log.
4. Readers tolerate anything: missing, truncated, duplicated, reordered, stray or malicious files. A bad file is "not yet arrived" or "invalid", never fatal and never deletes data.
5. Conflict state is a deterministic function of the set of verified log entries, so all devices compute the same conflicts without coordination.
6. Provider interface needs only: `list(prefix) -> [(name, size, etag?, mtime?)]`, `get(name)`, `put_new(name, bytes)` (create-only if capability `create_only` is true, otherwise best effort), `delete(name)`.

## 2. Remote layout

```
<sync-root>/
  h/<header_gen 8 hex>.nvhdr              vault header, one immutable file per generation
  d/<device_id 32 hex>/l/<first_seq 16 hex>.nvlog   log segments (envelope type 2)
  o/<object_id[0..2] hex>/<object_id 32 hex>.nvobj  note revision objects (envelope type 1)
  s/<device_id 32 hex>/<object_id 32 hex>.nvsnap    snapshots (reserved, post-MVP)
```

- `object_id` and `device_id` are 128-bit random. Names reveal only counts, device count and rough timing.
**Decided:** `o/` is sharded by first byte (256 directories) to keep listings small on WebDAV/NAS and mobile sync tools.
- Temporary upload names start with `.tmp-` and are never read by peers; the writer renames to the final name after a complete write (folder backend). On WebDAV use `PUT` to a tmp name then `MOVE`, or `If-None-Match: *` when supported.
- Header rewrite (passphrase change) creates `h/<gen+1>.nvhdr`; devices use the highest valid generation; the writer may delete older generations after a grace period.
- Only names matching the strict grammar `^[0-9a-f]{N}\.(nvhdr|nvlog|nvobj|nvsnap)$` are protocol files; see section 8 for the rest.

## 3. Identities

- `device_id`: 128-bit random generated at first run of a vault on a device, stored locally (`sync-state/device-id`). Cloning a vault directory to a second device without regenerating the ID creates a fork (section 10.4); the app must regenerate on "restore from backup / copy" and detect it by comparing own remote log against local journal.
- `note_id`: 128-bit random, stable for a note's life.
- `object_id`: 128-bit random per note revision (the immutable "version id" of that content).
- `tombstone_id`: 128-bit random per DELETE, drawn from the same space as `object_id`. It names the deletion as a vertex of the version DAG (section 5.3) so it can be a parent; no object file exists for it.
- In this document a **revision id** is either an `object_id` (PUT) or a `tombstone_id` (DELETE).
- Vault-wide logical identity is `vault_id` (header).

## 4. Content objects

An object is one envelope (type 1) containing a complete note revision (title, path, tags, timestamps, body). Header `scope_id = note_id`, `object_id` = revision id. Because it is complete and immutable, "uploading a revision" is `put_new(o/xx/<object_id>.nvobj)`. The local vault stores the latest revision of each note as the same envelope bytes, so upload is a file copy, and `ct_hash = SHA-256(envelope bytes)` is computed once.

An object is **referenced** only when a log entry names it. Unreferenced objects (orphans from crashed uploads) are ignored and later garbage collected (section 9).

## 5. Per-device append-only log

### 5.1 Segments

A log is a sequence of immutable segment files because plain files cannot be safely appended through dumb sync layers. A device writes a new segment per sync flush containing 1..n new entries. Segment envelope: type 2, `scope_id = device_id`, `object_id` random, name `<first_seq>`. Segment payload is a CBOR array of entries.

### 5.2 Entry (CBOR map, inside the AEAD)

| Key | Field |
|-----|-------|
| 0 | seq: u64, starts at 1, +1 per entry, contiguous across segments |
| 1 | prev: bstr(32), hash of the previous entry (32 zero bytes for seq 1) |
| 2 | kind: 1 PUT, 2 DELETE, 3 CHECKPOINT |
| 3 | time: i64 ms, informational only, never used for ordering or resolution |
| 4 | note_id: bstr(16) (PUT, DELETE) |
| 5 | rev_id: bstr(16): `object_id` (PUT) or `tombstone_id` (DELETE) |
| 6 | ct_hash: bstr(32) SHA-256 of the object file (PUT) |
| 7 | parents: array of bstr(16): the revision ids (`object_id` or `tombstone_id`) this change is based on (PUT, DELETE); empty for a new note |
| 8 | seen: map device_id -> [seq, entry_hash] (CHECKPOINT; may be attached to any entry) |

- `entry_hash = SHA-256("nuvoletta/v1/log-entry" ‖ prev ‖ bytes of the entry's canonical CBOR encoding as stored)`. Chain = each entry commits to all earlier ones; removing or reordering history of a device is detectable by anyone who remembered a later hash.
- Authentication and confidentiality: the segment is AEAD-protected with the log subkey K_log (note ids, object ids and the version graph are not visible to the provider). AAD binds vault, type, device scope, segment id (`CRYPTO_FORMAT.md` section 4).
- A reader verifies, per segment: envelope valid; `scope_id` equals the directory's device_id; entries' seq contiguous starting at the file's `first_seq`; first `prev` equals the stored hash of seq-1 (or zero); each `ct_hash`/`note_id`/`rev_id` well-formed; PUT has `ct_hash`, DELETE has none.
- **Decided:** All devices share the DEK, so any DEK holder can forge log entries claiming another device (N5). Accepted for MVP and documented; per-device Ed25519 signing keys come with DEK rotation/revocation work.

### 5.3 Versions and the version DAG

Every PUT introduces revision `object_id` with `parents` = revisions it was derived from. DELETE introduces tombstone `tombstone_id` with `parents` = the revisions it removes. A `rev_id` is introduced by exactly one entry; a second entry introducing an already-known `rev_id` for a different `note_id`, or with different `parents`/`ct_hash`, is invalid and reported (an entry repeating an identical `note_id`, `parents` and `ct_hash` triple is accepted: this is how a forked device re-emits revisions, section 10.4). The set of verified entries from all devices forms a DAG per `note_id`. The **heads** of a note are revisions or tombstones that are not a parent of any other entry. One head = no conflict. More than one head = conflict (section 7). Parent ids never need the parents' content to be present.

## 6. Local state and the sync cycle

### 6.1 Local files (all under `.nuvoletta/sync-state/`, encrypted with K_local as envelope type 4, written atomically, all rebuildable except `device-id` and the unsent journal)

| File | Contents |
|------|----------|
| `device-id` | device_id (plaintext hex; not secret) |
| `journal` | own entries not yet confirmed uploaded (seq, entry bytes), next seq, last own entry_hash, and the **sealed bytes** of the segment being uploaded, if any (section 6.3 step 2) |
| `peers` | per device: highest verified seq, its entry_hash, last segment name, last_checkpoint seen, first/last time seen, and a `fork` record if one was detected (section 10.4) |
| `notes` | per note_id: `base` (revision id last merged and uploaded/applied locally), heads, `dirty` flag, conflict flag, local `ct_hash` |
| `inventory` | last remote listing (name, size, etag, seen_at) used to skip unchanged files |
| `pending` | referenced objects not yet fetched; segments buffered after a gap |
| `revs/<object_id>.nvobj` | local revision cache (see below) |
| `max-header-gen` | highest header_gen seen |

**Local revision cache.** The vault's `notes/` directory holds only the latest revision of each note (`STORAGE_FORMAT.md`), but merging needs older ones: the common ancestor for 7.2 and every head of a conflict for 7.3. `revs/` holds verbatim copies of type 1 object envelopes (the exact remote bytes, so `SHA-256(file) == ct_hash`; they are already encrypted with K_note and are not re-wrapped in type 4). Retained per note: the current `base`, every head, every direct parent of a head, and the last K revisions (section 9 rule 3). Other entries may be pruned locally at any time. Everything in `revs/` is a cache: a missing entry is re-fetched from the remote, and only if the remote copy was garbage collected does it degrade a merge to a visible conflict (section 9 rule 6).

Losing `peers`/`notes`/`inventory`/`revs` is recoverable by re-reading remote logs (devices re-verify chains from seq 1). Losing `journal` loses unsent edits only if the notes themselves are also lost; otherwise the next scan finds notes whose local revision is not in the own log and re-emits entries.

### 6.2 Local edit

On save: write the new revision envelope locally (atomic replace) with a fresh `object_id`; set `parents = [base]`; queue a PUT entry in the journal; set `dirty`. `base` stays unchanged until the revision is uploaded and referenced; later edits before upload may collapse into one revision (parents remain `[base]`; the intermediate local revisions are not uploaded).

### 6.3 Sync cycle (idempotent; safe to interrupt at any point)

1. **Push objects**: for each journal PUT, upload the object file if absent (`put_new`). Ensure the file is fully written before step 2.
2. **Append log**: seal one new segment with the journaled entries (plus a CHECKPOINT every rule in section 10.2) and persist the sealed bytes in the journal **before** uploading, so every retry uploads byte-identical content under the same name `<first_seq>`. Remove the entries from the journal only after `list` shows the segment present with the expected size and, optionally, `get` verifies it.
   - Retry after an interrupted upload: if `d/<own>/l/<first_seq>.nvlog` already exists, `get` it. Same bytes as the persisted segment: confirmed. A `.tmp-` leftover of our own: delete it and upload again. Different bytes that authenticate and carry the same entries (same `entry_hash` for every seq, e.g. sealed before the journal persisted the bytes): confirmed; keep the remote file and drop ours. Anything else that authenticates is a fork of our own device ID (section 10.4). A file that fails AEAD is "not yet arrived" (partial propagation); wait and retry, never overwrite it.
3. **List**: list `d/*/l/` and `o/` (use inventory to avoid refetching).
4. **Pull logs**: for each other device, fetch segments with `first_seq` above the stored position, verify (5.2), and apply entries in seq order. If a segment is missing (gap) or fails AEAD, stop that device's stream at the gap, record `pending`, retry next cycle. Truncated/partial files fail AEAD and are treated as not yet arrived; after N retries across M hours surface "possibly corrupted or withheld" in status.
5. **Pull objects**: for each applied PUT whose object is not local, fetch `o/..`, verify `SHA-256(file) == ct_hash` and envelope `object_id`, `scope_id == note_id`, then decrypt. Missing object: entry stays applied in the graph, the revision is "dangling" (not shown, user told), retried.
6. **Merge** (section 7) and write results to local notes atomically.
7. **Persist** state.

Applying others' entries never modifies local note content unless there is no local divergence (fast-forward) or an automatic merge is defined (7.2).

## 7. Merge and conflicts

### 7.1 Classification (per note, using heads and the local `base`)

| Situation | Result |
|-----------|--------|
| One head, equals local base | no change |
| One head H ≠ base, local not dirty | fast-forward: install H, `base = H` |
| One head H ≠ base, local dirty (local revision L with parents [base]) | L is itself a head (as a queued entry) alongside H: two heads, conflict |
| Two or more heads | conflict |
| Head is a tombstone and others exist | conflict (delete vs edit) |

Because local edits are PUT entries appended to the journal before any comparison, conflict is always "two or more heads in the DAG", identical on every device.

### 7.2 Automatic resolution (3-way, metadata only)

If exactly two PUT heads A and B share a common ancestor revision C available locally (retained, section 9), and they differ from C in disjoint field sets of {title, path, tags, body}, the device may write a merged PUT with `parents = [A, B]` and the union of changes. If both changed the body, or both changed the same metadata field to different values, there is no automatic merge. Concurrent merges by two devices produce two merge revisions that are themselves resolved the same way (merging identical content is idempotent: equal field values do not conflict). **Decided:** automatic merge only for metadata-vs-body (rename or tags vs body edit); no text-level 3-way merge in MVP.

### 7.3 Conflict presentation and resolution

- The conflict is state, not extra files: the note shows "N conflicting versions"; each head is viewable and diffable; the deterministic display order is by (`time`, `device_id`, `object_id`) and is cosmetic.
- Resolve by choosing a head (writes a PUT with `parents = all heads` and the chosen content), by editing a merge, or **keep both**: the resolving device creates a new `note_id` for one head content (title suffixed "(conflict copy)") and merges the original. Only the resolving device creates the copy, so devices do not each fabricate their own duplicate. **Decided:** multi-head conflict state with explicit user resolution and a "keep both" action.
- A tombstone head vs a PUT head: user picks "keep note" (PUT with parents = both) or "delete" (DELETE with parents = all).
- Nothing is discarded: every head object is retained until a resolving entry supersedes it (and after, per section 9).

### 7.4 Rename and move

Path and title are fields inside the revision, so rename is a normal PUT (one object). Path collisions (two notes with the same path) are not a sync conflict: the app shows both and disambiguates by display; `note_id` is the identity.

## 8. Foreign and sync-tool files

- Only files matching the strict name grammar are protocol files. Others (`name (conflicted copy)`, `.sync-conflict-20240101-...`, `name (1).nvlog`, `*.tmp`, `*.part`, `.DS_Store`) are listed as "stray" in a diagnostics screen and ignored; never read as truth, never deleted.
- Exception: a stray file inside a protocol directory whose name starts with the id the grammar requires there (8 hex in `h/`, 16 hex `first_seq` in `d/<device_id>/l/`, 32 hex in `o/xx/` and `s/<device_id>/`) followed by a non-hex character, whose extension is the one expected there, and whose content authenticates (envelope valid under the vault key, vault_id matches) is treated as a copy. Because identity comes from the authenticated header, an identical copy is deduplicated by `object_id`; for a segment the identity is the authenticated `(scope_id = device_id, seq of its first entry)`, never the name; a copy with the **same** `(device_id, first_seq)` but different entries (different `entry_hash` at some seq; differing bytes with identical entries are just a re-seal, section 6.3 step 2) is an **equivocation/fork** (section 10.4) and is surfaced, not applied. Rationale: sync tools create conflicted copies when two sides wrote the same path, which our single-writer scheme should never do except for clones or restores.
- **Decided:** authenticated stray copies are accepted for objects (content-addressed by `object_id` and `ct_hash`) and report-only for log segments.
- The local synced-folder backend must not follow symlinks out of the root, must reject names containing path separators or `..`, and must treat case-insensitive filesystems as equal names (all names are lowercase hex).

## 9. Garbage collection

Immutable objects and segments accumulate; deletion is the only destructive step, so it is deliberately conservative.

1. Only the **writer** deletes its own objects/segments (matches single-writer; avoids races). Segments live under the writer's `d/<device_id>/` directory. Objects live in the shared `o/` tree, so the writer of an object is the device whose log contains the PUT entry introducing its `object_id`; a device only deletes objects introduced by its own log (plus its own unreferenced orphans, rule 5, which it knows from its journal).
2. A revision R is collectable when all hold: (a) it is not a head and not a head's direct parent, (b) it is superseded by an entry at (device, seq) such that every device with a recent checkpoint (last seen within 90 days) has a recorded `seen` ≥ that position, and (c) a grace period (default 30 days) has passed since the superseder was written.
3. Keep at least the last K = 5 revisions per note (supports local history and 3-way bases).
4. Tombstoned notes: content objects follow rules 2 and 3 relative to the tombstone; tombstone entries stay in the log.
5. Orphans (uploaded but never referenced in the writer's log after 7 days): the writer may delete them.
6. Data-loss analysis: if a base C is collected, an offline device that edited from C merely loses automatic merge and sees a normal two-head conflict with both full revisions intact; no user content is lost. The risk of rule 2 is only degraded merge quality, not data loss.
7. **Decided:** defaults are a 90-day ack window, 30-day grace, K = 5 revisions kept; GC is manual ("Clean up old versions") in the first release.

## 10. Integrity, rollback and withholding

### 10.1 What is detected

- Modification, truncation, cross-object substitution: AEAD plus `ct_hash` and chain hashes.
- Dropped or reordered entries inside a log: seq gaps, chain mismatch against `peers.last_hash`.
- Provider serving a shorter or older log for a device than we have verified: `remote max seq < peers.seq` or a different hash at a known seq: flagged as rollback or fork; the device does not revert state and shows a warning.
- Own log shorter than local journal confirmed state, or foreign entries under our device_id: fork.
- Old header: `header_gen` lower than `max-header-gen`.

### 10.2 Cross-attestation

A CHECKPOINT (or `seen` attached to a PUT/DELETE) records for each known device the highest `(seq, entry_hash)` this device has verified. Any device that later sees B's checkpoint saying "I saw A up to seq 40" while A's log shows only 38 detects withholding or rollback of A, even if the provider hid A's tail from this device. **Decided:** write a checkpoint whenever the seen vector changed, at most hourly while editing, and always after a sync cycle that applied foreign entries.

### 10.3 Stated limits (cannot be fixed without a trusted external checkpoint)

a. A provider can consistently roll back all files to an earlier state that this device has not seen past; nothing contradicts it.
b. It can hide the newest entries of another device's log that this device has not observed (and, until a later checkpoint reveals them, no alarm is possible).
c. It can withhold the logs of a device this device has never seen.
d. It can show different devices different histories (split view) until they exchange checkpoints through a path the provider relays.
e. A compromised enrolled device can forge any device's entries (N5).
f. Optional mitigation, post-MVP: user-verifiable "state fingerprint" (hash of all peers' last hashes) comparable out-of-band between devices.

### 10.4 Fork handling

A fork (same `device_id` and seq with different `entry_hash`, from cloned vault directories or restored backups) is surfaced as "this device ID was used twice". **Decided (MVP):** forks are detected and frozen, not merged as separate branches:

1. `peers` stays keyed by `device_id`. On detection the reader records `fork = {seq: f, hashes: [...]}` for that device (f = first seq with two different verified entries) and stops applying that device's entries at seq f-1, for every branch. The rule depends only on the fork point, so every device that has seen both branches computes the same DAG. Entries at or after f that this device had already applied are removed from the DAG (it is recomputed from verified entries); local note content they installed is kept as local content, never deleted, and the affected notes are marked "needs attention".
2. No segment of either branch is deleted, by anyone. Their objects stay retained (they are referenced by authenticated entries) and remain listable in the diagnostics screen.
3. A device that finds a fork of **its own** device_id (own remote log disagrees with its journal or confirmed state, or section 6.3 step 2 finds a foreign segment under its name) stops writing to that log, generates a new device_id and re-emits, under the new log, PUT/DELETE entries for every local revision not reachable from the frozen prefix. Re-emission reuses the original `rev_id`, `ct_hash` and `parents`, so peers that installed that content see the same revision ids again and the notes return to normal (fast-forward or regular conflict). Each clone does this, so no edit made on any branch is lost.
4. Limit: a device that has only seen one branch keeps applying it until the other becomes visible (same class as split view, 10.3 d).

## 11. Compaction and snapshots

- Logs grow without bound. Estimated size: 150 B per entry before padding; 100k entries is about 15 MB per device across segments: acceptable for MVP.
- Reserved: a per-device snapshot object (type 3) holding the state of all note heads and the minimal DAG at vector V, after which the device may delete its own segments older than V once every active device's `seen` ≥ V (same rule as 9.2). New devices bootstrap from the latest snapshots plus later segments. The chain continues across the snapshot (`prev` = snapshot hash).
- **Decided:** snapshots and compaction are post-MVP; `obj_type 3`, the `s/` directory and the chain rule stay reserved. Small adjacent own segments are merged only manually.
- Local encrypted note history (MVP if cheap) is separate and local only (`K_local`).

## 12. Worked scenarios

Notation: `R0` is a revision id; `A#n` is device A's log entry n. Both devices start synced with `base(N) = R0`.

### S1. Two devices edit the same note offline

1. A (offline) edits body: R1, parents [R0], journal `PUT N R1`. B (offline) edits body: R2, parents [R0].
2. A reconnects: uploads R1, appends `A#7 PUT N R1 parents[R0]`. B reconnects: uploads R2, appends `B#4 PUT N R2 parents[R0]`.
3. Each pulls the other's log. Heads of N = {R1, R2}; both bodies changed so no automatic merge. Both show "2 versions". No file is lost; both objects retained.
4. The user on A picks "merge": editor shows both; saves R3 with parents [R1, R2]. B pulls `A#8 PUT N R3`; the single head is R3; B's base becomes R3. If B also resolved concurrently with R4 [R1, R2]: heads {R3, R4}; if bodies are identical the merge of equal content is deterministic: either device may write R5 [R3, R4]. The conflict never silently disappears.

### S2. Rename vs edit

1. A renames N: R1 (path changed, body same), parents [R0]. B edits body: R2, parents [R0].
2. Heads {R1, R2}. Common ancestor R0 is retained; changed fields are disjoint (path vs body), so the first device to merge (say A) writes R3 = path from R1 + body from R2, parents [R1, R2].
3. B pulls R3 and fast-forwards. Had R0 been garbage collected, the result would be a visible conflict, still without loss.

### S3. Delete vs edit

1. A deletes N: `A#9 DELETE N T1 parents[R0]` (T1 is the tombstone id). B edits: R1 parents [R0].
2. Heads {T1, R1}. This is a conflict: N is shown in a "needs attention" list, not deleted on B and not resurrected silently on A (A shows it as "deleted here, edited elsewhere").
3. Resolution "keep": PUT R2 parents [R1, T1]; all devices converge on R2. Resolution "delete": DELETE T2 with parents [R1, T1]; R1 stays retained through the grace period (section 9) so the user can undo.

### S4. Sync-tool "conflicted copy" files

1. Folder contains `d/<A>/l/0000000000000007.nvlog` and `d/<A>/l/0000000000000007 (conflicted copy).nvlog` (Syncthing/Dropbox made it because A was restored from backup and rewrote seq 7).
2. The second name fails the strict grammar: reported as stray. Its content authenticates and has `(device A, first_seq 7)` identical to the first file's identity but different bytes: fork (10.4) is shown with "device ID A was used twice"; neither segment is deleted.
3. If the bytes were identical (a pure duplicate), it is silently deduplicated. A conflicted copy of an object `o/ab/<id> (1).nvobj` with the same `object_id` and `ct_hash` is likewise ignored as duplicate.
4. A copy that fails AEAD (half-synced) is "not yet arrived"; it is re-read next cycle and never used.

### S5. Out-of-order propagation (partial)

A's segment `l/…0011.nvlog` (seq 11 to 12) arrives at B before `l/…0009.nvlog` (seq 9 to 10), and the object for entry 12 has not arrived. B verifies the segment (valid AEAD) but cannot link `prev`; it buffers it, applies nothing past seq 8, retries. When 9 to 10 arrives, 9 to 12 apply; the revision of entry 12 is dangling until the object arrives and `ct_hash` matches. The UI shows "syncing: 1 pending object" rather than an error.

## 13. Test plan hooks (Phase 4)

Two or more in-process vault instances over a fault-injecting folder backend (drops, reorders, delays, truncates, duplicates, renames to conflicted-copy names, rolls back); property tests: all devices reach identical heads from any delivery order; no revision lost; idempotent re-sync; crash at every step of the sync cycle.

## Implementation notes (nuvoletta-sync, Phase 4)

Places where the text above leaves a choice, is ambiguous, or the implementation is deliberately narrower. The safest reading was taken in each case; none changes a byte format of `CRYPTO_FORMAT.md`.

1. **Local edits are detected by scanning the vault**, not by hooking storage writes (section 6.2). A note whose stored revision id is not the one sync last saw is an edit; a synced note that disappeared is a delete; the scan runs at the start of every cycle and again before merging. Edits before the entry is sealed collapse into one revision (parents unchanged); only the intent that is the note's current local revision collapses, never re-emitted history. A note created and deleted before any entry was sealed leaves no entry and no object. A DELETE entry must carry at least one parent (otherwise it is reported invalid).
2. **A vault revision that is a known vertex is never a local edit.** Adopting it (instead of queueing a PUT) is what makes crash recovery and a lost `notes` file safe. An edit made while the sync state was lost has an unknown base, so it is queued as a root (empty parents): a visible two-head conflict, never a silent overwrite.
3. **Vault writes are two-phase.** The plan (`applying` per note) is persisted first, then the vault is written, then `local_rev` is updated. A planned write is never mistaken for a local edit or delete after a crash.
4. **Objects are fetched lazily**: heads and their direct parents (the retention set of section 6.1), plus a common ancestor on demand when a 3-way merge needs it, not every applied PUT.
5. **Checkpoints (10.2).** A standalone CHECKPOINT entry is written only when foreign PUT/DELETE entries were applied since the last one and the seen vector changed; `seen` is attached to the last PUT/DELETE of a segment when the vector changed and either foreign content was applied or the interval (default one hour, configurable) elapsed. Checkpoints never trigger checkpoints, so devices do not ping-pong.
6. **Automatic merge (7.2).** Two PUT heads merge when, field by field in {title, path, tags, body}, at most one side changed the field from the common ancestor or both changed it to the same value (so metadata-vs-metadata with disjoint fields merges too). A history with several maximal common ancestors (criss-cross) never merges automatically. Timestamps of the merge are `min(created_at)` and `max(modified_at)`, so concurrent merges write equal content. For two heads with **identical content** only the **designated device** (the smallest device id among the heads' introducing devices) writes the merge, otherwise two devices resolving concurrently could keep producing identical heads forever. If that device never syncs again the identical heads stay visible and the user resolves them.
7. **Forks (10.4).**
   - The fork point is the first sequence number at which two verified entries differ; when only a `prev` mismatch is visible it is the last sequence number both branches may share. Entries from it on are not applied and applied ones are removed from the graph.
   - If the diverging entries are our own **unpublished** sealed segment that lost the race for its name (create-only), nobody saw our branch: the other branch is simply the history of the old id (read like any other device, not frozen), and our entries are re-emitted under a new id. Only when both branches are visible on the remote is the id frozen for everyone.
   - A frozen stream's content that was never installed is never installed; content already installed stays and the note is "needs attention" until the entries reappear (re-emission) or the user resolves it. Nothing is ever removed from the vault or the remote.
   - If the own chain disagrees with the journal (state files out of step after a crash, or `peers` lost), the device continues under a new id; the old log is then read as a foreign device from sequence 1. Unconfirmed sealed entries become intents again.
8. **Segments after a gap are buffered** (section 6.3 step 4). A segment that authenticates, belongs to its directory and carries the right name but cannot be linked to the verified prefix is kept (its bytes as stored remotely, inside the K_local-sealed `pending` file) together with the size and etag of the listing it came from, and is not fetched again while the listing shows the same file. Segments are applied in order as soon as the gap fills, through the same verification as any fetched segment (chain, fork and validity checks run at that point, so a buffered segment that turns out invalid or forked is reported and dropped like any other). A buffered segment whose file is no longer listed, or whose device is frozen by a fork, is dropped. The buffer is bounded (32 MiB for all devices); over the bound a segment is simply fetched every cycle. `pending` is local and rebuildable: losing it costs a refetch.
9. **The `inventory` skips unchanged files** by name, size and etag (or modification time when the provider has no etags). A same-size, same-time substitution is therefore not re-read (an accepted limit; the chain check on every newly read file still applies).
10. **`peers` stores the full hash chain** of each device (32 bytes per entry, about 3 MB per 100k entries) so any later file can be compared entry by entry; this is the "re-verify chains" state of section 6.1.
11. **Rollback warnings** compare the highest visible sequence number per device with the verified one, for own log too (against the confirmed part); a delayed listing can therefore raise a transient warning.
12. **Header files** are published and observed (`h/`): a newer remote generation, an older maximum than seen before, and invalid or diverging headers are reported. Adopting a newer header needs the new passphrase and is an application action.
13. **Provider contract.** `list(prefix)` is recursive, returns regular files only (symlinks are omitted) and may include stray names; the engine classifies them. The folder backend creates files with a hard link from a `.tmp-` file (create-only), falling back to check-then-rename where hard links are unavailable; `create_only` is reported as true on Unix only.
14. **Not implemented**: snapshots and compaction (section 11), the hourly cap of standalone checkpoints, enrollment and revocation. The application UI (sync status, conflict screens, "Clean up old versions") is described in `APP.md`; it uses `conflicts()`, `note_status()`, `read_revision()`, `collect_garbage()` and the resolution actions. Resolving with hand-merged content is not an engine action: the app offers "use this version and edit", the edit syncs as an ordinary change.
15. **Folder backend and missing folders.** `list` of a missing directory is an empty listing, so an application must check that the sync folder exists before every cycle (an unmounted drive would otherwise look like a rolled-back remote). The desktop app does.
16. **Garbage collection (section 9)** is `Syncer::collect_garbage(vault, &GcOptions)`, manual only, with a dry-run mode and an injected clock (`now_ms`). Choices where the text leaves room, always toward keeping:
    - **Segments are never deleted.** Section 11 allows deleting own segments only after a snapshot, and snapshots are post-MVP. The report only counts them. Headers, stray files, copies and other devices' files are never touched.
    - **Which objects.** Only content objects whose every introducing entry is in this device's current log (a revision re-emitted after a fork has other origins and is kept). Tombstones have no object; their entries stay in the log.
    - **"Superseded".** A revision is superseded by its child entries. Collectable only if the newest child entry is older than the grace period (the entry's `time`, informational everywhere else; a forged old time can only bring forward deletion of this device's own objects that are anyway acknowledged) **and** every child entry has a sealed sequence number that each *active* device has acknowledged (`claims[writer] >= seq`).
    - **Active devices** are those whose log advanced, by this device's own clock, within the ack window (default 90 days). A device whose log has never been read here counts as active, and so does a device that only appears in someone's checkpoint: if it is unknown, nothing is assumed about it. A device with no checkpoint has acknowledged nothing.
    - **Never in conflict.** A note with two or more heads, or needing attention, keeps all its revisions. A tombstone head keeps its direct parents (they are head parents), so a deleted note keeps its last content and the last K revisions; purging deleted notes entirely is not implemented.
    - **Last K (default 5)** is the union of the K newest revisions of the note by entry time and every revision within K generations of a head.
    - **Refusals.** The collection does nothing while any device id is frozen by a fork (section 10.4 step 2: objects of both branches stay), and while the local state is being rebuilt. A stale local state is safe (more is kept), so the app does not force a sync first.
    - **Orphans (rule 5).** Orphans are only the own objects that this device remembers uploading and then abandoning (an intermediate local revision that collapsed into a later edit before any entry referenced it); the list is in `pending.orphans` with the time. A listed object that nobody knows is never an orphan (it may belong to a device whose log has not arrived). Orphans are deleted after 7 days, unless an entry references them by then (the record is dropped). If the record is lost the orphan leaks, which is safe.
    - **Failures.** Each deletion is independent; the first provider failure stops the run and reports the error. Everything deleted before it was collectable, and repeating the run continues. Nothing in the sync state depends on a collected object.
    - **Consequence (rule 6).** A device that edits from a revision that was collected meets a normal two-head conflict with both full revisions; automatic merge is not possible without the base.
17. **Revision cache pruning (section 6.1)** runs after every sync cycle and as part of a collection (including its dry-run count). It keeps per note the `base`, the local revision, every head, every direct parent of a head and the last K revisions, everything for a note in conflict or needing attention, every revision queued in the journal, and every file the state does not know. The rest is removed locally; it is re-fetched on demand if it is still on the remote.
