//! Validated virtual note paths (`docs/CRYPTO_FORMAT.md` section 6.2).

use crate::error::{Error, Result};
use core::fmt;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroize;

/// A virtual, `/`-separated note path such as `projects/2026/ideas`.
///
/// Paths are labels inside the encrypted note, never file names. A valid path is NFC normalised,
/// has no leading or trailing `/`, no empty, `.` or `..` segment and no control characters.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Zeroize)]
pub struct NotePath(String);

impl NotePath {
    /// Normalise `input` to NFC and validate it.
    ///
    /// # Errors
    /// Returns [`Error::InvalidPath`] if the normalised path breaks a rule.
    pub fn parse(input: &str) -> Result<Self> {
        let s: String = input.nfc().collect();
        if s.starts_with('/') {
            return Err(Error::InvalidPath("leading separator"));
        }
        if s.chars().any(char::is_control) {
            return Err(Error::InvalidPath("control character"));
        }
        for seg in s.split('/') {
            match seg {
                "" => return Err(Error::InvalidPath("empty segment")),
                "." | ".." => return Err(Error::InvalidPath("dot segment")),
                _ => {}
            }
        }
        Ok(Self(s))
    }

    /// The path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `/`-separated segments.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for NotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for NotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NotePath(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn plain_nested_path_is_accepted() {
        let p = NotePath::parse("a/b/c").unwrap();
        assert_eq!(p.segments().collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn invalid_paths_are_rejected() {
        for bad in [
            "", "/a", "a/", "a//b", "..", "a/../b", "a/..", ".", "a/./b", "a\0b", "a\nb",
            "a\u{7f}", "a\u{85}b",
        ] {
            assert!(NotePath::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn input_is_normalised_to_nfc() {
        let p = NotePath::parse("cafe\u{301}").unwrap();
        assert_eq!(p.as_str(), "caf\u{e9}");
    }

    #[test]
    fn dots_inside_a_segment_are_allowed() {
        assert!(NotePath::parse("a/..b/c..").is_ok());
    }

    proptest! {
        #[test]
        fn parsing_is_idempotent_and_result_is_valid(s in "\\PC{0,40}(/\\PC{0,10}){0,3}") {
            if let Ok(p) = NotePath::parse(&s) {
                prop_assert_eq!(NotePath::parse(p.as_str()).unwrap(), p.clone());
                prop_assert!(!p.as_str().starts_with('/'));
                prop_assert!(p.segments().all(|x| !x.is_empty() && x != ".." && x != "."));
                prop_assert!(!p.as_str().chars().any(char::is_control));
            }
        }

        #[test]
        fn parse_never_panics(s in ".*") {
            let _ = NotePath::parse(&s);
        }
    }
}
