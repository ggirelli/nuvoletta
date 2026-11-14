//! Wikilink parsing and resolution.

use nuvoletta_core::{Note, NoteId};
use nuvoletta_markdown::{Candidate, Resolution, markdown_links, parse_links, resolve};

fn one(body: &str) -> nuvoletta_markdown::WikiLink {
    let mut v = parse_links(body);
    assert_eq!(v.len(), 1, "{body:?} -> {v:?}");
    v.remove(0)
}

#[test]
fn plain_alias_and_fragment() {
    let l = one("see [[Target]] here");
    assert_eq!(
        (l.target.as_str(), l.alias, l.fragment),
        ("Target", None, None)
    );
    assert_eq!(&"see [[Target]] here"[l.range], "[[Target]]");

    let l = one("[[Target|the alias]]");
    assert_eq!(l.alias.as_deref(), Some("the alias"));

    let l = one("[[ Target # Head ing | al ]]");
    assert_eq!(l.target, "Target");
    assert_eq!(l.fragment.as_deref(), Some("Head ing"));
    assert_eq!(l.alias.as_deref(), Some("al"));
}

#[test]
fn several_in_order() {
    let v = parse_links("[[A]] and [[B|b]]\n\n- [[C#x]]");
    let t: Vec<_> = v.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(t, ["A", "B", "C"]);
}

#[test]
fn not_links_in_code() {
    assert!(parse_links("`[[A]]`").is_empty());
    assert!(parse_links("```\n[[A]]\n```").is_empty());
    assert!(parse_links("    [[A]]").is_empty());
    assert!(parse_links("<span>[[A]]</span> `x`").len() <= 1);
}

#[test]
fn escaped_and_malformed() {
    assert!(parse_links(r"\[[A]]").is_empty());
    assert!(parse_links("[[A").is_empty());
    assert!(parse_links("[[]]").is_empty());
    assert!(parse_links("[[ ]]").is_empty());
    assert!(parse_links("[[|alias]]").is_empty());
    assert!(parse_links("[A]]").is_empty());
    // Must not panic whatever the nesting.
    let _ = parse_links("[[[[A]]]] [[a[[b]]c]] [[A]]]] [[[A]]");
    let _ = parse_links(&"[[".repeat(10_000));
}

#[test]
fn unicode_is_carried_verbatim() {
    let l = one("[[Cafe\u{301}|\u{1F600}]]");
    assert_eq!(l.target, "Cafe\u{301}");
    assert_eq!(l.alias.as_deref(), Some("\u{1F600}"));
}

#[test]
fn standard_links() {
    let v = markdown_links(
        "[a](http://x) <https://y> ![img](http://i) [[W]] [r][1]\n\n[1]: mailto:a@b",
    );
    let d: Vec<_> = v.iter().map(|l| l.dest.as_str()).collect();
    assert_eq!(d, ["http://x", "https://y", "mailto:a@b"]);
}

struct Fixture(Vec<Note>);

impl Fixture {
    fn new(notes: &[(&str, &str)]) -> Self {
        Self(
            notes
                .iter()
                .map(|(title, path)| Note::new(title, path, vec![], "").unwrap())
                .collect(),
        )
    }
    fn resolve(&self, t: &str) -> Resolution {
        resolve(t, self.0.iter().map(Candidate::from))
    }
    fn id(&self, i: usize) -> NoteId {
        self.0[i].id
    }
}

#[test]
fn resolve_by_path_segment_and_title() {
    let f = Fixture::new(&[("First", "work/projects/alpha"), ("Second", "home/recipes")]);
    assert_eq!(
        f.resolve("work/projects/alpha"),
        Resolution::Resolved(f.id(0))
    );
    assert_eq!(f.resolve("alpha"), Resolution::Resolved(f.id(0)));
    assert_eq!(f.resolve("Second"), Resolution::Resolved(f.id(1)));
    assert_eq!(f.resolve("nothing"), Resolution::Unresolved);
    assert_eq!(f.resolve("  "), Resolution::Unresolved);
    assert_eq!(f.resolve(""), Resolution::Unresolved);
}

#[test]
fn resolve_case_whitespace_and_nfc() {
    let f = Fixture::new(&[("Caf\u{e9}", "x/Caf\u{e9}s")]);
    assert_eq!(f.resolve("  CAFE\u{301}  "), Resolution::Resolved(f.id(0)));
    assert_eq!(f.resolve("X/CAF\u{c9}S"), Resolution::Resolved(f.id(0)));
    assert_eq!(f.resolve("/x/cafés/"), Resolution::Resolved(f.id(0)));
}

#[test]
fn resolve_ambiguity_and_path_priority() {
    let f = Fixture::new(&[
        ("A", "one/todo"),
        ("B", "two/todo"),
        ("todo", "three/other"),
    ]);
    assert_eq!(
        f.resolve("todo"),
        Resolution::Ambiguous(vec![f.id(0), f.id(1), f.id(2)])
    );
    assert_eq!(f.resolve("two/todo"), Resolution::Resolved(f.id(1)));
    // A note matching by both segment and title counts once.
    let g = Fixture::new(&[("todo", "a/todo")]);
    assert_eq!(g.resolve("TODO"), Resolution::Resolved(g.id(0)));
}

#[test]
fn full_path_beats_segment_match() {
    let f = Fixture::new(&[("x", "a"), ("y", "b/a")]);
    assert_eq!(f.resolve("a"), Resolution::Resolved(f.id(0)));
}
