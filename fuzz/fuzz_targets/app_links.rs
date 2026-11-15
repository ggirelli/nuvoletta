//! Classification of links clicked in the preview: what the app opens or hands to the system.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_app_core::{LinkAction, classify_link};

fuzz_target!(|data: &[u8]| {
    let Ok(href) = std::str::from_utf8(data) else {
        return;
    };
    if let LinkAction::External(url) = classify_link(href) {
        let l = url.to_ascii_lowercase();
        assert!(l.starts_with("https://") || l.starts_with("http://") || l.starts_with("mailto:"));
        assert!(!url.chars().any(char::is_control));
    }
});
