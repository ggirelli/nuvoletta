//! Rendering and malicious-input tests.

use nuvoletta_core::NoteId;
use nuvoletta_markdown::{RenderOptions, Resolution, render_html};
use proptest::prelude::*;

fn id() -> NoteId {
    NoteId::from_bytes([0xab; 16])
}

fn resolver(t: &str) -> Resolution {
    match t {
        "Known" => Resolution::Resolved(id()),
        "Dup" => Resolution::Ambiguous(vec![id(), id()]),
        _ => Resolution::Unresolved,
    }
}

fn render(s: &str) -> String {
    render_html(s, &RenderOptions::default(), &resolver)
}

fn render_images(s: &str) -> String {
    let o = RenderOptions {
        allow_remote_images: true,
    };
    render_html(s, &o, &resolver)
}

/// Invariants that must hold for any output.
fn check_safe(html: &str) {
    let l = html.to_ascii_lowercase();
    assert!(!l.contains("<script"), "{html}");
    assert!(
        !l.contains("<iframe") && !l.contains("<img ") && !l.contains("<style"),
        "{html}"
    );
    for bad in ["javascript:", "vbscript:", "data:", "file:"] {
        for attr in ["href=\"", "src=\""] {
            assert!(!l.contains(&format!("{attr}{bad}")), "{html}");
        }
    }
    // No emitted tag may carry an event-handler or style attribute.
    for t in l.split('<').skip(1) {
        let tag = t.split('>').next().unwrap_or_default();
        for a in tag.split_whitespace().skip(1) {
            assert!(!a.starts_with("on") && !a.starts_with("style"), "{html}");
        }
    }
}

#[test]
fn basics() {
    let h = render(
        "# T\n\n*a* **b** ~~c~~ `d`\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\nnote[^1]\n\n[^1]: fn\n",
    );
    for s in [
        "<h1>T</h1>",
        "<em>a</em>",
        "<strong>b</strong>",
        "<del>c</del>",
        "<code>d</code>",
        "checked",
        "<table>",
        "align-center",
        "footnote-ref",
        "footnote-definition",
    ] {
        assert!(h.contains(s), "missing {s} in {h}");
    }
}

#[test]
fn raw_html_is_escaped() {
    for s in [
        "<script>alert(1)</script>",
        "text <img src=x onerror=alert(1)> text",
        "<div onclick=\"x()\">hi</div>",
        "<a href=\"javascript:alert(1)\">x</a>",
        "<svg/onload=alert(1)>",
        "<style>*{}</style>",
        "<iframe src=//evil></iframe>",
        "<scr<script>ipt>x",
        "<!-- c --><b",
    ] {
        let h = render(s);
        for t in ["<script", "<svg", "<div", "<img", "<iframe", "<style"] {
            assert!(!h.contains(t), "{h}");
        }
        check_safe(&h);
    }
    assert!(render("<script>x</script>").contains("&lt;script&gt;"));
}

#[test]
fn dangerous_schemes_neutralised() {
    for url in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        " javascript:alert(1)",
        "java\tscript:alert(1)",
        "java&#x0A;script:alert(1)",
        "&#106;avascript:alert(1)",
        "&#x6A;&#x61;vascript&colon;alert(1)",
        "jav&#x09;ascript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD4=",
        "vbscript:msgbox",
        "file:///etc/passwd",
        "/relative/path",
        "//evil.example/x",
        "#frag",
        "mailto:",
        "ht tp://x",
        "\u{1}javascript:x",
    ] {
        for md in [
            format!("[x](<{url}>)"),
            format!("[x]({url})"),
            format!("[x][r]\n\n[r]: <{url}>"),
            format!("<{url}>"),
        ] {
            let h = render(&md);
            check_safe(&h);
            assert!(!h.contains("<a "), "{md:?} -> {h}");
        }
    }
}

#[test]
fn allowed_schemes_kept() {
    let h = render(
        "[a](http://x.test/?q=1&r=\"2\") [b](HTTPS://x) [c](mailto:a@b.test) <https://auto.test>",
    );
    assert_eq!(h.matches("<a href=").count(), 4, "{h}");
    assert!(h.contains("&amp;r=&quot;2&quot;"));
    assert!(h.contains("rel=\"noopener noreferrer nofollow\""));
}

#[test]
fn attribute_breakout_is_escaped() {
    let h = render("[x](http://a\"onmouseover=\"alert(1) \"t\")");
    check_safe(&h);
    let h = render("[x](http://a \"ti\\\"tle onclick=alert(1)\")");
    assert!(!h.contains("\" onclick"), "{h}");
}

#[test]
fn images_blocked_by_default() {
    let h = render("![alt <b>](http://x.test/a.png \"t\") ![z](javascript:x)");
    assert!(!h.contains("<img"), "{h}");
    assert!(h.contains("image-blocked") && h.contains("alt "));
    assert!(!h.contains("http://x.test"));
}

#[test]
fn images_opt_in_http_only() {
    let h = render_images(
        "![a\"b](https://x.test/a.png) ![c](data:image/png;base64,AAAA) ![d](javascript:x) ![e](/rel.png)",
    );
    assert_eq!(h.matches("<img ").count(), 1, "{h}");
    assert!(h.contains("alt=\"a&quot;b\""));
    assert!(h.contains("referrerpolicy=\"no-referrer\""));
    check_safe(&h.replace("<img ", "<im "));
}

#[test]
fn wikilinks_render() {
    let h = render("[[Known]] [[Known#Some Head|al\"ias]] [[Dup]] [[Nope|x]] `[[Known]]`");
    assert!(
        h.contains(&format!(
            "<a href=\"nuvoletta://note/{}\" class=\"wikilink\">Known</a>",
            id()
        )),
        "{h}"
    );
    assert!(
        h.contains("#Some%20Head\" class=\"wikilink\">al&quot;ias</a>"),
        "{h}"
    );
    assert!(h.contains("<span class=\"wikilink ambiguous\">Dup</span>"));
    assert!(h.contains("<span class=\"wikilink unresolved\">x</span>"));
    assert!(h.contains("<code>[[Known]]</code>"));
}

#[test]
fn code_blocks_and_language() {
    let h = render("```rust\" onclick=\"x\n<b>&\n```");
    assert!(
        h.contains("class=\"language-rustonclickx\"") || h.contains("class=\"language-rust\""),
        "{h}"
    );
    assert!(h.contains("&lt;b&gt;&amp;"));
    check_safe(&h);
}

#[test]
fn degenerate_inputs_do_not_panic_or_blow_up() {
    let big = [
        ">".repeat(50_000),
        "[".repeat(100_000),
        "[[".repeat(50_000),
        "*".repeat(100_000),
        "<".repeat(100_000),
        "- ".repeat(20_000) + "x",
        "![".repeat(30_000),
        "`".repeat(100_000),
        "[a](".repeat(20_000),
        "<a href=\"".repeat(20_000),
        "| a |\n|---|\n".repeat(5_000),
    ];
    for s in &big {
        let start = std::time::Instant::now();
        let h = render(s);
        check_safe(&h);
        assert!(start.elapsed().as_secs() < 10, "slow input: {}", &s[..8]);
    }
}

proptest! {
    #[test]
    fn never_unsafe(s in "[ -~\\n\\t\u{e9}\u{1F600}]{0,400}") {
        check_safe(&render(&s));
        check_safe(&render_images(&s).replace("<img ", "<im "));
    }

    #[test]
    fn never_unsafe_from_fragments(parts in proptest::collection::vec(
        prop::sample::select(vec![
            "<script>", "</", "[", "]", "(", ")", "[[", "]]", "!", "javascript:", "data:", "on", "=",
            "\"", "'", "<", ">", "&#106;", "&colon;", "`", "\n", "http://x", " ", "<a href=", "#", "|",
        ]), 0..60)) {
        let s: String = parts.concat();
        check_safe(&render(&s));
        check_safe(&render_images(&s).replace("<img ", "<im "));
    }
}
