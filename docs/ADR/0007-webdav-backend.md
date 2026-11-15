# 0007. WebDAV backend with create-only credentials

Status: Accepted (2026-11)

## Context

WebDAV was deferred in the MVP (PLAN.md Phase 5). It is now the chosen route to sync on Android
(ADR 0006: the Storage Access Framework route needs JNI and a device) and later iOS. Decisions:

## Decisions

1. **HTTP stack.** `ureq` 3 (blocking, rustls with the ring provider and the built-in `webpki-roots`, no system store and so no JNI on Android) with `allow_non_standard_methods` for `PROPFIND`, `MKCOL`, `MOVE` and `COPY`, `https_only`, no environment proxy, `http_status_as_error(false)` (statuses are mapped by the client) and `max_redirects(0)`: a same-origin redirect is followed by the client itself (at most three), anything else is refused, so credentials are sent only to the configured origin. `quick-xml` reads `PROPFIND` answers. Self-signed servers: `ureq`'s `TlsConfig` only takes root stores, not a verifier, so a pinned certificate replaces ureq's TLS connector in the connector chain (`tls.rs`, about 80 lines on ureq's public `unversioned::transport` API): rustls with a verifier that accepts only the leaf certificate whose SHA-256 equals the pin (after the user confirmed the fingerprint) and still checks the handshake signature; name and validity period are not checked, because the pin identifies the exact certificate. The fingerprint is obtained by a handshake-only connection that sends nothing.
2. **Credentials (option A).** The address, user name and password are stored only in
   `.nuvoletta/device-state/webdav`, sealed as envelope type 4 with `K_local`, never synced, readable
   only while the vault is unlocked. Stronger than desktop keyrings while the vault is locked
   (keyring entries are readable by any process of the user on Linux and Windows), identical on
   every platform, needs no JNI. Limit: the vault passphrase plus the device files reveal the
   credential; mitigation: a dedicated account or app password that can only read and create, which
   can be rotated.
3. **Refuse credentials that can delete or overwrite.** A stolen sync password must not be able to
   destroy the remote copy. The check (`permissions.rs`, described in `WEBDAV.md`) proves create and
   refusal of overwrite, copy-over, move and delete by trying, in the root and in `h/`, `d/`, `o/`.
   It runs at setup and before the first operation of every session. Unknown fails closed. During
   normal operation the client sends only `PROPFIND`, `GET`, `PUT` (with `If-None-Match: *`) and
   `MKCOL`.
4. **Probe names.** `.nuvoletta-probe` (stable, reused by every check and device),
   `.nuvoletta-probe-b`, and at setup one `.nuvoletta-probe-<random>`. They are outside the name
   grammar, hence stray. Stable names (instead of a fresh random one per check) because a server
   that cannot delete would otherwise collect one file per session; the random one proves creation
   at setup.
5. **Consequences.** No garbage collection on WebDAV (`Capabilities::can_delete` false, `GcSkip::NoDelete`);
   header generations are never deleted; uploads are create-only `PUT`s straight to the final name
   (no temporary name and `MOVE`, which needs delete rights); `MKCOL` is permitted by create-only
   rights (Nextcloud `K`).
6. **Listings.** `PROPFIND` depth 1 per directory. A sync cycle uses `Provider::list_cycle`, which
   skips the 256 shards of `o/` (objects are fetched by name); the only loss is the discovery of
   authenticated stray copies of objects made by file-sync tools, which do not exist on WebDAV.

## Partial uploads (no protocol change)

With no delete and no overwrite, a half-written file under a deterministic name would block it
forever. Nextcloud and most servers commit uploads atomically, so this needs a server that keeps
the partial body after a dropped connection (Apache `mod_dav` and nginx can, on some failures). It is
handled in the engine, not by changing names or formats:

- Providers declare `authoritative` (a listed file is the truth, no propagation delay). Only then
  does the engine stop waiting for an unreadable file under a name it owns.
- Own log segment `d/<id>/l/<first_seq>.nvlog` unreadable for `partial_upload_grace_ms` (default
  10 minutes, tracked in the journal): the device uses the existing own-fork recovery
  (SYNC_PROTOCOL 10.4, as for a lost create-only race): new device id, unsent entries re-emitted
  with their original revision ids. The old id keeps its confirmed prefix; its stream stops at the
  damaged file, which other devices report as a gap.
- Own object `o/xx/<id>.nvobj` with different bytes: after the grace period the note is sealed again
  (`install_revision`) under a fresh random `object_id` and the next scan queues it with the same
  parents. Only the note's current revision can be re-sealed; an older re-emitted one stays
  blocked.

No file is deleted or overwritten. The on-disk journal gains an optional field (local state, not a
wire format). **There is no change of the remote name grammar or of SYNC_PROTOCOL's formats.**

## Consequences

- A new crate `nuvoletta-webdav`; `nuvoletta-sync` only gains the capability fields, `list_cycle`,
  three error variants and the partial-upload handling.
- Setup needs a server and an account that can express "read and create only".
- Digest authentication, proxies and HTTP/2 are not supported.
