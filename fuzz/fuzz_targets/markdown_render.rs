//! Markdown renderer and sanitizer. Invariant: whatever the note says, the HTML contains only the
//! tags and attributes the renderer emits on purpose, and no script, event handler, style or
//! unsafe URL.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_core::NoteId;
use nuvoletta_markdown::{RenderOptions, Resolution, render_html};

const TAGS: &[&str] = &[
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "code",
    "ul",
    "ol",
    "li",
    "div",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "em",
    "strong",
    "del",
    "a",
    "span",
    "img",
    "br",
    "hr",
    "sup",
    "input",
];
const ATTRS: &[&str] = &[
    "href",
    "src",
    "alt",
    "title",
    "class",
    "id",
    "rel",
    "loading",
    "referrerpolicy",
    "start",
    "type",
    "checked",
    "disabled",
];

/// Walk the output as the HTML parser would, checking every tag.
fn check(html: &str, remote_images: bool) {
    let b = html.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // Text is always escaped: a raw `>` or quote outside a tag means a bug.
            b'>' => panic!("stray > in {html:?}"),
            b'<' => {
                i += 1;
                let close = b.get(i) == Some(&b'/');
                if close {
                    i += 1;
                }
                let start = i;
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let name = &html[start..i];
                assert!(TAGS.contains(&name), "tag {name:?} in {html:?}");
                if close {
                    assert_eq!(b.get(i), Some(&b'>'));
                    i += 1;
                    continue;
                }
                loop {
                    while b.get(i) == Some(&b' ') {
                        i += 1;
                    }
                    match b.get(i) {
                        Some(b'>') => {
                            i += 1;
                            break;
                        }
                        Some(_) => {}
                        None => panic!("unterminated tag in {html:?}"),
                    }
                    let s = i;
                    while i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'-') {
                        i += 1;
                    }
                    let attr = &html[s..i];
                    assert!(ATTRS.contains(&attr), "attribute {attr:?} in {html:?}");
                    if b.get(i) == Some(&b'=') {
                        assert_eq!(b.get(i + 1), Some(&b'"'), "unquoted value in {html:?}");
                        i += 2;
                        let s = i;
                        while i < b.len() && b[i] != b'"' {
                            assert!(b[i] != b'<' && b[i] != b'>', "raw angle in attribute");
                            i += 1;
                        }
                        assert!(i < b.len(), "unterminated attribute in {html:?}");
                        let val = &html[s..i];
                        i += 1;
                        if attr == "href" || attr == "src" {
                            let v = val.to_ascii_lowercase();
                            let ok = v.starts_with("https://")
                                || v.starts_with("http://")
                                || v.starts_with("mailto:")
                                || v.starts_with("nuvoletta://note/")
                                || v.starts_with("#fn-");
                            assert!(ok, "unsafe {attr} {val:?}");
                            assert!(!val.chars().any(char::is_control));
                            if attr == "src" {
                                assert!(remote_images, "image emitted while disabled");
                                assert_eq!(name, "img");
                            }
                        }
                    }
                }
            }
            _ => i += 1,
        }
    }
    assert!(!html.to_ascii_lowercase().contains("<script"));
}

fuzz_target!(|data: &[u8]| {
    let Ok(body) = std::str::from_utf8(data) else {
        return;
    };
    for remote in [false, true] {
        let opts = RenderOptions {
            allow_remote_images: remote,
        };
        let html = render_html(body, &opts, &|t: &str| {
            // A deterministic resolver covering all three outcomes.
            match t.len() % 3 {
                0 => Resolution::Unresolved,
                1 => Resolution::Resolved(NoteId::from_bytes([t.len() as u8; 16])),
                _ => Resolution::Ambiguous(vec![NoteId::from_bytes([1; 16])]),
            }
        });
        check(&html, remote);
    }
});
