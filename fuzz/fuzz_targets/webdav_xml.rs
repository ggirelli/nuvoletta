//! WebDAV PROPFIND (multistatus) XML parser and the HTTP date parser.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_webdav::xml::{parse_http_date, parse_multistatus};

fuzz_target!(|data: &[u8]| {
    if let Ok(props) = parse_multistatus(data) {
        for p in props {
            // Entries without an href are dropped by the parser.
            assert!(!p.href.is_empty());
            if let Some(m) = p.mtime_ms {
                let _ = m;
            }
        }
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_http_date(s);
    }
});
