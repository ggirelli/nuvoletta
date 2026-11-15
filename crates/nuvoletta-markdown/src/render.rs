use crate::links::split_dest;
use crate::{Resolution, dialect};
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Parser, Tag, TagEnd,
};
use std::fmt::Write;

/// Resolves a wikilink target (without fragment) to a note.
///
/// Implemented for any `Fn(&str) -> Resolution`, so callers can pass a closure over their index.
pub trait Resolver {
    /// Resolve `target`.
    fn resolve(&self, target: &str) -> Resolution;
}

impl<F: Fn(&str) -> Resolution> Resolver for F {
    fn resolve(&self, target: &str) -> Resolution {
        self(target)
    }
}

/// Rendering options. The default is the safest configuration.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    /// Render `http`/`https` images as `<img>` (which makes the webview fetch them). Off by
    /// default: images are replaced by a placeholder with their alt text.
    pub allow_remote_images: bool,
}

/// Escape text for HTML text and double-quoted attribute contexts.
fn esc(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Return a URL safe to put in `href`/`src` if its scheme is allowed, else `None`.
///
/// Decoding of entities and escapes is done by the Markdown parser before this runs. Browsers
/// ignore ASCII tab, newline and other control characters inside URLs, so they are removed first
/// and the scheme is checked on what remains. Relative URLs are not allowed (there is no base).
fn safe_url(raw: &str, schemes: &[&str]) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == ' ' { '\u{0}' } else { c })
        .collect::<String>()
        .replace('\u{0}', "%20");
    let (scheme, rest) = cleaned.split_once(':')?;
    if rest.is_empty()
        || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    // `https:foo` and `https:/foo` are not absolute: a browser resolves them against the page's own
    // origin when that is also https. Web URLs must have an authority.
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        let authority = rest.strip_prefix("//")?;
        if authority.is_empty() || authority.starts_with(['/', '?', '#']) {
            return None;
        }
    }
    schemes
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
        .then_some(cleaned)
}

const LINK_SCHEMES: &[&str] = &["http", "https", "mailto"];
const IMAGE_SCHEMES: &[&str] = &["http", "https"];

/// Keep only characters that are safe inside an id or class token.
fn token(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// Percent-encode everything but unreserved characters.
fn pct(out: &mut String, s: &str) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

const fn heading(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Render a note body to HTML that is safe to inject into the preview.
///
/// Raw HTML is escaped. Links are limited to `http`, `https`, `mailto` and wikilinks
/// (`nuvoletta://note/<id>[#fragment]`, class `wikilink`; unresolved and ambiguous wikilinks are
/// non-navigable `<span class="wikilink unresolved|ambiguous">`). Any other link is rendered as
/// `<span class="link-blocked">` with its text. Images follow [`RenderOptions`]. No `style`
/// or event-handler attribute is ever emitted.
#[must_use]
pub fn render_html(body: &str, opts: &RenderOptions, resolver: &dyn Resolver) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    // Closing tag for each open link, in order.
    let mut link_ends: Vec<&'static str> = Vec::new();
    let mut in_head = false;
    let mut aligns: Vec<Alignment> = Vec::new();
    let mut col = 0usize;
    let mut events = Parser::new_ext(body, dialect());
    while let Some(ev) = events.next() {
        match ev {
            Event::Text(t) => esc(&mut out, &t),
            Event::Code(t) => {
                out.push_str("<code>");
                esc(&mut out, &t);
                out.push_str("</code>");
            }
            Event::Html(t) | Event::InlineHtml(t) => esc(&mut out, &t),
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("<br>\n"),
            Event::Rule => out.push_str("<hr>\n"),
            Event::FootnoteReference(l) => {
                let t = token(&l);
                let _ = write!(out, "<sup class=\"footnote-ref\"><a href=\"#fn-{t}\">");
                esc(&mut out, &l);
                out.push_str("</a></sup>");
            }
            Event::TaskListMarker(c) => {
                out.push_str("<input type=\"checkbox\" disabled");
                if c {
                    out.push_str(" checked");
                }
                out.push_str("> ");
            }
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    let _ = write!(out, "<h{}>", heading(level));
                }
                Tag::BlockQuote(_) => out.push_str("<blockquote>\n"),
                Tag::CodeBlock(kind) => {
                    out.push_str("<pre><code");
                    if let CodeBlockKind::Fenced(info) = kind {
                        let lang = info.split_whitespace().next().unwrap_or_default();
                        let lang: String = lang
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_'))
                            .collect();
                        if !lang.is_empty() {
                            let _ = write!(out, " class=\"language-{lang}\"");
                        }
                    }
                    out.push('>');
                }
                Tag::List(None) => out.push_str("<ul>\n"),
                Tag::List(Some(n)) => {
                    let _ = writeln!(out, "<ol start=\"{n}\">");
                }
                Tag::Item => out.push_str("<li>"),
                Tag::FootnoteDefinition(l) => {
                    let _ = write!(
                        out,
                        "<div class=\"footnote-definition\" id=\"fn-{}\">",
                        token(&l)
                    );
                }
                Tag::Table(a) => {
                    aligns = a;
                    out.push_str("<table>\n");
                }
                Tag::TableHead => {
                    in_head = true;
                    col = 0;
                    out.push_str("<thead><tr>");
                }
                Tag::TableRow => {
                    col = 0;
                    out.push_str("<tr>");
                }
                Tag::TableCell => {
                    let name = if in_head { "th" } else { "td" };
                    let class = match aligns.get(col) {
                        Some(Alignment::Left) => " class=\"align-left\"",
                        Some(Alignment::Center) => " class=\"align-center\"",
                        Some(Alignment::Right) => " class=\"align-right\"",
                        _ => "",
                    };
                    col += 1;
                    let _ = write!(out, "<{name}{class}>");
                }
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    ..
                } => {
                    if matches!(link_type, LinkType::WikiLink { .. }) {
                        link_ends.push(start_wikilink(&mut out, &dest_url, resolver));
                    } else if let Some(url) = safe_url(&dest_url, LINK_SCHEMES) {
                        out.push_str("<a href=\"");
                        esc(&mut out, &url);
                        out.push_str("\" rel=\"noopener noreferrer nofollow\"");
                        if !title.is_empty() {
                            out.push_str(" title=\"");
                            esc(&mut out, &title);
                            out.push('"');
                        }
                        out.push('>');
                        link_ends.push("</a>");
                    } else {
                        out.push_str("<span class=\"link-blocked\">");
                        link_ends.push("</span>");
                    }
                }
                Tag::Image {
                    dest_url, title, ..
                } => {
                    // Collect the alt text (plain) up to the matching end tag.
                    let mut alt = String::new();
                    let mut depth = 1usize;
                    for inner in events.by_ref() {
                        match inner {
                            Event::Start(Tag::Image { .. }) => depth += 1,
                            Event::End(TagEnd::Image) => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                            Event::SoftBreak | Event::HardBreak => alt.push(' '),
                            _ => {}
                        }
                    }
                    let url =
                        safe_url(&dest_url, IMAGE_SCHEMES).filter(|_| opts.allow_remote_images);
                    if let Some(url) = url {
                        out.push_str("<img src=\"");
                        esc(&mut out, &url);
                        out.push_str("\" alt=\"");
                        esc(&mut out, &alt);
                        out.push_str("\" loading=\"lazy\" referrerpolicy=\"no-referrer\"");
                        if !title.is_empty() {
                            out.push_str(" title=\"");
                            esc(&mut out, &title);
                            out.push('"');
                        }
                        out.push('>');
                    } else {
                        out.push_str("<span class=\"image-blocked\">");
                        esc(&mut out, &alt);
                        out.push_str("</span>");
                    }
                }
                // Not enabled in the dialect; ignore if ever produced.
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out.push_str("</p>\n"),
                TagEnd::Heading(level) => {
                    let _ = writeln!(out, "</h{}>", heading(level));
                }
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>\n"),
                TagEnd::CodeBlock => out.push_str("</code></pre>\n"),
                TagEnd::List(false) => out.push_str("</ul>\n"),
                TagEnd::List(true) => out.push_str("</ol>\n"),
                TagEnd::Item => out.push_str("</li>\n"),
                TagEnd::FootnoteDefinition => out.push_str("</div>\n"),
                TagEnd::Table => out.push_str("</tbody></table>\n"),
                TagEnd::TableHead => {
                    in_head = false;
                    out.push_str("</tr></thead><tbody>\n");
                }
                TagEnd::TableRow => out.push_str("</tr>\n"),
                TagEnd::TableCell => out.push_str(if in_head { "</th>" } else { "</td>" }),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Link => out.push_str(link_ends.pop().unwrap_or_default()),
                _ => {}
            },
            // Other events are not produced by the dialect.
            _ => {}
        }
    }
    out
}

/// Open the element for a wikilink and return its closing tag.
fn start_wikilink(out: &mut String, dest: &str, resolver: &dyn Resolver) -> &'static str {
    let (target, fragment) = split_dest(dest);
    match resolver.resolve(&target) {
        Resolution::Resolved(id) if !target.is_empty() => {
            let _ = write!(out, "<a href=\"nuvoletta://note/{id}");
            if let Some(f) = fragment {
                out.push('#');
                pct(out, &f);
            }
            out.push_str("\" class=\"wikilink\">");
            "</a>"
        }
        Resolution::Ambiguous(_) => {
            out.push_str("<span class=\"wikilink ambiguous\">");
            "</span>"
        }
        _ => {
            out.push_str("<span class=\"wikilink unresolved\">");
            "</span>"
        }
    }
}
