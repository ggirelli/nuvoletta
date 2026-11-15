//! Wikilink and Markdown link extraction, and target resolution.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_core::{NoteId, NotePath};
use nuvoletta_markdown::{Candidate, Resolution, markdown_links, parse_links, resolve};

fuzz_target!(|data: &[u8]| {
    let Ok(body) = std::str::from_utf8(data) else {
        return;
    };
    let paths: Vec<NotePath> = ["a", "a/b", "b/a", "Café/ñ"]
        .iter()
        .map(|p| NotePath::parse(p).expect("valid path"))
        .collect();
    let cands: Vec<Candidate> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| Candidate {
            id: NoteId::from_bytes([i as u8; 16]),
            path,
            title: "A title",
        })
        .collect();
    let mut last_end = 0;
    for l in parse_links(body) {
        // Ranges are on character boundaries, in document order, and enclose a `[[`.
        let text = body.get(l.range.clone()).expect("range on char boundaries");
        assert!(text.starts_with("[[") && text.ends_with("]]"), "{text:?}");
        assert!(l.range.start >= last_end.min(l.range.start));
        last_end = l.range.end;
        assert!(!l.target.is_empty() && l.target == l.target.trim());
        assert!(l.fragment.as_deref().is_none_or(|f| !f.is_empty()));
        match resolve(&l.target, cands.iter().copied()) {
            Resolution::Ambiguous(ids) => assert!(ids.len() > 1),
            Resolution::Resolved(_) | Resolution::Unresolved => {}
        }
    }
    for l in markdown_links(body) {
        assert!(body.get(l.range).is_some());
    }
    // Resolution over arbitrary targets, without going through the parser.
    let _ = resolve(body, cands.iter().copied());
});
