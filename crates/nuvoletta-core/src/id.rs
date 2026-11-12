//! Stable note identifiers.

use crate::error::{Error, Result};
use core::fmt;
use core::str::FromStr;
use zeroize::Zeroize;

/// A note id: 16 random bytes, stable for the life of the note and independent of its title,
/// path or file name. Displayed as 32 lower-case hexadecimal characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Zeroize)]
pub struct NoteId([u8; 16]);

impl NoteId {
    /// Generate a fresh id from the OS CSPRNG.
    ///
    /// # Errors
    /// Fails if the OS random number generator fails.
    pub fn generate() -> Result<Self> {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|_| Error::Rng)?;
        Ok(Self(b))
    }

    /// Build an id from raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoteId({self})")
    }
}

impl FromStr for NoteId {
    type Err = Error;

    /// Parse exactly 32 hexadecimal characters (lower or upper case).
    fn from_str(s: &str) -> Result<Self> {
        let s = s.as_bytes();
        if s.len() != 32 {
            return Err(Error::InvalidId);
        }
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            *o = (hex_val(s[2 * i])? << 4) | hex_val(s[2 * i + 1])?;
        }
        Ok(Self(out))
    }
}

fn hex_val(c: u8) -> Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(Error::InvalidId),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn display_is_32_lowercase_hex_characters() {
        let id = NoteId::from_bytes([0xab; 16]);
        assert_eq!(id.to_string(), "ab".repeat(16));
    }

    #[test]
    fn generated_ids_differ() {
        assert_ne!(NoteId::generate().unwrap(), NoteId::generate().unwrap());
    }

    #[test]
    fn parse_rejects_wrong_length_and_characters() {
        for bad in [
            "",
            "ab",
            &"g".repeat(32),
            &"a".repeat(31),
            &"a".repeat(33),
            "é",
        ] {
            assert_eq!(bad.parse::<NoteId>(), Err(Error::InvalidId), "{bad}");
        }
    }

    #[test]
    fn parse_accepts_upper_case() {
        let id: NoteId = "AB".repeat(16).parse().unwrap();
        assert_eq!(id, NoteId::from_bytes([0xab; 16]));
    }

    proptest! {
        #[test]
        fn display_parse_round_trips(bytes in any::<[u8; 16]>()) {
            let id = NoteId::from_bytes(bytes);
            prop_assert_eq!(id.to_string().parse::<NoteId>().unwrap(), id);
        }

        #[test]
        fn parse_never_panics(s in ".*") {
            let _ = s.parse::<NoteId>();
        }
    }
}
