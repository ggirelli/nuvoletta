//! Remote name grammar and stray-file classifier.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_sync::names::{Class, TMP_PREFIX, classify};

fuzz_target!(|data: &[u8]| {
    let Ok(path) = std::str::from_utf8(data) else {
        return;
    };
    match classify(path) {
        Class::Protocol(n) => {
            // The strict grammar is canonical: the name is exactly the canonical path.
            assert_eq!(n.path(), path);
        }
        Class::Copy(n) => {
            // A copy is never itself canonical, and what it copies is a protocol name.
            assert_ne!(n.path(), path);
            assert_eq!(classify(&n.path()), Class::Protocol(n));
        }
        Class::Temp => {
            assert!(
                path.rsplit('/')
                    .next()
                    .is_some_and(|f| f.starts_with(TMP_PREFIX))
            );
        }
        Class::Stray => {}
    }
});
