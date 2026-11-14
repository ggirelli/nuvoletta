# ADR 0003: Markdown dialect, wikilinks and safe preview rendering

Status: Proposed (Phase 2, owner decisions recorded, not yet reviewed)
Date: 2026-11-14

## Context

Notes are Markdown (PLAN.md principle on portability). The preview renders note bodies, which may come from imports or a synced device, so they are untrusted input inside a webview that holds decrypted plaintext. We need a fixed dialect, a linking syntax compatible with Obsidian-style notes, and a rendering policy that cannot execute script or leak requests.

## Decision

1. **Dialect.** `pulldown-cmark` with `CommonMark` plus tables, strikethrough, task lists and footnotes. Nothing else: no math, smart punctuation, heading attributes, metadata blocks or raw-HTML passthrough.
2. **Wikilinks.** `[[Target]]`, `[[Target|alias]]` and `[[Target#Heading|alias]]`. Parsing uses pulldown-cmark's built-in `ENABLE_WIKILINKS` (so code spans, code blocks, raw HTML and escaped `\[[` are not links, and split text events are not our problem). The fragment is parsed and carried but not resolved. Links are stored as text in the body and never rewritten.
3. **Resolution** (pure function over a candidate set, reused by the index): trim, NFC and Unicode-lowercase both sides. (a) An exact match on the full virtual path resolves. Otherwise (b) notes whose last path segment or title matches: one resolves, several are `Ambiguous` (never silently picked), none is `Unresolved`.
4. **Safe rendering.** A small in-crate renderer over the parser's event stream, not the stock HTML writer:
    - raw HTML (block and inline) is escaped as text;
    - link schemes are allow-listed (`http`, `https`, `mailto`); the check runs on the parser-decoded URL after removing control characters (tab and newline obfuscation), case-insensitively; anything else, including relative URLs, renders as `<span class="link-blocked">` with the link text;
    - wikilinks render as `<a href="nuvoletta://note/<id>[#fragment]" class="wikilink">` when resolved, otherwise as non-navigable `<span class="wikilink unresolved">` or `<span class="wikilink ambiguous">`; the host application intercepts the `nuvoletta://` scheme;
    - images are blocked by default (placeholder `<span class="image-blocked">` with alt text); `RenderOptions::allow_remote_images` opts in to `http`/`https` images with `referrerpolicy="no-referrer"`;
    - no `style` or event-handler attribute is ever emitted; the only generated attributes are `href`, `rel`, `title`, `class`, `id`, `src`, `alt`, `start`, `loading`, `referrerpolicy` and checkbox `type`/`disabled`/`checked`, all escaped or built from sanitised tokens;
    - rendering takes a `Resolver` (any `Fn(&str) -> Resolution`), so the crate does not depend on storage.
5. **No `ammonia` pass.** The renderer emits HTML only from a fixed set of tags and escapes all text and attribute values, so there is no input path that produces unknown markup; `ammonia` would pull in the `html5ever` tree (a large dependency) to re-parse output we fully control. Defence in depth is provided instead by property tests asserting the output never contains script, event handlers or disallowed schemes, and by the webview's CSP (to be set in the UI phase). Revisit if the renderer ever passes through markup.
6. **Index.** `nuvoletta_markdown::Index` is an in-memory, rebuildable index over decrypted notes (no I/O, database or on-disk cache). It precomputes the normalised keys used by `resolve` (one shared normalisation) so resolution is a hash lookup with identical semantics, except that ties between notes with the same path are broken by smallest note id instead of candidate order. Links are resolved at query time, so creating, renaming, moving or deleting a note updates every other note's links, backlinks (self-links excluded), `unresolved()` and `ambiguous()` immediately. Tags match after NFC and lowercase. Full-text search tokenises into alphanumeric runs (no dictionary segmentation for scripts without spaces), ANDs all terms with a prefix match on the last, and ranks title above path and tags above body. Incremental `upsert`/`remove` and `rebuild` give identical state (property tested). The index owns plaintext (bodies for snippets, words); it zeroizes owned strings on drop and removal, best effort.

## Consequences

- Positive: small dependency surface (`pulldown-cmark`, already needed); one parser configuration shared by indexing and rendering, so the links a user sees are the links the index knows.
- Negative: relative and fragment-only links do not work in preview; footnote anchors are the only in-page links. Alignment is exposed as classes, not styles.
- Negative: wikilink semantics follow pulldown-cmark's parser; upgrades need the link tests re-run.
- Risk: a future renderer change could reintroduce unsafe output; the malicious-input and property tests guard this and must be extended with every new construct.

## Alternatives considered

- Stock `pulldown_cmark::html` writer plus `ammonia`: more code to trust, heavier dependency tree, still needs custom wikilink handling.
- `comrak`: larger feature surface than the owner asked for.
- Hand-parsing `[[...]]` from text events: fragile (split events, code contexts); the parser's own support is more correct.
