//! Padme padding (CRYPTO_FORMAT.md section 5).

use crate::error::{Error, Result};

/// Minimum padded length.
pub const PADDING_FLOOR: u64 = 256;

/// Padme: round `l` up so that only the top bits of the length remain significant.
#[must_use]
pub fn padme(l: u64) -> u64 {
    if l < 2 {
        return l;
    }
    let e = u64::from(l.ilog2());
    let s = u64::from(e.ilog2()) + 1;
    let z = e - s;
    let mask = (1u64 << z) - 1;
    (l + mask) & !mask
}

/// Padded plaintext length for a payload of `len` bytes: `padme(max(len + 4, 256))`.
pub fn padded_len(len: usize) -> Result<u64> {
    let l = u64::try_from(len).map_err(|_| Error::TooLarge)?;
    if l > u64::from(u32::MAX) - 4 {
        return Err(Error::TooLarge);
    }
    Ok(padme((l + 4).max(PADDING_FLOOR)))
}

/// Build `u32_le(len) || payload || zeros`.
pub(crate) fn pad(payload: &[u8]) -> Result<Vec<u8>> {
    let total = usize::try_from(padded_len(payload.len())?).map_err(|_| Error::TooLarge)?;
    let len = u32::try_from(payload.len()).map_err(|_| Error::TooLarge)?;
    let mut v = Vec::with_capacity(total);
    v.extend_from_slice(&len.to_le_bytes());
    v.extend_from_slice(payload);
    v.resize(total, 0);
    Ok(v)
}

/// Verify and strip padding. Strict: the padded length must be exactly the one
/// the writer rule produces, and the padding must be all zero.
pub(crate) fn unpad(padded: &[u8]) -> Result<&[u8]> {
    let head: [u8; 4] = padded
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or(Error::InvalidPadding)?;
    let len = u32::from_le_bytes(head) as usize;
    let end = len.checked_add(4).ok_or(Error::InvalidPadding)?;
    if end > padded.len() || padded_len(len)? != padded.len() as u64 {
        return Err(Error::InvalidPadding);
    }
    if padded[end..].iter().any(|&b| b != 0) {
        return Err(Error::InvalidPadding);
    }
    Ok(&padded[4..end])
}
