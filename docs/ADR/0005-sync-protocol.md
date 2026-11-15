# ADR 0005: Sync protocol over a dumb shared store

Status: Accepted (Phase 4; protocol `docs/SYNC_PROTOCOL.md` rev 2 approved by the owner)
Date: 2026-11-15

## Context

The first sync backend is a folder kept in sync by an external tool (Syncthing, Dropbox, a NAS
mount), later WebDAV. Such stores have no atomicity across files, no ordering, delayed and partial
propagation, no conditional writes and, with sync tools, "conflicted copy" files. The provider is
untrusted: it sees only ciphertext and opaque names (PLAN.md section 10, `THREAT_MODEL.md`). The
protocol must never silently lose an edit and must tolerate hostile or broken files.

## Decision

1. **Single-writer immutable files.** Every remote file is written by exactly one device and never
   overwritten: revision objects (`o/`), per-device log segments (`d/<device>/l/`) and vault header
   generations (`h/`). The provider interface is therefore only `list`, `get`, `put_new` and
   `delete`, with an explicit capability model (`create_only`, `atomic_rename`, `etags`).
2. **Per-device hash-chained logs.** A device changes shared state only by uploading an immutable
   object and then appending an entry to its own log (segments sealed with the log key). Each entry
   commits to the previous one, so removed or reordered history is detectable.
3. **Multi-head conflict DAG.** Every revision names its parents; the set of verified entries from
   all devices forms a DAG per note. Several heads are a conflict, computed identically on every
   device from the entries alone, never from time stamps or file names. Only metadata-versus-body
   edits are merged automatically (3-way, disjoint fields); everything else stays a visible
   conflict until the user picks a head, keeps both or resolves delete-versus-edit.
4. **Detect and freeze forks.** A device ID used twice (cloned directory, restored backup, sync-tool
   copy) is surfaced and the stream is frozen at the fork point for every reader; the forked device
   regenerates its ID and re-emits its revisions. No file of either branch is deleted.
5. **Readers tolerate everything.** Only strictly named files are protocol files; stray files are
   listed and ignored; authenticated copies are accepted for objects and report-only for
   segments; a file that fails authentication is "not yet arrived".
6. **Local state is rebuildable.** Everything under `.nuvoletta/sync-state/` is encrypted (type 4)
   and can be rebuilt from the remote logs, except the device ID and the unsent journal.

## Consequences

- Positive: works on any store that can list, read, create and delete files; no overwrite races;
  no data loss by construction; conflicts are deterministic state, not extra files; the engine is
  testable with an in-process fault-injecting folder.
- Negative: logs and objects grow until a manual garbage collection; conflicts need user action
  except for disjoint metadata edits; a device offline forever can stall automatic merges of
  identical-content heads (documented in the implementation notes).
- Stated limits (`SYNC_PROTOCOL.md` section 10.3): a provider can roll back consistently, hide the
  newest entries of a device not yet observed, show split views until devices exchange
  checkpoints, and any enrolled device can forge entries of another (all devices share the data
  key; per-device signing keys come with key rotation and revocation work).

## Alternatives considered

- CRDT text merge per note: strong convergence, but a large format and dependency commitment and
  needs a trustworthy operation history; text-level merging is post-MVP.
- Last-writer-wins by modification time: silently loses edits and trusts clocks; rejected.
- One shared append-only log or a manifest file: needs atomic append or compare-and-swap, which
  dumb stores lack.
- Merging forked branches instead of freezing: more complex, and different devices could disagree
  on the graph; detect-and-freeze with re-emission is simple and loses nothing.
