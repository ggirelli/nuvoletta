//! Emitter and parser for the fixed front matter of exported notes.
//!
//! This is a small, strict subset of YAML: `key: value` lines between two `---` lines. Values are
//! double-quoted strings (written by the emitter), single-quoted or plain scalars, flow lists
//! (`[a, "b"]`) or block lists (`- a`). Anything else is reported as malformed.

use super::timestamp;
use nuvoletta_core::{Note, NoteId};

/// Front matter fields found in a file. Absent keys are `None`.
#[derive(Debug, Default)]
pub(crate) struct FrontMatter {
    pub id: Option<NoteId>,
    pub title: Option<String>,
    pub path: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created: Option<i64>,
    pub modified: Option<i64>,
    /// Keys that are not part of the format; ignored, but reported.
    pub unknown_keys: Vec<String>,
}

/// Result of looking for a front matter block at the start of a file.
pub(crate) enum Split<'a> {
    /// The text does not start with a complete front matter block; all of it is body.
    None,
    /// A block: its inner lines and the body after the closing line, verbatim.
    Block { front: &'a str, body: &'a str },
}

fn is_delimiter(line: &str) -> bool {
    line.trim_end_matches([' ', '\t', '\r']) == "---"
}

/// Split `text` into front matter and body. A leading BOM is ignored. An opening `---` without a
/// closing `---` line is a thematic break, not front matter.
pub(crate) fn split(text: &str) -> Split<'_> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some((first, rest)) = text.split_once('\n') else {
        return Split::None;
    };
    if !is_delimiter(first) {
        return Split::None;
    }
    let mut offset = 0;
    while offset < rest.len() {
        let end = rest[offset..].find('\n').map_or(rest.len(), |i| offset + i);
        if is_delimiter(&rest[offset..end]) {
            let body = rest.get(end + 1..).unwrap_or("");
            return Split::Block {
                front: &rest[..offset],
                body,
            };
        }
        offset = end + 1;
    }
    Split::None
}

/// Render the front matter block (including both delimiters) of `note`.
pub(crate) fn emit(note: &Note) -> String {
    let mut s = String::from("---\n");
    s.push_str(&format!("id: \"{}\"\n", note.id));
    s.push_str(&format!("title: {}\n", quote(&note.title)));
    s.push_str(&format!("path: {}\n", quote(note.path.as_str())));
    let tags: Vec<String> = note.tags.iter().map(|t| quote(t)).collect();
    s.push_str(&format!("tags: [{}]\n", tags.join(", ")));
    s.push_str(&format!(
        "created: {}\n",
        timestamp::format(note.created_at)
    ));
    s.push_str(&format!(
        "modified: {}\n",
        timestamp::format(note.modified_at)
    ));
    s.push_str("---\n");
    s
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                out.push_str(&format!("\\u{:04X}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

type Res<T> = Result<T, &'static str>;

fn double_quoted(s: &str) -> Res<(String, &str)> {
    let mut out = String::new();
    let mut it = s.char_indices().skip(1);
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Ok((out, &s[i + 1..])),
            '\\' => {
                let (_, e) = it.next().ok_or("unterminated string")?;
                let hex = |it: &mut dyn Iterator<Item = (usize, char)>, n: usize| -> Res<char> {
                    let mut v = 0u32;
                    for _ in 0..n {
                        let (_, h) = it.next().ok_or("bad escape")?;
                        v = v * 16 + h.to_digit(16).ok_or("bad escape")?;
                    }
                    char::from_u32(v).ok_or("bad escape")
                };
                out.push(match e {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '0' => '\0',
                    '\\' | '"' | '/' | ' ' => e,
                    'x' => hex(&mut it, 2)?,
                    'u' => hex(&mut it, 4)?,
                    'U' => hex(&mut it, 8)?,
                    _ => return Err("unsupported escape"),
                });
            }
            c => out.push(c),
        }
    }
    Err("unterminated string")
}

fn single_quoted(s: &str) -> Res<(String, &str)> {
    let mut out = String::new();
    let mut it = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = it.next() {
        if c == '\'' {
            if it.peek().is_some_and(|&(_, n)| n == '\'') {
                out.push('\'');
                it.next();
            } else {
                return Ok((out, &s[i + 1..]));
            }
        } else {
            out.push(c);
        }
    }
    Err("unterminated string")
}

fn only_comment(rest: &str) -> Res<()> {
    let r = rest.trim();
    if r.is_empty() || r.starts_with('#') {
        Ok(())
    } else {
        Err("unexpected text after value")
    }
}

/// Parse a whole scalar value (quoted or plain).
fn scalar(value: &str) -> Res<String> {
    let v = value.trim();
    if v.starts_with('"') {
        let (s, rest) = double_quoted(v)?;
        only_comment(rest)?;
        Ok(s)
    } else if v.starts_with('\'') {
        let (s, rest) = single_quoted(v)?;
        only_comment(rest)?;
        Ok(s)
    } else if v.starts_with(['[', '{', '|', '>']) {
        Err("unsupported value")
    } else {
        let v = v.find(" #").map_or(v, |i| &v[..i]);
        Ok(v.trim().to_owned())
    }
}

fn flow_list(value: &str) -> Res<Vec<String>> {
    let mut rest = value.trim().strip_prefix('[').ok_or("expected a list")?;
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix(']') {
            only_comment(r)?;
            return Ok(out);
        }
        let item = if rest.starts_with('"') {
            let (s, r) = double_quoted(rest)?;
            rest = r;
            s
        } else if rest.starts_with('\'') {
            let (s, r) = single_quoted(rest)?;
            rest = r;
            s
        } else {
            let end = rest.find([',', ']']).ok_or("unterminated list")?;
            let s = rest[..end].trim();
            if s.is_empty() || s.starts_with(['[', '{']) {
                return Err("bad list item");
            }
            rest = &rest[end..];
            s.to_owned()
        };
        out.push(item);
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix(',') {
            rest = r;
        } else if !rest.starts_with(']') {
            return Err("bad list");
        }
    }
}

fn tags(inline: &str, cont: &[&str]) -> Res<Vec<String>> {
    if cont.is_empty() {
        return if inline.trim().is_empty() || inline.trim() == "~" || inline.trim() == "null" {
            Ok(Vec::new())
        } else {
            flow_list(inline)
        };
    }
    if !inline.trim().is_empty() {
        return Err("bad list");
    }
    cont.iter()
        .map(|l| {
            let l = l.trim();
            let item = l.strip_prefix('-').ok_or("bad list")?;
            if !item.is_empty() && !item.starts_with([' ', '\t']) {
                return Err("bad list");
            }
            scalar(item)
        })
        .collect()
}

fn is_continuation(line: &str) -> bool {
    line.starts_with([' ', '\t']) || line == "-" || line.starts_with("- ")
}

/// Parse the inner lines of a front matter block.
///
/// # Errors
/// Returns a short, content-free reason if the block is not valid for this format.
pub(crate) fn parse(front: &str) -> Res<FrontMatter> {
    let mut entries: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for raw in front.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if is_continuation(line) {
            entries
                .last_mut()
                .ok_or("unexpected indentation")?
                .2
                .push(line);
            continue;
        }
        let (key, value) = line.split_once(':').ok_or("line is not key: value")?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("invalid key");
        }
        if !(value.is_empty() || value.starts_with([' ', '\t'])) {
            return Err("missing space after colon");
        }
        entries.push((key, value, Vec::new()));
    }
    let mut fm = FrontMatter::default();
    let mut seen = Vec::new();
    for (key, inline, cont) in entries {
        if seen.contains(&key) {
            return Err("duplicate key");
        }
        seen.push(key);
        let one = |inline: &str, cont: &[&str]| -> Res<String> {
            if cont.is_empty() {
                scalar(inline)
            } else {
                Err("multi-line value")
            }
        };
        match key {
            "id" => {
                fm.id = Some(one(inline, &cont)?.parse().map_err(|_| "invalid id")?);
            }
            "title" => fm.title = Some(one(inline, &cont)?),
            "path" => fm.path = Some(one(inline, &cont)?),
            "tags" => fm.tags = Some(tags(inline, &cont)?),
            "created" => {
                fm.created = Some(timestamp::parse(&one(inline, &cont)?).ok_or("invalid created")?);
            }
            "modified" => {
                fm.modified =
                    Some(timestamp::parse(&one(inline, &cont)?).ok_or("invalid modified")?);
            }
            other => fm.unknown_keys.push(other.to_owned()),
        }
    }
    Ok(fm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> (&str, &str) {
        match split(text) {
            Split::Block { front, body } => (front, body),
            Split::None => panic!("no block"),
        }
    }

    #[test]
    fn split_finds_the_first_closing_delimiter_and_keeps_the_body_verbatim() {
        let (f, b) = block("---\na: 1\n---\nbody\n---\nmore\n");
        assert_eq!(f, "a: 1\n");
        assert_eq!(b, "body\n---\nmore\n");
        assert_eq!(block("---\n---").1, "");
        assert_eq!(block("---\r\nx: 1\r\n---\r\nb").1, "b");
        assert_eq!(block("\u{feff}---\nx: 1\n---\nb").1, "b");
    }

    #[test]
    fn unclosed_or_absent_front_matter_is_not_a_block() {
        for t in ["", "text\n---\n", "---\nno end", "----\na: 1\n---\n", "---"] {
            assert!(matches!(split(t), Split::None), "{t:?}");
        }
    }

    #[test]
    fn emitted_front_matter_parses_back() {
        let mut n = Note::new(
            "Title: \"quoted\" # not a comment\nsecond\\line\t\u{85}\u{2028}",
            "a/b",
            vec!["x, y".into(), "[z]".into(), String::new(), "it's".into()],
            "",
        )
        .unwrap();
        n.created_at = -5;
        n.modified_at = i64::MAX;
        let text = emit(&n);
        let (front, body) = block(&text);
        assert_eq!(body, "");
        let fm = parse(front).unwrap();
        assert_eq!(fm.id, Some(n.id));
        assert_eq!(fm.title.as_deref(), Some(n.title.as_str()));
        assert_eq!(fm.path.as_deref(), Some("a/b"));
        assert_eq!(fm.tags.as_ref(), Some(&n.tags));
        assert_eq!((fm.created, fm.modified), (Some(-5), Some(i64::MAX)));
        assert!(fm.unknown_keys.is_empty());
    }

    #[test]
    fn hand_written_front_matter_styles_are_accepted() {
        let fm = parse(
            "# comment\ntitle: Plain title # trailing\ntags:\n  - one\n  - 'two'\ncreated: 2023-11-14\nowner: me\naliases:\n  - x\n",
        )
        .unwrap();
        assert_eq!(fm.title.as_deref(), Some("Plain title"));
        assert_eq!(fm.tags, Some(vec!["one".into(), "two".into()]));
        assert_eq!(fm.created, Some(1_699_920_000_000));
        assert_eq!(fm.unknown_keys, ["owner", "aliases"]);
        let fm = parse("tags: [a, 'b c', \"d\",]\n").unwrap();
        assert_eq!(fm.tags, Some(vec!["a".into(), "b c".into(), "d".into()]));
        assert_eq!(parse("tags: []\n").unwrap().tags, Some(vec![]));
    }

    #[test]
    fn malformed_front_matter_is_rejected() {
        for bad in [
            "just text\n",
            "title: \"open\n",
            "title: \"a\" junk\n",
            "id: nothex\n",
            "created: yesterday\n",
            "title: a\ntitle: b\n",
            "tags: [a, b\n",
            "tags: nope\n",
            "  indented first\n",
            "title:no-space\n",
            "bad key: 1\n",
            "title: \"\\q\"\n",
            "title: \"\\uD800\"\n",
            "title: [x]\n",
            "title:\n  more\n",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
