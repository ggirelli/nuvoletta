//! Tokenisation and the in-memory inverted index behind [`Index::search`](super::Index::search).

use crate::resolve::norm;
use nuvoletta_core::NoteId;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Bound, Range};
use zeroize::Zeroize;

/// Field flags stored per posting.
pub(super) const TITLE: u8 = 1;
pub(super) const PATH: u8 = 2;
pub(super) const TAGS: u8 = 4;
pub(super) const BODY: u8 = 8;

/// Score of a term for a note: the best field it occurs in.
const fn weight(mask: u8) -> u32 {
    if mask & TITLE != 0 {
        10
    } else if mask & (PATH | TAGS) != 0 {
        5
    } else {
        1
    }
}

/// A character that is part of a word: alphanumeric, or a combining mark (so decomposed text
/// does not split words).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || ('\u{300}'..='\u{36f}').contains(&c)
}

/// Words of `text` with their byte ranges, in order. A word is a maximal run of word characters.
pub(super) fn words(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        while chars.next_if(|&(_, c)| !is_word(c)).is_some() {}
        let (start, _) = *chars.peek()?;
        let mut end = start;
        while let Some((i, c)) = chars.next_if(|&(_, c)| is_word(c)) {
            end = i + c.len_utf8();
        }
        Some((start..end, &text[start..end]))
    })
}

/// Normalised terms of `text` (NFC, lowercase).
pub(super) fn terms(text: &str) -> Vec<String> {
    words(text).map(|(_, w)| norm(w)).collect()
}

/// Inverted index: term to the notes containing it and the fields it occurs in.
#[derive(Default, PartialEq, Eq)]
pub(super) struct SearchIndex {
    postings: BTreeMap<String, HashMap<NoteId, u8>>,
    doc_terms: HashMap<NoteId, Vec<String>>,
}

impl SearchIndex {
    /// Index a note's fields. The note must not be present.
    pub(super) fn add(&mut self, id: NoteId, fields: &[(u8, &str)]) {
        let mut per_term: BTreeMap<String, u8> = BTreeMap::new();
        for &(flag, text) in fields {
            for t in terms(text) {
                *per_term.entry(t).or_default() |= flag;
            }
        }
        let mut list = Vec::with_capacity(per_term.len());
        for (t, mask) in per_term {
            if let Some(docs) = self.postings.get_mut(&t) {
                docs.insert(id, mask);
            } else {
                self.postings.insert(t.clone(), HashMap::from([(id, mask)]));
            }
            list.push(t);
        }
        self.doc_terms.insert(id, list);
    }

    /// Remove a note, if present.
    pub(super) fn remove(&mut self, id: NoteId) {
        let Some(list) = self.doc_terms.remove(&id) else {
            return;
        };
        for mut t in list {
            if let Some(docs) = self.postings.get_mut(&t) {
                docs.remove(&id);
                if docs.is_empty()
                    && let Some((mut k, _)) = self.postings.remove_entry(&t)
                {
                    k.zeroize();
                }
            }
            t.zeroize();
        }
    }

    /// Notes matching every query term (the last one as a prefix), best first.
    pub(super) fn query(&self, q: &[String]) -> Vec<(NoteId, u32)> {
        let Some((last, exact)) = q.split_last() else {
            return Vec::new();
        };
        let mut per_term: Vec<HashMap<NoteId, u32>> = Vec::with_capacity(q.len());
        for t in exact {
            let m = self.postings.get(t).map_or_else(HashMap::new, |docs| {
                docs.iter().map(|(&id, &m)| (id, weight(m))).collect()
            });
            per_term.push(m);
        }
        let mut m: HashMap<NoteId, u32> = HashMap::new();
        for (_, docs) in self
            .postings
            .range::<str, _>((Bound::Included(last.as_str()), Bound::Unbounded))
            .take_while(|(k, _)| k.starts_with(last.as_str()))
        {
            for (&id, &mask) in docs {
                let w = weight(mask);
                let e = m.entry(id).or_default();
                *e = (*e).max(w);
            }
        }
        per_term.push(m);
        per_term.sort_by_key(HashMap::len);
        let mut rest = per_term.into_iter();
        let mut acc = rest.next().unwrap_or_default();
        for other in rest {
            acc.retain(|id, score| {
                other.get(id).is_some_and(|s| {
                    *score += s;
                    true
                })
            });
        }
        let mut out: Vec<_> = acc.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }
}

impl Drop for SearchIndex {
    fn drop(&mut self) {
        for (mut k, _) in std::mem::take(&mut self.postings) {
            k.zeroize();
        }
        for (_, list) in std::mem::take(&mut self.doc_terms) {
            for mut t in list {
                t.zeroize();
            }
        }
    }
}

/// A short excerpt of `body` around the first word matching the query (the last term as a
/// prefix), with whitespace collapsed. `None` if the body has no match.
pub(super) fn snippet(body: &str, q: &[String]) -> Option<String> {
    let last = q.len().checked_sub(1)?;
    let (range, _) = words(body).find(|(_, w)| {
        let t = norm(w);
        q.iter().enumerate().any(|(i, term)| {
            if i == last {
                t.starts_with(term)
            } else {
                t == *term
            }
        })
    })?;
    let from = body[..range.start]
        .char_indices()
        .rev()
        .nth(39)
        .map_or(0, |(i, _)| i);
    let to = body[range.end..]
        .char_indices()
        .nth(80)
        .map_or(body.len(), |(i, _)| range.end + i);
    let mut s = String::new();
    if from > 0 {
        s.push('…');
    }
    s.push_str(
        &body[from..to]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    );
    if to < body.len() {
        s.push('…');
    }
    Some(s)
}
