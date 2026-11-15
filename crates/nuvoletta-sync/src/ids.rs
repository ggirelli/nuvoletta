//! Fixed-size identifiers and hashes used by the protocol, with hex and CBOR (byte string)
//! encodings.

use crate::error::{Error, Result};
use core::fmt;
use minicbor::decode::Error as DecodeError;
use minicbor::encode::{Error as EncodeError, Write};
use minicbor::{Decode, Decoder, Encode, Encoder};
use sha2::{Digest, Sha256};

/// A 128-bit random identifier: device id, note id, revision id (`object_id` or `tombstone_id`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id16(pub [u8; 16]);

/// A 256-bit hash (SHA-256): entry hashes and object hashes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash32(pub [u8; 32]);

/// Device identifier alias, for readability.
pub type DeviceId = Id16;
/// Revision identifier alias (an `object_id` for a PUT, a `tombstone_id` for a DELETE).
pub type RevId = Id16;

impl Id16 {
    /// The all-zero id (never generated randomly in practice).
    pub const ZERO: Self = Self([0; 16]);

    /// Generate a random id from the OS CSPRNG.
    ///
    /// # Errors
    /// Fails if the OS random number generator fails.
    pub fn random() -> Result<Self> {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|_| Error::Rng)?;
        Ok(Self(b))
    }

    /// Parse exactly 32 lower-case hexadecimal characters.
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        let mut out = [0u8; 16];
        hex_decode(s, &mut out)?;
        Some(Self(out))
    }

    /// The first byte as two hex characters (shard directory of `o/`).
    #[must_use]
    pub fn shard(&self) -> String {
        format!("{:02x}", self.0[0])
    }
}

impl Hash32 {
    /// The all-zero hash (`prev` of the first entry of a log).
    pub const ZERO: Self = Self([0; 32]);

    /// SHA-256 of `data`.
    #[must_use]
    pub fn of(data: &[u8]) -> Self {
        Self(Sha256::digest(data).into())
    }
}

/// Decode lower-case hex into `out`; `None` on any other character or wrong length.
pub(crate) fn hex_decode(s: &str, out: &mut [u8]) -> Option<()> {
    let b = s.as_bytes();
    if b.len() != out.len() * 2 {
        return None;
    }
    let val = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    for (i, o) in out.iter_mut().enumerate() {
        *o = (val(b[2 * i])? << 4) | val(b[2 * i + 1])?;
    }
    Some(())
}

fn write_hex(bytes: &[u8], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for b in bytes {
        write!(f, "{b:02x}")?;
    }
    Ok(())
}

impl fmt::Display for Id16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}
impl fmt::Debug for Id16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}
impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}
impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}

impl<C> Encode<C> for Id16 {
    fn encode<W: Write>(
        &self,
        e: &mut Encoder<W>,
        _: &mut C,
    ) -> core::result::Result<(), EncodeError<W::Error>> {
        e.bytes(&self.0)?.ok()
    }
}
impl<C> Encode<C> for Hash32 {
    fn encode<W: Write>(
        &self,
        e: &mut Encoder<W>,
        _: &mut C,
    ) -> core::result::Result<(), EncodeError<W::Error>> {
        e.bytes(&self.0)?.ok()
    }
}
impl<'b, C> Decode<'b, C> for Id16 {
    fn decode(d: &mut Decoder<'b>, _: &mut C) -> core::result::Result<Self, DecodeError> {
        let b = d.bytes()?;
        <[u8; 16]>::try_from(b)
            .map(Self)
            .map_err(|_| DecodeError::message("id must be 16 bytes"))
    }
}
impl<'b, C> Decode<'b, C> for Hash32 {
    fn decode(d: &mut Decoder<'b>, _: &mut C) -> core::result::Result<Self, DecodeError> {
        let b = d.bytes()?;
        <[u8; 32]>::try_from(b)
            .map(Self)
            .map_err(|_| DecodeError::message("hash must be 32 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_is_strict() {
        let id = Id16([0xab; 16]);
        assert_eq!(Id16::from_hex(&id.to_string()), Some(id));
        assert_eq!(Id16::from_hex(&"AB".repeat(16)), None);
        assert_eq!(Id16::from_hex("ab"), None);
        assert_eq!(Id16::from_hex(&"g".repeat(32)), None);
    }

    #[test]
    fn cbor_rejects_wrong_lengths() {
        let mut buf = Vec::new();
        minicbor::encode(Id16([1; 16]), &mut buf).unwrap();
        assert_eq!(minicbor::decode::<Id16>(&buf).unwrap(), Id16([1; 16]));
        assert!(minicbor::decode::<Hash32>(&buf).is_err());
    }
}
