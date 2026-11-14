//! Deterministic escaping of virtual path segments into names valid on Linux, macOS and Windows.

use unicode_normalization::UnicodeNormalization;

/// Longest escaped segment in bytes; leaves room for `~<id>` and `.md` under the 255-byte limit.
const MAX_SEGMENT: usize = 200;

/// Characters invalid in Windows file names (and `/`, `\` which are separators somewhere).
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

fn push_escaped(out: &mut String, c: char) {
    let mut buf = [0u8; 4];
    for b in c.encode_utf8(&mut buf).bytes() {
        out.push_str(&format!("%{b:02X}"));
    }
}

fn is_reserved(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or("").trim_end();
    let up = stem.to_uppercase();
    let numbered = |prefix: &str| {
        up.strip_prefix(prefix).is_some_and(|rest| {
            let mut it = rest.chars();
            matches!(it.next(), Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'))
                && it.next().is_none()
        })
    };
    matches!(
        up.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT")
}

/// Escape one path segment.
///
/// Rules, applied in this order:
/// 1. `< > : " / \ | ? *`, control characters and U+2028/U+2029 become `%XX` per UTF-8 byte.
/// 2. The result is cut to 200 bytes at a character boundary.
/// 3. A leading `.` and a trailing `.` or space become `%2E` / `%20`.
/// 4. A Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9`,
///    also with an extension) has its first character escaped.
///
/// Everything else, including non-ASCII text, is kept. The mapping is not injective (two
/// different segments can map to the same name); the exporter resolves that.
pub(crate) fn escape_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for c in seg.chars() {
        if FORBIDDEN.contains(&c) || c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
            push_escaped(&mut out, c);
        } else {
            out.push(c);
        }
    }
    while out.len() > MAX_SEGMENT {
        out.pop();
    }
    if out.starts_with('.') {
        out.replace_range(..1, "%2E");
    }
    if out.ends_with('.') {
        out.replace_range(out.len() - 1.., "%2E");
    } else if out.ends_with(' ') {
        out.replace_range(out.len() - 1.., "%20");
    }
    if out.is_empty() {
        out.push_str("%5F");
    }
    if is_reserved(&out) {
        let first = out.chars().next().unwrap_or('_');
        let mut esc = String::new();
        push_escaped(&mut esc, first);
        out.replace_range(..first.len_utf8(), &esc);
    }
    out
}

/// Key under which two names are considered the same file: lower-cased and NFD-normalised, to be
/// safe on case-insensitive and normalisation-insensitive file systems.
pub(crate) fn fold(name: &str) -> String {
    name.to_lowercase().nfd().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_are_untouched() {
        for s in [
            "note",
            "caf\u{e9} \u{1f600}",
            "a.b",
            "my-note_1",
            "\u{65e5}\u{672c}",
        ] {
            assert_eq!(escape_segment(s), s);
        }
    }

    #[test]
    fn forbidden_characters_are_percent_encoded() {
        assert_eq!(escape_segment("a:b*c?"), "a%3Ab%2Ac%3F");
        assert_eq!(escape_segment("x\\y"), "x%5Cy");
        assert_eq!(escape_segment("\u{2028}"), "%E2%80%A8");
    }

    #[test]
    fn dots_and_spaces_at_the_edges_are_escaped() {
        assert_eq!(escape_segment(".hidden"), "%2Ehidden");
        assert_eq!(escape_segment("end."), "end%2E");
        assert_eq!(escape_segment("end "), "end%20");
        assert_eq!(escape_segment("a..b"), "a..b");
    }

    #[test]
    fn reserved_windows_names_are_escaped() {
        for (s, e) in [
            ("CON", "%43ON"),
            ("nul", "%6Eul"),
            ("Com1", "%43om1"),
            ("lpt9.txt", "%6Cpt9.txt"),
            ("aux.", "aux%2E"),
        ] {
            assert_eq!(escape_segment(s), e);
        }
        assert_eq!(escape_segment("COM10"), "COM10");
        assert_eq!(escape_segment("console"), "console");
    }

    #[test]
    fn long_names_are_truncated_on_a_character_boundary() {
        let e = escape_segment(&"\u{e9}".repeat(300));
        assert!(e.len() <= MAX_SEGMENT);
        assert!(e.chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn fold_ignores_case_and_normalisation() {
        assert_eq!(fold("Caf\u{e9}"), fold("cafe\u{301}"));
    }
}
