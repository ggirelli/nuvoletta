//! Note revision payload, CBOR via minicbor (CRYPTO_FORMAT.md section 6.2).

use crate::error::{Error, Result};
use minicbor::{Decode, Encode};
use unicode_normalization::is_nfc;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Highest supported schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Maximum body size in bytes.
pub const MAX_BODY: usize = 16 * 1024 * 1024;
/// Maximum title size in bytes.
pub const MAX_TITLE: usize = 1024;
/// Maximum number of tags.
pub const MAX_TAGS: usize = 256;

/// Note revision plaintext structure (integer-keyed CBOR map, keys ascending).
#[derive(Clone, PartialEq, Eq, Encode, Decode, Zeroize, ZeroizeOnDrop)]
#[cbor(map)]
pub struct NotePayload {
    /// Schema version (key 0).
    #[n(0)]
    pub schema_version: u32,
    /// Note id, must equal the envelope `scope_id` (key 1).
    #[cbor(n(1), with = "minicbor::bytes")]
    pub note_id: [u8; 16],
    /// Title (key 2).
    #[n(2)]
    pub title: String,
    /// Virtual path (key 3).
    #[n(3)]
    pub path: String,
    /// Tags (key 4).
    #[n(4)]
    pub tags: Vec<String>,
    /// Creation time, ms since Unix epoch (key 5).
    #[n(5)]
    pub created_at: i64,
    /// Modification time, ms (key 6).
    #[n(6)]
    pub modified_at: i64,
    /// Markdown body (key 7).
    #[n(7)]
    pub body: String,
}

impl core::fmt::Debug for NotePayload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("NotePayload(<redacted>)")
    }
}

fn valid_path(p: &str) -> bool {
    !p.starts_with('/') && is_nfc(p) && p.split('/').all(|s| !s.is_empty() && s != "..")
}

impl NotePayload {
    fn validate(&self) -> Result<()> {
        if self.schema_version == 0 || self.schema_version > SCHEMA_VERSION {
            return Err(Error::Payload("unsupported schema version"));
        }
        if self.title.len() > MAX_TITLE {
            return Err(Error::Payload("title too long"));
        }
        if self.body.len() > MAX_BODY {
            return Err(Error::Payload("body too large"));
        }
        if self.tags.len() > MAX_TAGS {
            return Err(Error::Payload("too many tags"));
        }
        if !valid_path(&self.path) {
            return Err(Error::Payload("invalid path"));
        }
        Ok(())
    }

    /// Validate and encode to CBOR.
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        self.validate()?;
        let mut out = Zeroizing::new(Vec::new());
        minicbor::encode(self, &mut *out).map_err(|_| Error::Payload("encoding failed"))?;
        Ok(out)
    }

    /// Decode and validate. Unknown map keys are ignored.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let p: Self = minicbor::decode(bytes).map_err(|_| Error::Payload("malformed cbor"))?;
        p.validate()?;
        Ok(p)
    }

    /// Check `note_id` against the envelope `scope_id`.
    pub fn check_scope(&self, scope_id: &[u8; 16]) -> Result<()> {
        if &self.note_id == scope_id {
            Ok(())
        } else {
            Err(Error::ContextMismatch)
        }
    }
}
