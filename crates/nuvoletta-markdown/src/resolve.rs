use nuvoletta_core::{Note, NoteId, NotePath};
use unicode_normalization::UnicodeNormalization;

/// A note that a wikilink may point to.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// The note id.
    pub id: NoteId,
    /// The note's virtual path.
    pub path: &'a NotePath,
    /// The note's title.
    pub title: &'a str,
}

impl<'a> From<&'a Note> for Candidate<'a> {
    fn from(n: &'a Note) -> Self {
        Self {
            id: n.id,
            path: &n.path,
            title: &n.title,
        }
    }
}

/// The outcome of resolving a wikilink target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one note matches.
    Resolved(NoteId),
    /// Several notes match by last path segment or title (candidate order, no duplicates).
    Ambiguous(Vec<NoteId>),
    /// No note matches.
    Unresolved,
}

/// Normalise for comparison: trim, NFC, Unicode lowercase. The single normalisation used by
/// resolution, tags and the index.
pub(crate) fn norm(s: &str) -> String {
    s.trim().nfc().collect::<String>().to_lowercase()
}

/// Normalise a wikilink target: strip leading and trailing `/`, then [`norm`]. Empty means the
/// target cannot match anything.
pub(crate) fn normalize_target(target: &str) -> String {
    norm(target.trim_matches('/'))
}

/// The normalised keys a note can be found by.
#[derive(PartialEq, Eq)]
pub(crate) struct NoteKeys {
    /// Normalised full virtual path.
    pub(crate) path: String,
    /// Normalised last path segment.
    pub(crate) last: String,
    /// Normalised title.
    pub(crate) title: String,
}

impl NoteKeys {
    pub(crate) fn new(path: &NotePath, title: &str) -> Self {
        let path = norm(path.as_str());
        let last = path.rsplit('/').next().unwrap_or_default().to_owned();
        Self {
            path,
            last,
            title: norm(title),
        }
    }
}

/// Resolve a wikilink `target` (without fragment) against `candidates`.
///
/// (a) A case-insensitive exact match on the full virtual path wins (the first such candidate if
/// paths collide). Otherwise (b) notes whose last path segment or title equals the target: one
/// match resolves, several are [`Resolution::Ambiguous`], none is [`Resolution::Unresolved`].
/// Comparison is NFC, Unicode-lowercase and whitespace-trimmed on both sides.
pub fn resolve<'a, I>(target: &str, candidates: I) -> Resolution
where
    I: IntoIterator<Item = Candidate<'a>>,
{
    let t = normalize_target(target);
    if t.is_empty() {
        return Resolution::Unresolved;
    }
    let mut loose: Vec<NoteId> = Vec::new();
    for c in candidates {
        let keys = NoteKeys::new(c.path, c.title);
        if keys.path == t {
            return Resolution::Resolved(c.id);
        }
        if (keys.last == t || keys.title == t) && !loose.contains(&c.id) {
            loose.push(c.id);
        }
    }
    match loose.len() {
        0 => Resolution::Unresolved,
        1 => Resolution::Resolved(loose[0]),
        _ => Resolution::Ambiguous(loose),
    }
}
