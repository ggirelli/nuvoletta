//! The Content-Security-Policy of the webview (`PLAN.md` Phase 3, ADR 0003).
//!
//! The policy is defence in depth behind the safe renderer: it blocks every network load (so a
//! hostile note cannot make the webview fetch or leak anything even if the renderer had a bug),
//! inline event handlers, `javascript:` URLs, plugins, frames, forms and `<base>` tampering.
//!
//! What it deliberately allows, and why (Dioxus 0.7 desktop constraints):
//! - `script-src-elem 'unsafe-inline'`: Dioxus injects an inline bootstrap `<script>` whose text
//!   contains a per-run key, so it cannot be pinned by hash or nonce.
//! - `script-src 'unsafe-eval'`: `document::eval`, which carries editor text and shortcuts
//!   between the webview and Rust, runs its snippets through the `AsyncFunction` constructor.
//! - `connect-src ws://127.0.0.1:*`: Dioxus streams UI edits from Rust to the webview over a
//!   WebSocket on the loopback interface (random port, random per-run key). No other host.
//! - `style-src 'unsafe-inline'`: CodeMirror and Dioxus insert `<style>` elements.
//!
//! `script-src-attr 'none'` blocks inline event-handler attributes (verified in WebKitGTK).
//! `javascript:` URLs are **not** stopped by this policy there (WebKit checks them against
//! `script-src-elem`, which has to allow inline scripts for Dioxus). They are stopped one layer
//! earlier: the renderer never emits them (link scheme allow-list, ADR 0003) and the app cancels
//! every click on a link in the preview before the webview can act on it.

/// The policy, as the value of a `<meta http-equiv="Content-Security-Policy">` element.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
script-src 'self' 'unsafe-eval'; \
script-src-elem 'self' 'unsafe-inline'; \
script-src-attr 'none'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
font-src 'self'; \
connect-src 'self' ws://127.0.0.1:*; \
object-src 'none'; \
frame-src 'none'; \
worker-src 'none'; \
media-src 'none'; \
manifest-src 'none'; \
base-uri 'none'; \
form-action 'none'";

#[cfg(test)]
mod tests {
    use super::*;

    fn directive(name: &str) -> &'static str {
        CONTENT_SECURITY_POLICY
            .split(';')
            .map(str::trim)
            .find_map(|d| d.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
            .unwrap_or_else(|| panic!("directive {name} missing"))
    }

    #[test]
    fn nothing_may_be_loaded_from_a_remote_host() {
        assert!(!CONTENT_SECURITY_POLICY.contains("http:"));
        assert!(!CONTENT_SECURITY_POLICY.contains("https:"));
        // The only wildcard is the loopback WebSocket port.
        assert_eq!(CONTENT_SECURITY_POLICY.matches('*').count(), 1);
        assert_eq!(directive("default-src"), "'none'");
        assert_eq!(directive("img-src"), "'self' data:");
        assert_eq!(directive("connect-src"), "'self' ws://127.0.0.1:*");
    }

    #[test]
    fn inline_event_handlers_stay_blocked_and_script_src_has_no_unsafe_inline() {
        assert!(!directive("script-src").contains("'unsafe-inline'"));
        assert_eq!(directive("script-src-attr"), "'none'");
    }

    #[test]
    fn frames_plugins_forms_and_base_are_closed() {
        for d in ["object-src", "frame-src", "worker-src", "base-uri"] {
            assert_eq!(directive(d), "'none'", "{d}");
        }
        assert_eq!(directive("form-action"), "'none'");
    }

    #[test]
    fn policy_cannot_break_out_of_its_html_attribute() {
        assert!(!CONTENT_SECURITY_POLICY.contains('"'));
    }
}
