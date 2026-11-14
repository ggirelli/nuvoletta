use super::*;
use crate::{Candidate, resolve};
use nuvoletta_core::NotePath;
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::time::Instant;

fn nid(n: u8) -> NoteId {
    NoteId::from_bytes([n; 16])
}

fn note(n: u8, title: &str, path: &str, tags: &[&str], body: &str) -> Note {
    Note {
        id: nid(n),
        title: title.to_owned(),
        path: NotePath::parse(path).unwrap(),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        created_at: 0,
        modified_at: 0,
        body: body.to_owned(),
    }
}

#[test]
fn unresolved_link_resolves_when_target_is_created_and_back_when_removed() {
    let mut ix = Index::new();
    let a = note(1, "A", "a", &[], "see [[Target]]");
    ix.upsert(&a);
    assert_eq!(ix.unresolved().len(), 1);
    assert_eq!(ix.unresolved()[0].target, "Target");
    assert!(ix.backlinks(nid(2)).is_empty());

    ix.upsert(&note(2, "Target", "notes/t", &[], ""));
    assert!(ix.unresolved().is_empty());
    assert_eq!(
        ix.outgoing(nid(1))[0].resolution,
        Resolution::Resolved(nid(2))
    );
    assert_eq!(ix.backlinks(nid(2)), vec![nid(1)]);

    assert!(ix.remove(nid(2)));
    assert_eq!(ix.unresolved().len(), 1);
    assert!(!ix.remove(nid(2)));
}

#[test]
fn second_matching_title_makes_link_ambiguous_and_renaming_fixes_it() {
    let mut ix = Index::new();
    ix.upsert(&note(1, "A", "a", &[], "[[Idea]]"));
    ix.upsert(&note(2, "Idea", "x/two", &[], ""));
    assert_eq!(ix.backlinks(nid(2)), vec![nid(1)]);

    ix.upsert(&note(3, "idea", "y/three", &[], ""));
    assert_eq!(
        ix.outgoing(nid(1))[0].resolution,
        Resolution::Ambiguous(vec![nid(2), nid(3)])
    );
    assert_eq!(ix.ambiguous().len(), 1);
    assert_eq!(ix.ambiguous()[0].candidates, vec![nid(2), nid(3)]);
    assert!(ix.backlinks(nid(2)).is_empty());
    assert!(ix.backlinks(nid(3)).is_empty());

    ix.upsert(&note(3, "Other", "y/three", &[], ""));
    assert!(ix.ambiguous().is_empty());
    assert_eq!(ix.backlinks(nid(2)), vec![nid(1)]);
}

#[test]
fn moving_a_note_updates_path_links() {
    let mut ix = Index::new();
    ix.upsert(&note(
        1,
        "A",
        "a",
        &[],
        "[[projects/plan]] [[/Projects/Plan/]]",
    ));
    ix.upsert(&note(2, "Plan", "projects/plan", &[], ""));
    assert_eq!(ix.backlinks(nid(2)), vec![nid(1)]);
    ix.upsert(&note(2, "Plan", "archive/plan", &[], ""));
    assert_eq!(ix.unresolved().len(), 2);
    assert!(ix.backlinks(nid(2)).is_empty());
    ix.upsert(&note(1, "A", "a", &[], "[[plan]]"));
    assert_eq!(ix.backlinks(nid(2)), vec![nid(1)]);
}

#[test]
fn self_links_are_not_backlinks_and_replaced_links_are_forgotten() {
    let mut ix = Index::new();
    ix.upsert(&note(1, "A", "a", &[], "[[a]] [[b]]"));
    ix.upsert(&note(2, "B", "b", &[], ""));
    assert!(ix.backlinks(nid(1)).is_empty());
    assert_eq!(ix.backlinks(nid(2)), vec![nid(1)]);
    ix.upsert(&note(1, "A", "a", &[], "no links"));
    assert!(ix.backlinks(nid(2)).is_empty());
    assert!(ix.outgoing(nid(1)).is_empty());
}

#[test]
fn index_works_as_render_resolver() {
    let mut ix = Index::new();
    ix.upsert(&note(2, "Target", "t", &[], ""));
    let html = crate::render_html("[[Target]] [[none]]", &crate::RenderOptions::default(), &ix);
    assert!(html.contains("nuvoletta://note/"));
    assert!(html.contains("unresolved"));
}

#[test]
fn tags_are_matched_normalised_and_listed_with_display_form() {
    let mut ix = Index::new();
    ix.upsert(&note(1, "A", "a", &["Rust", "Écrit"], ""));
    ix.upsert(&note(2, "B", "b", &["rust", "e\u{301}crit"], ""));
    ix.upsert(&note(3, "C", "c", &["Other"], ""));
    assert_eq!(ix.notes_with_tag("RUST"), vec![nid(1), nid(2)]);
    assert_eq!(ix.notes_with_tag("écrit"), vec![nid(1), nid(2)]);
    assert!(ix.notes_with_tag("missing").is_empty());
    let tags = ix.tags();
    assert_eq!(tags.len(), 3);
    assert_eq!(
        tags[0],
        TagCount {
            tag: "Other".into(),
            count: 1
        }
    );
    assert_eq!(tags[2].count, 2);
    assert_eq!(
        tags[1],
        TagCount {
            tag: "Rust".into(),
            count: 2
        }
    );
    ix.remove(nid(1));
    assert_eq!(ix.notes_with_tag("rust"), vec![nid(2)]);
}

#[test]
fn search_is_and_with_prefix_on_last_term_and_ranks_title_first() {
    let mut ix = Index::new();
    ix.upsert(&note(
        1,
        "Groceries",
        "home/list",
        &[],
        "milk and eggs, plus fresh bread",
    ));
    ix.upsert(&note(
        2,
        "Bread",
        "home/baking",
        &["food"],
        "flour and yeast",
    ));
    ix.upsert(&note(3, "Misc", "misc", &[], "I like bread a lot"));
    let hits = ix.search("bre", 10);
    assert_eq!(
        hits.iter().map(|h| h.id).collect::<Vec<_>>(),
        vec![nid(2), nid(1), nid(3)]
    );
    assert!(hits[0].snippet.is_none());
    assert!(hits[1].snippet.as_deref().unwrap().contains("fresh bread"));

    assert_eq!(ix.search("fresh brea", 10).len(), 1);
    assert!(
        ix.search("brea fresh", 10).is_empty(),
        "only the last term is a prefix"
    );
    assert_eq!(ix.search("FOOD", 10)[0].id, nid(2));
    assert_eq!(ix.search("baking", 10)[0].id, nid(2), "path is searchable");
    assert!(ix.search("   ", 10).is_empty());
    assert_eq!(ix.search("bread", 1).len(), 1);
    ix.remove(nid(2));
    assert_eq!(ix.search("bread", 10).len(), 2);
}

#[test]
fn search_is_unicode_aware() {
    let mut ix = Index::new();
    ix.upsert(&note(1, "Café", "a", &[], "Straße, naïve; ÉCOLE"));
    assert_eq!(ix.search("cafe\u{301}", 10).len(), 1);
    assert_eq!(ix.search("café", 10).len(), 1);
    assert_eq!(ix.search("école", 10).len(), 1);
    assert_eq!(ix.search("straße naï", 10).len(), 1);
}

#[test]
fn debug_output_hides_content() {
    let mut ix = Index::new();
    ix.upsert(&note(1, "secret", "secret", &["secret"], "secret"));
    assert!(!format!("{ix:?}").contains("secret"));
}

fn corpus(n: usize) -> Vec<Note> {
    (0..n)
        .map(|i| {
            let id = {
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&(i as u64).to_be_bytes());
                NoteId::from_bytes(b)
            };
            Note {
                id,
                title: format!("Note number {i}"),
                path: NotePath::parse(&format!("folder{}/note{i}", i % 50)).unwrap(),
                tags: vec![format!("tag{}", i % 20)],
                created_at: 0,
                modified_at: 0,
                body: format!(
                    "Body of note {i} mentions [[Note number {}]] and the word lorem{} with ipsum dolor sit amet.",
                    (i + 1) % n,
                    i % 100
                ),
            }
        })
        .collect()
}

fn exercise(n: usize, search_budget_ms: u128) {
    let notes = corpus(n);
    let t = Instant::now();
    let ix = Index::from_notes(&notes);
    let build = t.elapsed();
    assert_eq!(ix.len(), n);
    assert!(ix.unresolved().is_empty());
    let t = Instant::now();
    for q in ["lorem42", "note number 7", "ipsum dol", "folder3"] {
        assert!(!ix.search(q, 20).is_empty(), "{q}");
    }
    let per_query = t.elapsed().as_millis() / 4;
    let t = Instant::now();
    let _ = ix.backlinks(notes[0].id);
    let _ = ix.unresolved();
    eprintln!(
        "{n} notes: build {build:?}, ~{per_query} ms/query, links {:?}",
        t.elapsed()
    );
    assert!(
        per_query <= search_budget_ms,
        "search too slow: {per_query} ms"
    );
}

#[test]
fn moderate_corpus_builds_and_searches_quickly() {
    exercise(1000, 500);
}

#[test]
#[ignore = "timing check for ~10k notes; run with --release"]
fn ten_thousand_notes_search_is_fast() {
    exercise(10_000, 50);
}

// ---- property tests ----

const TITLES: &[&str] = &[
    "Alpha",
    "alpha",
    "Beta",
    "é",
    "e\u{301}",
    "Gamma Delta",
    "a/b",
    "",
];
const PATHS: &[&str] = &[
    "a",
    "A",
    "a/b",
    "b/alpha",
    "x/y/z",
    "é",
    "e\u{301}",
    "beta",
    "gamma delta",
];
const FRAGS: &[&str] = &[
    "[[alpha]]",
    "[[a/b]]",
    "[[Beta|x]]",
    "[[é]]",
    "[[/gamma delta/]]",
    "[[nothing#h]]",
    "`[[code]]` ",
    "text ",
    "Word ",
];
const TAGS: &[&str] = &["Rust", "rust", "Écrit", "x", ""];

#[derive(Debug, Clone)]
enum Op {
    Upsert(u8, usize, usize, Vec<usize>, Vec<usize>),
    Remove(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (
            0u8..6,
            0..TITLES.len(),
            0..PATHS.len(),
            prop::collection::vec(0..TAGS.len(), 0..3),
            prop::collection::vec(0..FRAGS.len(), 0..6)
        )
            .prop_map(|(i, t, p, tg, b)| Op::Upsert(i, t, p, tg, b)),
        1 => (0u8..6).prop_map(Op::Remove),
    ]
}

fn build_note(i: u8, t: usize, p: usize, tg: &[usize], b: &[usize]) -> Note {
    let tags: Vec<&str> = tg.iter().map(|&k| TAGS[k]).collect();
    let body: String = b.iter().map(|&k| FRAGS[k]).collect();
    note(i, TITLES[t], PATHS[p], &tags, &body)
}

fn observe(ix: &Index) -> impl PartialEq + fmt::Debug {
    let ids: Vec<NoteId> = (0..6).map(nid).collect();
    (
        ids.iter().map(|&i| ix.outgoing(i)).collect::<Vec<_>>(),
        ids.iter().map(|&i| ix.backlinks(i)).collect::<Vec<_>>(),
        ix.unresolved(),
        ix.ambiguous(),
        ix.tags(),
        ix.notes_with_tag("rust"),
        ["alpha", "word", "gamma d", "rust"].map(|q| ix.search(q, 10)),
    )
}

proptest! {
    #[test]
    fn incremental_index_equals_rebuilt_index(ops in prop::collection::vec(op(), 0..40)) {
        let mut ix = Index::new();
        let mut state: BTreeMap<u8, Note> = BTreeMap::new();
        for o in &ops {
            match o {
                Op::Upsert(i, t, p, tg, b) => {
                    let n = build_note(*i, *t, *p, tg, b);
                    ix.upsert(&n);
                    state.insert(*i, n);
                }
                Op::Remove(i) => {
                    prop_assert_eq!(ix.remove(nid(*i)), state.remove(i).is_some());
                }
            }
        }
        let rebuilt = Index::from_notes(state.values());
        prop_assert!(ix == rebuilt, "internal state differs");
        prop_assert_eq!(observe(&ix), observe(&rebuilt));
        let mut again = Index::new();
        again.upsert(&note(9, "x", "x", &[], "[[y]]"));
        again.rebuild(state.values());
        prop_assert!(again == rebuilt);
    }

    #[test]
    fn index_resolution_equals_resolve(
        meta in prop::collection::vec(
            ("[a-cA-C \u{e9}\u{301}]{0,4}", "[a-cA-C\u{e9}\u{301}]{1,2}(/[a-cA-C]{1,2}){0,2}"),
            0..8
        ),
        targets in prop::collection::vec("[/ ]{0,2}[a-cA-C \u{e9}\u{301}/]{0,7}[/ ]{0,2}", 1..8),
    ) {
        // Ids ascend with position, so candidate order is id order, the index's tie-break.
        let notes: Vec<Note> = meta
            .iter()
            .enumerate()
            .map(|(i, (t, p))| note(u8::try_from(i).unwrap(), t, p, &[], ""))
            .collect();
        let ix = Index::from_notes(&notes);
        for t in &targets {
            let expected = resolve(t, notes.iter().map(Candidate::from));
            prop_assert_eq!(ix.resolve_target(t), expected, "target {:?}", t);
        }
    }
}
