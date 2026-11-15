//! Embedded assets. Everything the page needs is compiled into the binary and inlined into the
//! document head, so the app does not depend on `dx`'s asset pipeline or on files next to the
//! executable, and the webview never loads a file or a URL for its own UI.

use nuvoletta_app_core::CONTENT_SECURITY_POLICY;
use std::sync::LazyLock;

const CSS: &str = include_str!("../assets/main.css");
/// Vendored CodeMirror 6 bundle (see `js/README.md`).
const EDITOR_JS: &str = include_str!("../assets/editor.js");
/// Dialog focus trap and focus restore, and the document language (`assets/a11y.js`).
const A11Y_JS: &str = include_str!("../assets/a11y.js");
const MARK_SVG: &str = include_str!("../assets/01-logo-mark.svg");
const WORDMARK_LIGHT_SVG: &str = include_str!("../assets/07a-wordmark-light.svg");
const WORDMARK_DARK_SVG: &str = include_str!("../assets/07b-wordmark-dark.svg");

/// The content of `<head>`: CSP first, then styles and the editor bundle.
pub fn head() -> String {
    format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{CONTENT_SECURITY_POLICY}\">\
         <style>{CSS}</style><script>{EDITOR_JS}</script><script>{A11Y_JS}</script>"
    )
}

/// Percent-encode an SVG into a `data:` URI (allowed by the CSP's `img-src`).
fn svg_uri(svg: &str) -> String {
    let mut out = String::from("data:image/svg+xml,");
    for b in svg.trim().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Brand mark as a data URI.
pub static MARK: LazyLock<String> = LazyLock::new(|| svg_uri(MARK_SVG));
/// Wordmark for light themes.
pub static WORDMARK_LIGHT: LazyLock<String> = LazyLock::new(|| svg_uri(WORDMARK_LIGHT_SVG));
/// Wordmark for dark themes.
pub static WORDMARK_DARK: LazyLock<String> = LazyLock::new(|| svg_uri(WORDMARK_DARK_SVG));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_script_and_style_cannot_close_their_element() {
        for (name, text) in [
            ("editor.js", EDITOR_JS),
            ("a11y.js", A11Y_JS),
            ("main.css", CSS),
        ] {
            let lower = text.to_ascii_lowercase();
            assert!(!lower.contains("</script"), "{name}");
            assert!(!lower.contains("</style"), "{name}");
            assert!(!lower.contains("<script"), "{name}");
        }
    }

    #[test]
    fn svg_uri_encodes_markup_and_quotes() {
        let u = svg_uri("<svg a=\"#4C7898\"/>");
        assert!(u.starts_with("data:image/svg+xml,%3Csvg"));
        assert!(!u.contains(['<', '>', '"', '#', ' ']));
    }

    #[test]
    fn head_starts_with_the_policy() {
        assert!(head().starts_with("<meta http-equiv=\"Content-Security-Policy\""));
    }
}
