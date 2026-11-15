//! Mapping of a server-supplied `href` to a path inside the sync root: the server is untrusted,
//! so no result may escape the root or contain an unsafe segment.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_webdav::{Options, WebDavConfig, WebDavProvider};
use std::sync::OnceLock;

static PROVIDER: OnceLock<WebDavProvider> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let Ok(href) = std::str::from_utf8(data) else {
        return;
    };
    let p = PROVIDER.get_or_init(|| {
        let cfg = WebDavConfig::new(
            "https://dav.example.org/remote.php/dav/Notes%20X/",
            "u",
            "p",
            None,
        )
        .expect("valid config");
        WebDavProvider::new(cfg, Options::default())
    });
    let Some(rel) = p.rel_of_href_for_fuzz(href) else {
        return;
    };
    if rel.is_empty() {
        return;
    }
    assert!(!rel.starts_with('/') && !rel.ends_with('/'));
    for seg in rel.split('/') {
        assert!(
            !seg.is_empty() && seg != "." && seg != "..",
            "segment in {rel:?}"
        );
        assert!(!seg.contains(['\\', '\0', ':']) && !seg.chars().any(char::is_control));
    }
});
