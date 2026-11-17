# WebDAV setup guide

Nuvoletta can sync through a WebDAV server (Nextcloud, ownCloud, a NAS, Apache, nginx). On Android
this is the way to sync. Only encrypted files are sent; the server sees opaque names, sizes and
timing (`THREAT_MODEL.md`).

## The one rule: the account must not be able to change or delete files

Nuvoletta refuses to use a WebDAV account that can **delete**, **overwrite**, **rename or move**
files. It needs an account that can **read, list and create** files and folders, nothing more.

Why: every file Nuvoletta writes is written once and never changed (`SYNC_PROTOCOL.md` section 1).
Your password for the server is stored on your devices; if it is stolen, an account that can only
read and create cannot destroy your synced notes. An account that can delete could wipe the server
copy, and the server copy is what saves you when a device is lost.

When you enter the address and password, and again at the start of every session (after you unlock
the vault), Nuvoletta checks this by trying it: see [How the check works](#how-the-check-works).
If the account can do more, or the answer is unclear, sync is not used and the app says why.

Consequences:

- **No "Clean up old versions" on WebDAV.** Removing old versions needs delete rights. Old versions
  stay; they are small. To reclaim space, an administrator can delete files with the server's own
  tools (never delete the newest versions or any `h/` file).
- Header generations are never deleted, and a damaged upload cannot be replaced in place (see
  [Interrupted uploads](#interrupted-uploads)).

## Nextcloud (and ownCloud)

1. Create a **dedicated user** (for example `nuvoletta`), or use your own account only to create the
   share below. A dedicated account is better: its password then gives access to nothing else.
2. In the owner account, create a folder, for example `Nuvoletta`.
3. **Share** the folder with the dedicated user (or, with your own account, use the folder itself
   only if you can restrict permissions as below). Open the share's *Custom permissions* and allow
   **Read** and **Create** only. Switch off *Edit*, *Delete* and *Share*. (File-drop shares cannot
   list files and do not work.)
4. As the dedicated user, create an **app password** (Settings, Security, *Devices & sessions*, *Create
   new app password*). Use it in Nuvoletta instead of the login password; you can revoke it on its
   own.
5. The folder address is the WebDAV URL of the share as that user sees it, for example
   `https://cloud.example.org/remote.php/dav/files/nuvoletta/Nuvoletta/`. (Open the folder in the web
   interface, *Settings* at the bottom left shows the WebDAV base URL.)
6. In Nuvoletta: *Sync*, section *WebDAV*, enter address, user name and app password, *Check account
   and save*.
7. Turn on **file versions and the trash** for the owner account (default on). Because the
   dedicated user cannot delete, they matter little, but they protect against a mistake made in the
   owner account.

Nextcloud reports the account's rights (`oc:permissions`); Nuvoletta uses this as a quick first
refusal (a `D`, `W`, `N` or `V` means refused) and still proves it by trying.

## Other servers

Any WebDAV server works if all of this holds for the account:

- `PROPFIND` (depth 0 and 1), `GET`, `MKCOL` and `PUT` work; `PUT` honours `If-None-Match: *`
  (`412` when the file exists) or answers `403`/`405` for an existing name;
- `PUT` over an existing file, `COPY` over it, `MOVE` and `DELETE` are refused (`401`, `403`, `405`
  or `501`).

Apache `mod_dav`, nginx (with `nginx-dav-ext-module` for `PROPFIND`) and NAS web servers: these
were **not tested** against Nuvoletta. The usual difficulty is "create but not overwrite": plain
servers let an account that may `PUT` also replace files. Restrict the methods the account may use
to `GET`, `PROPFIND`, `MKCOL` and `PUT`, and make `PUT` refuse an existing file (a filesystem
permission or ACL that lets the account create but not modify files, or a rule in front of the
server). Run the check: it tells you what the account can still do. If your server cannot express
this, use Nextcloud or another server that can.

The address must start with `https://`. Plain `http://` is refused with no override.

### Self-signed certificates (NAS)

If the server's certificate is not signed by a public authority, Nuvoletta shows the certificate's
**SHA-256 fingerprint**, without having sent your password. Compare it with the fingerprint your NAS
or router shows; if they are identical, tick the confirmation and continue. From then on only that
exact certificate is accepted for this address. If the server's certificate is renewed, the
fingerprint changes and you must confirm the new one. There is no "ignore certificate errors"
option.

Public CA certificates (for example Let's Encrypt) are checked against a built-in list of roots, not
the system store.

## How the check works

(Details: `docs/ADR/0007-webdav-backend.md`.)

1. `PROPFIND` the folder. Nextcloud's `oc:permissions` with `D`, `W`, `N` or `V` refuses at once.
2. `MKCOL` the folders `h`, `d`, `o` (creating folders needs only the *create* right).
3. In the main folder and in each of `h/`, `d/`, `o/` (rights can differ per folder): make sure the
   test file `.nuvoletta-probe` exists, then try to overwrite it, copy over a second test file, move
   it and delete it. Every attempt must be refused, and the file is read back to be sure it is
   unchanged. When you have just entered the credentials, one extra uniquely named test file
   `.nuvoletta-probe-<random>` is created to prove that creating works.
4. If the account turns out to be unsafe, the test files are deleted again if possible.

Anything unexpected (an odd status, a timeout in the middle) counts as "unknown", and unknown is
refused. The test files are not Nuvoletta files; every device ignores them and lists them as stray.
A server that cannot delete keeps about five small test files for good.

**Limits of the check.** It probes the main folder and `h/`, `d/`, `o/`, but not the sub-folders
that Nuvoletta creates later (`d/<device>/l/`, `o/xx/`), and it never tries to delete or move a
whole folder. On Nextcloud, ownCloud and most NAS shares the account's rights are the same
everywhere, so this is enough. On servers with per-folder or owner-based rules (Apache
`<Location>`/`<LimitExcept>` blocks, ACLs that give a folder's creator extra rights) a stolen password
could possibly delete or move a folder the account itself created. If you run such a server, give the
account no delete or move right anywhere in the tree and check it by hand once, for example by
trying to delete a sub-folder of `o/` with a WebDAV client.

## Interrupted uploads

Most servers (Nextcloud included) commit a file only when the upload is complete. A server that
keeps the half of an upload that was cut off, under the final name, would block that name for good
because nothing can overwrite or delete it. Nuvoletta handles it without changing the format:

- a half-written **note object** is detected after ten minutes; the note is sealed again under a new
  random id and uploaded;
- a half-written **log segment** blocks the device's next log file. After ten minutes the device
  continues under a new device id and re-sends the unsent entries (the existing fork-recovery
  path). Other devices may show a harmless "history stops at a damaged file" notice for the old id.

## Not supported

Digest or NTLM authentication (Basic over HTTPS only), HTTP proxies, client certificates, HTTP/2,
folder-level locking. The address, user name and password are stored only on the device where you
entered them (`STORAGE_FORMAT.md`); enter them on every device.

## If the password may have leaked

Revoke the app password on the server and enter a new one. The attacker could read the (encrypted)
files; to read them they would also need your vault passphrase and one of your devices' files. If
you think both leaked, change the vault passphrase and rotate the recovery key as well.
