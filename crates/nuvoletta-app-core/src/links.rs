//! Classification of links clicked in the preview.

use nuvoletta_core::NoteId;

/// What the application does with a clicked link. The webview itself never navigates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    /// Open this note in the app (`nuvoletta://note/<id>[#fragment]`).
    OpenNote(NoteId),
    /// Open this `http`, `https` or `mailto` URL in the system browser/mail client.
    External(String),
    /// Do nothing.
    Ignore,
}

/// Decide what to do with `href`.
#[must_use]
pub fn classify_link(href: &str) -> LinkAction {
    let href = href.trim();
    if href.chars().any(char::is_control) {
        return LinkAction::Ignore;
    }
    let Some((scheme, rest)) = href.split_once(':') else {
        return LinkAction::Ignore;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "nuvoletta" => {
            let Some(path) = rest.strip_prefix("//note/") else {
                return LinkAction::Ignore;
            };
            let id = path.split('#').next().unwrap_or("");
            id.parse().map_or(LinkAction::Ignore, LinkAction::OpenNote)
        }
        "http" | "https" if rest.starts_with("//") && rest.len() > 2 => {
            LinkAction::External(href.to_owned())
        }
        "mailto" if !rest.is_empty() => LinkAction::External(href.to_owned()),
        _ => LinkAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn note_links_open_in_app_with_or_without_fragment() {
        let want = LinkAction::OpenNote(ID.parse().unwrap());
        assert_eq!(classify_link(&format!("nuvoletta://note/{ID}")), want);
        assert_eq!(classify_link(&format!("nuvoletta://note/{ID}#Intro")), want);
    }

    #[test]
    fn external_links_are_limited_to_web_and_mail() {
        assert!(matches!(
            classify_link("https://example.org/x"),
            LinkAction::External(_)
        ));
        assert!(matches!(
            classify_link("HTTP://example.org"),
            LinkAction::External(_)
        ));
        assert!(matches!(
            classify_link("mailto:a@example.org"),
            LinkAction::External(_)
        ));
    }

    #[test]
    fn everything_else_is_ignored() {
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "nuvoletta://note/zz",
            "nuvoletta://other/x",
            "https:",
            "https://",
            "relative/path",
            "#frag",
            "ht\ttps://example.org",
            "",
        ] {
            assert_eq!(classify_link(bad), LinkAction::Ignore, "{bad:?}");
        }
    }
}
