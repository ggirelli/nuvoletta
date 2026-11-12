//! Recovery key and Crockford base32 codec (CRYPTO_FORMAT.md section 3).

use crate::error::{Error, Result};
use crate::rng::{OsRandom, RandomSource};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CHECK_LEN: usize = 3;
const PAYLOAD_LEN: usize = 32 + CHECK_LEN;
/// Number of base32 characters in a recovery key.
const CHARS: usize = 56;

/// A 256-bit recovery key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey(pub(crate) [u8; 32]);

impl core::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RecoveryKey(<redacted>)")
    }
}

fn check(rk: &[u8; 32]) -> [u8; CHECK_LEN] {
    let mut h = Sha256::new();
    h.update(b"nuvoletta/v1/recovery-check");
    h.update(rk);
    let d = h.finalize();
    [d[0], d[1], d[2]]
}

impl RecoveryKey {
    /// Generate a fresh key from the OS CSPRNG.
    pub fn generate() -> Result<Self> {
        Self::generate_with(&mut OsRandom)
    }

    pub(crate) fn generate_with(rng: &mut impl RandomSource) -> Result<Self> {
        Ok(Self(rng.array()?))
    }

    /// Build from raw bytes (e.g. test vectors).
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Printable form: 14 groups of 4 Crockford base32 characters separated by `-`.
    #[must_use]
    pub fn to_display_string(&self) -> Zeroizing<String> {
        let mut payload = Zeroizing::new([0u8; PAYLOAD_LEN]);
        payload[..32].copy_from_slice(&self.0);
        payload[32..].copy_from_slice(&check(&self.0));
        let mut out = Zeroizing::new(String::with_capacity(CHARS + 13));
        let mut acc: u32 = 0;
        let mut bits = 0u32;
        let mut n = 0usize;
        for &b in payload.iter() {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                let idx = ((acc >> bits) & 31) as usize;
                if n > 0 && n.is_multiple_of(4) {
                    out.push('-');
                }
                out.push(char::from(ALPHABET[idx]));
                n += 1;
            }
            acc &= (1 << bits) - 1;
        }
        debug_assert_eq!(bits, 0);
        acc.zeroize();
        out
    }

    /// Parse the printable form. Case-insensitive; ignores `-` and whitespace;
    /// maps `O` to `0` and `I`, `L` to `1`. A wrong checksum is reported as
    /// [`Error::RecoveryKeyChecksum`] (typo).
    pub fn parse(text: &str) -> Result<Self> {
        let mut payload = Zeroizing::new([0u8; PAYLOAD_LEN]);
        let mut acc: u32 = 0;
        let mut bits = 0u32;
        let mut n = 0usize;
        let mut pos = 0usize;
        for c in text.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let c = c.to_ascii_uppercase();
            let c = match c {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            let v = ALPHABET
                .iter()
                .position(|&a| char::from(a) == c)
                .ok_or(Error::RecoveryKeyFormat)?;
            n += 1;
            if n > CHARS {
                return Err(Error::RecoveryKeyFormat);
            }
            acc = (acc << 5) | v as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                payload[pos] = ((acc >> bits) & 0xff) as u8;
                pos += 1;
            }
            acc &= (1 << bits) - 1;
        }
        acc.zeroize();
        if n != CHARS || pos != PAYLOAD_LEN {
            return Err(Error::RecoveryKeyFormat);
        }
        let mut rk = [0u8; 32];
        rk.copy_from_slice(&payload[..32]);
        let expected = check(&rk);
        if bool::from(expected.ct_eq(&payload[32..])) {
            Ok(Self(rk))
        } else {
            rk.zeroize();
            Err(Error::RecoveryKeyChecksum)
        }
    }
}
