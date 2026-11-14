//! In-memory, rebuildable note index: link resolution, backlinks, tags and full-text search.
//!
//! # Semantics
//!
//! The index is derived data. It is built from decrypted [`Note`]s (`Index::from_notes`), kept
//! current with [`Index::upsert`] and [`Index::remove`], and can always be thrown away and rebuilt
//! with [`Index::rebuild`]; the result never depends on the order of operations. It has no I/O, no
//! persistence and no on-disk cache.
//!
//! * **Resolution** matches [`resolve`](crate::resolve) exactly (same normalisation, path first,
//!   then unique last-segment-or-title match) but through precomputed keys, so a lookup is a hash
//!   probe. Where [`resolve`](crate::resolve) depends on candidate order (two notes with the same
//!   path), the index picks the smallest [`NoteId`] and lists ambiguous candidates in id order, so
//!   results are deterministic. Links are resolved at query time, so creating, renaming, moving or
//!   deleting a note immediately changes how other notes' links resolve.
//! * **Backlinks** of a note are the other notes with a wikilink that currently resolves to it.
//!   Links from a note to itself are not backlinks.
//! * **Tags** match after NFC and lowercase; the listing shows the smallest display form in use.
//! * **Search** is case-insensitive and NFC. Text is split into words (alphanumeric runs; scripts
//!   without spaces, such as Chinese, form one word per run). All query terms must match, the last
//!   one as a prefix. Matches in the title rank above path and tags, which rank above the body.
//!
//! # Plaintext
//!
//! The index holds plaintext derived from notes (bodies, words, titles' keys) in memory only. On
//! drop, and when a note is replaced or removed, the owned strings are zeroized where practical.
//! This is best effort: reallocation while building, hash-table growth and the allocator may leave
//! copies that cannot be reached, and the OS may swap memory out.

mod search;

use crate::resolve::{NoteKeys, norm, normalize_target};
use crate::{Resolution, Resolver, WikiLink, parse_links};
use nuvoletta_core::{Note, NoteId};
use search::{BODY, PATH, SearchIndex, TAGS, TITLE};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::ops::Range;
use zeroize::Zeroize;

/// A wikilink in a note together with how it currently resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingLink {
    /// The link as parsed from the body.
    pub link: WikiLink,
    /// What its target currently resolves to.
    pub resolution: Resolution,
}

/// A link whose target matches no note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedLink {
    /// The note containing the link.
    pub source: NoteId,
    /// The target as written.
    pub target: String,
    /// Byte range of the link in the source body.
    pub range: Range<usize>,
}

/// A link whose target matches several notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousLink {
    /// The note containing the link.
    pub source: NoteId,
    /// The target as written.
    pub target: String,
    /// Byte range of the link in the source body.
    pub range: Range<usize>,
    /// The matching notes, in id order.
    pub candidates: Vec<NoteId>,
}

/// A tag and the number of notes using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCount {
    /// Display form (the smallest form in use among the notes).
    pub tag: String,
    /// Number of notes with the tag.
    pub count: usize,
}

/// A search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// The matching note.
    pub id: NoteId,
    /// Relevance; higher is better. Title hits outrank path and tag hits, which outrank body hits.
    pub score: u32,
    /// A short excerpt of the body around the first match, if the body matches.
    pub snippet: Option<String>,
}

/// A wikilink with its normalised target key.
#[derive(PartialEq, Eq)]
struct IndexedLink {
    link: WikiLink,
    key: String,
}

/// What the index knows about one note.
#[derive(PartialEq, Eq)]
struct Entry {
    keys: NoteKeys,
    /// `(normalised, display)` tags.
    tags: Vec<(String, String)>,
    links: Vec<IndexedLink>,
    /// Kept for snippets.
    body: String,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.keys.path.zeroize();
        self.keys.last.zeroize();
        self.keys.title.zeroize();
        for (a, b) in &mut self.tags {
            a.zeroize();
            b.zeroize();
        }
        for l in &mut self.links {
            l.key.zeroize();
            l.link.target.zeroize();
            l.link.alias.zeroize();
            l.link.fragment.zeroize();
        }
        self.body.zeroize();
    }
}

type Multi = HashMap<String, BTreeSet<NoteId>>;

fn add_to(map: &mut Multi, key: &str, id: NoteId) {
    if !key.is_empty() {
        map.entry(key.to_owned()).or_default().insert(id);
    }
}

fn remove_from(map: &mut Multi, key: &str, id: NoteId) {
    if let Some(set) = map.get_mut(key) {
        set.remove(&id);
        if set.is_empty()
            && let Some((mut k, _)) = map.remove_entry(key)
        {
            k.zeroize();
        }
    }
}

/// The in-memory note index. See the [module documentation](self).
#[derive(Default, PartialEq, Eq)]
pub struct Index {
    entries: HashMap<NoteId, Entry>,
    /// Normalised full path to notes.
    by_path: Multi,
    /// Normalised last path segment or title to notes.
    by_name: Multi,
    /// Normalised link target to the notes containing such a link.
    linked: Multi,
    /// Normalised tag to notes.
    tags: Multi,
    search: SearchIndex,
}

impl fmt::Debug for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Index({} notes, <redacted>)", self.entries.len())
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        for map in [
            &mut self.by_path,
            &mut self.by_name,
            &mut self.linked,
            &mut self.tags,
        ] {
            for (mut k, _) in std::mem::take(map) {
                k.zeroize();
            }
        }
    }
}

impl Index {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an index from `notes`.
    pub fn from_notes<'a, I: IntoIterator<Item = &'a Note>>(notes: I) -> Self {
        let mut index = Self::new();
        for n in notes {
            index.upsert(n);
        }
        index
    }

    /// Discard everything and rebuild from `notes`.
    pub fn rebuild<'a, I: IntoIterator<Item = &'a Note>>(&mut self, notes: I) {
        *self = Self::from_notes(notes);
    }

    /// Number of indexed notes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether the note is indexed.
    #[must_use]
    pub fn contains(&self, id: NoteId) -> bool {
        self.entries.contains_key(&id)
    }

    /// Insert a note, replacing any earlier revision with the same id.
    pub fn upsert(&mut self, note: &Note) {
        self.remove(note.id);
        let id = note.id;
        let keys = NoteKeys::new(&note.path, &note.title);
        add_to(&mut self.by_path, &keys.path, id);
        add_to(&mut self.by_name, &keys.last, id);
        add_to(&mut self.by_name, &keys.title, id);

        let tags: Vec<(String, String)> = note
            .tags
            .iter()
            .map(|t| (norm(t), t.clone()))
            .filter(|(k, _)| !k.is_empty())
            .collect();
        for (k, _) in &tags {
            add_to(&mut self.tags, k, id);
        }

        let links: Vec<IndexedLink> = parse_links(&note.body)
            .into_iter()
            .filter_map(|link| {
                let key = normalize_target(&link.target);
                (!key.is_empty()).then_some(IndexedLink { link, key })
            })
            .collect();
        for l in &links {
            add_to(&mut self.linked, &l.key, id);
        }

        let tag_text = note.tags.join(" ");
        self.search.add(
            id,
            &[
                (TITLE, &note.title),
                (PATH, note.path.as_str()),
                (TAGS, &tag_text),
                (BODY, &note.body),
            ],
        );
        self.entries.insert(
            id,
            Entry {
                keys,
                tags,
                links,
                body: note.body.clone(),
            },
        );
    }

    /// Remove a note. Returns whether it was indexed.
    pub fn remove(&mut self, id: NoteId) -> bool {
        let Some(entry) = self.entries.remove(&id) else {
            return false;
        };
        remove_from(&mut self.by_path, &entry.keys.path, id);
        remove_from(&mut self.by_name, &entry.keys.last, id);
        remove_from(&mut self.by_name, &entry.keys.title, id);
        for (k, _) in &entry.tags {
            remove_from(&mut self.tags, k, id);
        }
        for l in &entry.links {
            remove_from(&mut self.linked, &l.key, id);
        }
        self.search.remove(id);
        true
    }

    /// Resolve a wikilink target (without fragment); same rules as [`crate::resolve`].
    #[must_use]
    pub fn resolve_target(&self, target: &str) -> Resolution {
        self.resolve_key(&normalize_target(target))
    }

    fn resolve_key(&self, key: &str) -> Resolution {
        if key.is_empty() {
            return Resolution::Unresolved;
        }
        if let Some(id) = self.by_path.get(key).and_then(|s| s.first()) {
            return Resolution::Resolved(*id);
        }
        match self.by_name.get(key) {
            None => Resolution::Unresolved,
            Some(s) if s.len() == 1 => s
                .first()
                .map_or(Resolution::Unresolved, |id| Resolution::Resolved(*id)),
            Some(s) => Resolution::Ambiguous(s.iter().copied().collect()),
        }
    }

    /// The wikilinks of a note, in document order, with their current resolution. Empty if the
    /// note is unknown.
    #[must_use]
    pub fn outgoing(&self, id: NoteId) -> Vec<OutgoingLink> {
        self.entries.get(&id).map_or_else(Vec::new, |e| {
            e.links
                .iter()
                .map(|l| OutgoingLink {
                    link: l.link.clone(),
                    resolution: self.resolve_key(&l.key),
                })
                .collect()
        })
    }

    /// Notes (other than `id`) with a wikilink that currently resolves to `id`, in id order.
    #[must_use]
    pub fn backlinks(&self, id: NoteId) -> Vec<NoteId> {
        let Some(e) = self.entries.get(&id) else {
            return Vec::new();
        };
        let mut out = BTreeSet::new();
        for key in [&e.keys.path, &e.keys.last, &e.keys.title] {
            if self.resolve_key(key) == Resolution::Resolved(id)
                && let Some(sources) = self.linked.get(key)
            {
                out.extend(sources.iter().copied());
            }
        }
        out.remove(&id);
        out.into_iter().collect()
    }

    /// Links matching no note, ordered by source id then position.
    #[must_use]
    pub fn unresolved(&self) -> Vec<UnresolvedLink> {
        self.link_issues(|r| matches!(r, Resolution::Unresolved))
            .into_iter()
            .map(|(source, l, _)| UnresolvedLink {
                source,
                target: l.link.target.clone(),
                range: l.link.range.clone(),
            })
            .collect()
    }

    /// Links matching several notes, ordered by source id then position.
    #[must_use]
    pub fn ambiguous(&self) -> Vec<AmbiguousLink> {
        self.link_issues(|r| matches!(r, Resolution::Ambiguous(_)))
            .into_iter()
            .map(|(source, l, r)| AmbiguousLink {
                source,
                target: l.link.target.clone(),
                range: l.link.range.clone(),
                candidates: match r {
                    Resolution::Ambiguous(c) => c,
                    _ => Vec::new(),
                },
            })
            .collect()
    }

    fn link_issues(
        &self,
        keep: fn(&Resolution) -> bool,
    ) -> Vec<(NoteId, &IndexedLink, Resolution)> {
        let mut ids: Vec<NoteId> = self.entries.keys().copied().collect();
        ids.sort_unstable();
        let mut out = Vec::new();
        for id in ids {
            for l in &self.entries[&id].links {
                let r = self.resolve_key(&l.key);
                if keep(&r) {
                    out.push((id, l, r));
                }
            }
        }
        out
    }

    /// Notes carrying `tag` (NFC, case-insensitive), in id order.
    #[must_use]
    pub fn notes_with_tag(&self, tag: &str) -> Vec<NoteId> {
        self.tags
            .get(&norm(tag))
            .map_or_else(Vec::new, |s| s.iter().copied().collect())
    }

    /// All tags with their note counts, sorted by normalised tag.
    #[must_use]
    pub fn tags(&self) -> Vec<TagCount> {
        let mut out: Vec<(&String, TagCount)> = self
            .tags
            .iter()
            .map(|(key, ids)| {
                let display = ids
                    .iter()
                    .filter_map(|id| self.entries.get(id))
                    .flat_map(|e| e.tags.iter())
                    .filter(|(k, _)| k == key)
                    .map(|(_, d)| d)
                    .min()
                    .cloned()
                    .unwrap_or_default();
                (
                    key,
                    TagCount {
                        tag: display,
                        count: ids.len(),
                    },
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out.into_iter().map(|(_, t)| t).collect()
    }

    /// Full-text search over title, path, tags and body: every query term must match (the last as
    /// a prefix). Best first, ties by id; at most `limit` hits, with body snippets.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let q = search::terms(query);
        self.search
            .query(&q)
            .into_iter()
            .take(limit)
            .map(|(id, score)| SearchHit {
                id,
                score,
                snippet: self
                    .entries
                    .get(&id)
                    .and_then(|e| search::snippet(&e.body, &q)),
            })
            .collect()
    }
}

impl Resolver for Index {
    fn resolve(&self, target: &str) -> Resolution {
        self.resolve_target(target)
    }
}

#[cfg(test)]
mod tests;
