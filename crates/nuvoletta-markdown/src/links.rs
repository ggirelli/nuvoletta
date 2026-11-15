use crate::dialect;
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};
use std::ops::Range;

/// A wikilink found in a note body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    /// The target as written, without alias or fragment, trimmed.
    pub target: String,
    /// The display alias of `[[Target|alias]]`, if any.
    pub alias: Option<String>,
    /// The heading fragment of `[[Target#Heading]]`, if any. Carried along, not resolved.
    pub fragment: Option<String>,
    /// Byte range of the whole `[[...]]` in the body.
    pub range: Range<usize>,
}

/// A standard Markdown or autolink (not an image, not a wikilink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownLink {
    /// The destination as parsed (entities and escapes decoded).
    pub dest: String,
    /// Byte range of the whole link in the body.
    pub range: Range<usize>,
}

/// Split a wikilink destination into trimmed target and optional fragment.
pub(crate) fn split_dest(dest: &str) -> (String, Option<String>) {
    let (t, f) = match dest.split_once('#') {
        Some((t, f)) => (t, Some(f.trim())),
        None => (dest, None),
    };
    (
        t.trim().to_owned(),
        f.filter(|f| !f.is_empty()).map(str::to_owned),
    )
}

/// Find all wikilinks in `body`, in document order.
///
/// Wikilinks inside code spans, code blocks and raw HTML are not links; escaped brackets
/// (`\[[x]]`) are not links. Links with an empty target are ignored.
#[must_use]
pub fn parse_links(body: &str) -> Vec<WikiLink> {
    let mut out = Vec::new();
    if !body.contains("[[") {
        return out; // no wikilink can exist: skip the Markdown parse
    }
    let mut iter = Parser::new_ext(body, dialect()).into_offset_iter();
    while let Some((ev, range)) = iter.next() {
        let Event::Start(Tag::Link {
            link_type: LinkType::WikiLink { has_pothole },
            dest_url,
            ..
        }) = ev
        else {
            continue;
        };
        let mut alias = String::new();
        for (inner, _) in iter.by_ref() {
            match inner {
                Event::End(TagEnd::Link) => break,
                Event::Text(t) | Event::Code(t) => alias.push_str(&t),
                _ => {}
            }
        }
        let (target, fragment) = split_dest(&dest_url);
        if target.is_empty() {
            continue;
        }
        out.push(WikiLink {
            target,
            alias: has_pothole
                .then(|| alias.trim().to_owned())
                .filter(|a| !a.is_empty()),
            fragment,
            range,
        });
    }
    out
}

/// Find all standard links (inline, reference, autolinks) in `body`, in document order.
#[must_use]
pub fn markdown_links(body: &str) -> Vec<MarkdownLink> {
    Parser::new_ext(body, dialect())
        .into_offset_iter()
        .filter_map(|(ev, range)| match ev {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) if !matches!(link_type, LinkType::WikiLink { .. }) => Some(MarkdownLink {
                dest: dest_url.into_string(),
                range,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod fast_path_tests {
    use super::parse_links;

    #[test]
    fn text_without_double_brackets_has_no_wikilinks() {
        assert!(parse_links("# Title\n\nplain [x](y) text [single]").is_empty());
        assert_eq!(parse_links("see [[Other]]").len(), 1);
    }
}
