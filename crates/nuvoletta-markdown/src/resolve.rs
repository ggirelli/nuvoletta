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

/// Normalise for comparison: trim, NFC, Unicode lowercase.
fn norm(s: &str) -> String {
    s.trim().nfc().collect::<String>().to_lowercase()
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
    let t = norm(target.trim_matches('/'));
    if t.is_empty() {
        return Resolution::Unresolved;
    }
    let mut loose: Vec<NoteId> = Vec::new();
    for c in candidates {
        let path = norm(c.path.as_str());
        if path == t {
            return Resolution::Resolved(c.id);
        }
        let last = path.rsplit('/').next().unwrap_or_default();
        if (last == t || norm(c.title) == t) && !loose.contains(&c.id) {
            loose.push(c.id);
        }
    }
    match loose.len() {
        0 => Resolution::Unresolved,
        1 => Resolution::Resolved(loose[0]),
        _ => Resolution::Ambiguous(loose),
    }
}
