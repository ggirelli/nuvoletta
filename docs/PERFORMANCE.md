# Performance and battery

Phase 7 review. The numbers come from `just bench`, a set of `#[ignore]`d timing tests in
`crates/nuvoletta-app-core/tests/perf.rs`. They need no extra dependency, print one `bench:` line
per measurement and assert nothing about speed, so they cannot make CI flaky. `NUVOLETTA_BENCH_NOTES`
(default 5000) sets the vault size and `NUVOLETTA_BENCH_NEW` (default 1000) the number of notes in the
"sync of new notes" step.

## Method

* Notes are about 2 KiB of prose with two to three wikilinks, two tags and a three-level path. Ids and
  text are pseudo-random but deterministic.
* The vault is built through the storage API, then opened through `App` like the UI does.
* Argon2id uses the minimum accepted parameters so the benchmark measures the vault code, not the
  key derivation. A real unlock adds the KDF cost chosen at vault creation (a fixed cost, independent
  of the note count).
* Sync runs one device against a temporary folder, or against the in-process WebDAV test server (TLS
  on loopback, create-only account). The WebDAV figures therefore contain no network latency: on a
  real connection each request adds a round trip, so the number of requests matters more than these
  times. The test server also keeps its state in memory.
* Single run, release build, other work on the machine. Treat differences below about 20 % as noise.

Machine: AMD Ryzen 9 5950X (16 cores / 32 threads), 121 GiB RAM, Linux 7.0, rustc 1.100, `/tmp` on
tmpfs (so file system latency is lower than on a phone's flash storage).

## Results

| Measurement | 5 000 notes | 20 000 notes |
| --- | --- | --- |
| Storage unlock (decrypt every note) | 65 ms | 250 ms |
| App unlock (storage scan, second decrypt, index build) | 365 ms | 1.5 s |
| Note list / folder tree | 2 ms / 3 ms | 11 ms / 19 ms |
| Search, 20 queries of 1 to 2 terms | 11 ms | 43 ms |
| 50 backlink lookups | 0.2 ms | 0.2 ms |
| Unresolved links across the vault | 6 ms | 26 ms |
| Open a note and 20 edit-and-save cycles | 2 ms | 2 ms |
| Lock (drop keys and index, zeroize) | 31 ms | 147 ms |
| Folder sync, first push of every note | 0.36 s | 3.0 s |
| Folder sync, no changes | 62 ms | 0.3 to 0.7 s |
| Folder sync, 1 000 new notes | 0.16 s | 0.4 s |
| Folder GC dry run | 21 ms | 70 ms |
| WebDAV sync, first push of every note | 1.7 s | 4.3 s |
| WebDAV sync, no changes | 0.34 s | 0.5 s |
| WebDAV sync, 1 000 new notes | 0.53 s | 0.77 s |

Before the changes below, the app unlock took 520 ms (5 000 notes) and 2.4 s (20 000 notes).

## Hotspots and what was done

1. **Index build at unlock** dominates everything else (about 80 % of unlock time). Profiling by
   elimination showed three costs, all fixed:
    * every note body went through a full Markdown parse to find wikilinks, even when it contains no
      `[[`; `parse_links` now returns early (a note without `[[` cannot hold a wikilink);
    * text normalisation (NFC plus lowercase) allocated and ran the Unicode tables for every word;
      pure ASCII text now takes a path that is equivalent by definition;
    * the inverted index cloned every term for every note even when the term already existed.

    Unlock at 5 000 notes went from 520 ms to 365 ms, and at 20 000 notes from 2.4 s to 1.5 s. The
    benchmark notes contain links in every paragraph, so real vaults, where many notes have none,
    gain more.
2. **Peak memory at unlock:** every decrypted note was collected into a `Vec` before indexing. They
   are now indexed one at a time, so only one decrypted copy exists besides the index.
3. Everything interactive (search, backlinks, tree, save) is in the low milliseconds even at 20 000
   notes. Saving does not depend on the vault size.

### Follow-ups, not done

* **Notes are decrypted twice at unlock**: once by the storage scan (for the summaries) and once to
  build the index (about 250 ms of the 1.5 s at 20 000 notes). Fixing it needs a storage API that
  hands the decrypted note to a callback during the scan. It is small but touches the storage
  crate's public surface, so it deserves its own change.
* The remaining index time is the Markdown parse of notes that do contain wikilinks. A cheaper
  scanner for `[[...]]` that agrees with `pulldown-cmark` (code spans and escapes) would need
  property tests against the parser.
* Index building and note decryption are single-threaded. They parallelise well, but a thread pool
  is a new dependency or a new `std::thread::scope` code path; measure on the target phone first.
* **Sync with no changes** lists the whole remote and local state each cycle. At 20 000 notes it is
  sub-second locally, but over a real WebDAV connection the cost is the number of requests; measure
  with real latency before optimising.
* **First push of everything** over WebDAV is one request per object. Batching is limited by what
  WebDAV offers (no multi-put), so this is mostly a connection reuse and parallelism question.
* The index keeps every note body in memory for snippets. At 20 000 notes of 2 KiB that is about
  40 MB plus the postings. Acceptable on desktop, to be measured on a low-memory phone; snippets
  could be produced by decrypting the few result notes on demand, at the cost of extra work per
  search.

PLAN.md section 4 sets a target of unlock plus index for about 10 000 notes on a mid-range Android
device. The desktop figure for 10 000 notes is well under a second; the Android figure still needs a
measurement on a device (see below).

## Battery (Android and laptops)

Periodic work in the app, as reviewed:

| Timer | Was | Now |
| --- | --- | --- |
| Autosave, auto-lock and sync scheduling tick | every 1 s, always | every 1 s while unlocked, every 3 s when locked or no vault |
| Collect a finished sync cycle | every 200 ms, always | every 200 ms only while a cycle runs, otherwise every 1 s |
| WebDAV check result | every 200 ms while the dialog is open | 200 ms only while a check runs, otherwise 1 s |
| Relative time refresh ("synced 3 min ago") | every 20 s, always | every 20 s while unlocked only |

On Android the app locks as soon as it leaves the foreground (ADR 0006), so a backgrounded process
holds no unlocked session, and the timers above run at their slow rates; the OS normally freezes the
process anyway. Sync runs only while unlocked, on the one-second tick, so there is no background
sync, no wake lock and no alarm. The sync interval itself is minutes (`SyncTiming`), not seconds.

Still to measure on a device, because a desktop cannot show it:

* Battery drain over an hour with the app in the foreground and idle, unlocked with sync on, and
  locked (Android Battery Historian or `adb shell dumpsys batterystats`).
* Whether the one-second UI tick can be replaced by a timer armed for the next deadline (autosave
  debounce, auto-lock, next sync). This would remove the idle wakeups entirely but needs the core to
  report its next deadline, so it is left until the measurement shows it matters.
* Unlock plus index time and peak memory at 5 000, 10 000 and 20 000 notes on a mid-range phone
  with production Argon2id parameters, and the time to resume after the OS killed the process.
* Cost of a sync cycle on mobile data and on slow flash storage.
