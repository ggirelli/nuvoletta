# Nuvoletta Plaintext Export Format (v1)

Export and import are the portability mechanism of Nuvoletta. A vault is written out as a folder
of ordinary Markdown files; a folder of Markdown files can be read into a vault. The code is in
`nuvoletta-storage` (`portable` module). The encrypted vault format is not affected.

!!! warning "Export writes plaintext"
    Exported files are not encrypted. Anyone who can read the destination can read the notes, and
    locking the vault does not remove them. Export is always user-initiated; the app must warn the
    user and let them choose the destination. On Unix, files are created `0600` and directories
    `0700`.

## 1. Layout

The note with virtual path `a/b/c` is written to `<dest>/a/b/c.md`: path segments become
directories, the last segment plus `.md` is the file name.

- `<dest>` must not be the vault directory or inside it (compared after canonicalising the
  nearest existing ancestor, so symlinks and `..` cannot hide it): a vault folder may be
  synchronised and must never receive plaintext. Export fails with `DestinationInsideVault`.
- `<dest>` must not exist or must be an empty real directory (never a symlink). Files are created
  exclusively, so nothing is ever overwritten.
- **Segment escaping** (deterministic), applied to every segment in this order:
    1. `< > : " / \ | ? *`, control characters and U+2028/U+2029 become `%XX` per UTF-8 byte.
    2. The result is cut to 200 bytes (at a character boundary).
    3. A leading `.` and a trailing `.` or space become `%2E` / `%20`.
    4. A Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9`,
       with or without extension, any case) has its first character escaped (`CON` becomes
       `%43ON`).
- **Collisions.** Two notes whose file names are equal after lower-casing and Unicode NFD
  normalisation (so also on case-insensitive and normalising file systems) cannot share a name.
  Notes are processed ordered by virtual path then id; the first keeps the plain name, the others
  get `~<first 8 hex digits of the id>` appended to the stem (`idea~1a2b3c4d.md`), or the full id
  in the unlikely case that collides too. Neither note is dropped.
- Every file name component is checked to be a plain name before writing, so no file can be
  created outside `<dest>`.
- A note that cannot be read from the vault is listed in the report and skipped; the rest is
  exported.

## 2. File content

```
---
id: "0123456789abcdef0123456789abcdef"
title: "Hello: world"
path: "projects/2026/ideas"
tags: ["a", "b c"]
created: 2026-03-01T09:30:00.000Z
modified: 2026-03-02T10:00:00.250Z
---
<body, byte for byte>
```

| Key        | Meaning                                                                    |
| ---------- | -------------------------------------------------------------------------- |
| `id`       | Note id, 32 lower-case hex characters (always quoted).                     |
| `title`    | Title, double-quoted.                                                      |
| `path`     | Exact virtual path, double-quoted. Authoritative on import (file names are lossy). |
| `tags`     | Flow list of double-quoted strings; `[]` when empty.                       |
| `created`  | RFC 3339, UTC, millisecond precision (`YYYY-MM-DDTHH:MM:SS.mmmZ`).         |
| `modified` | Same.                                                                      |

Timestamps outside the years 0000 to 9999 are written as an integer of milliseconds since the
Unix epoch. Everything after the closing `---` line is the body, verbatim (line endings
included). The body may itself contain `---` lines: the front matter ends at the first one.

Double-quoted strings use YAML escapes: `\"`, `\\`, `\n`, `\r`, `\t`, and `\uXXXX` for other
control characters, U+2028/U+2029 and U+FEFF. They never contain a raw newline.

## 3. Import

Every `.md` file (any case) under the source folder is read, recursively and in name order.

- **The source must not overlap the vault.** A source folder that contains the vault directory
  or lies inside it is refused (`SourceOverlapsVault`), so the vault's own files are never read
  as notes.
- **Front matter is optional.** A file starts a front matter block with a `---` line (a leading
  BOM is ignored) and the block ends at the next `---` line. Without a closing line the file has
  no front matter (it is just a thematic break) and is imported whole.
- **Accepted syntax:** `key: value` lines with double-quoted, single-quoted or plain scalars,
  flow lists (`[a, "b"]`), block lists (`- a`), blank lines and `#` comments; timestamps as
  RFC 3339 (`Z` or offset, fraction optional), a plain date (`2026-03-01`, midnight UTC) or an
  integer of milliseconds. A closed block that is not valid (bad quoting, bad id or timestamp,
  duplicate key, unsupported value) causes the file to be skipped as malformed; nothing is
  guessed.
- **Defaults:** `title` is the file stem; `path` is the relative path without `.md`; `tags` is
  empty; `created` is now, and `modified` defaults to `created`; `id` is a fresh id.
- **Unknown keys** are ignored (not stored in the note) and listed per note in the report.
- **Id conflicts.** If the id in a file already exists in the vault:
    - with identical content (id, title, path, tags, timestamps, body) the file is reported as
      `already_present` and nothing happens, so importing the same export twice is harmless;
    - otherwise the file is imported as a new note with a fresh id and reported as a conflict
      (with the existing and the new id). The existing note is never overwritten and the
      imported data is never discarded; the user decides which copy to keep.
- **Skipped, with a reason, without stopping the import:** non-`.md` files, hidden directories
  (name starts with `.`), symlinks (to files or directories; never followed), special files,
  non-UTF-8 file names or contents, files over the body limit (16 MiB), invalid virtual paths,
  limit violations (title, tags), malformed front matter, and directories nested deeper than 32
  levels.

Both operations return a structured report (`ExportReport`, `ImportReport`); the report contains
file names and ids but never note content.
