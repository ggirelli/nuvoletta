//! Object envelope (CRYPTO_FORMAT.md section 4).

use crate::error::{Error, Result};
use crate::keys::{Subkeys, VAULT_ID_LEN};
use crate::padding::{pad, unpad};
use crate::rng::{OsRandom, RandomSource};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

/// Envelope header length (also the AAD length).
pub const ENVELOPE_HEADER_LEN: usize = 80;
/// Poly1305 tag length.
pub const TAG_LEN: usize = 16;
/// Minimum envelope length: header, floor-sized padded plaintext and tag.
pub const MIN_ENVELOPE_LEN: usize = ENVELOPE_HEADER_LEN + 256 + TAG_LEN;

const MAGIC: [u8; 4] = *b"NVOB";
const VERSION: u8 = 1;
const ALG_XCHACHA_PADDED: u8 = 1;

/// Object type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjType {
    /// Note revision (key `K_note`).
    NoteRevision = 1,
    /// Log segment (key `K_log`).
    LogSegment = 2,
    /// Snapshot, reserved (key `K_log`).
    Snapshot = 3,
    /// Local-only state (key `K_local`).
    LocalState = 4,
}

impl ObjType {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::NoteRevision,
            2 => Self::LogSegment,
            3 => Self::Snapshot,
            4 => Self::LocalState,
            _ => return None,
        })
    }

    fn key(self, k: &Subkeys) -> &[u8; 32] {
        match self {
            Self::NoteRevision => &k.k_note,
            Self::LogSegment | Self::Snapshot => &k.k_log,
            Self::LocalState => &k.k_local,
        }
    }
}

/// Identity of an object: bound into the AAD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectMeta {
    /// Object type.
    pub obj_type: ObjType,
    /// Random per object; remote file name stem; revision id for notes.
    pub object_id: [u8; 16],
    /// note_id (type 1), device_id (types 2/3), zero (type 4).
    pub scope_id: [u8; 16],
}

impl ObjectMeta {
    /// Generate a random `object_id`.
    pub fn new(obj_type: ObjType, scope_id: [u8; 16]) -> Result<Self> {
        Self::new_with(&mut OsRandom, obj_type, scope_id)
    }

    pub(crate) fn new_with(
        rng: &mut impl RandomSource,
        obj_type: ObjType,
        scope_id: [u8; 16],
    ) -> Result<Self> {
        Ok(Self {
            obj_type,
            object_id: rng.array()?,
            scope_id,
        })
    }
}

/// A successfully authenticated and unpadded object.
pub struct OpenedObject {
    /// Metadata read from the (authenticated) envelope header.
    pub meta: ObjectMeta,
    /// Plaintext payload.
    pub plaintext: Zeroizing<Vec<u8>>,
}

impl core::fmt::Debug for OpenedObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenedObject")
            .field("meta", &self.meta)
            .finish_non_exhaustive()
    }
}

impl OpenedObject {
    /// Check object/scope ids against the context (file name, log reference).
    /// Call this after `open`; mismatch means the object was moved or replayed.
    pub fn check_context(&self, object_id: &[u8; 16], scope_id: &[u8; 16]) -> Result<()> {
        if &self.meta.object_id == object_id && &self.meta.scope_id == scope_id {
            Ok(())
        } else {
            Err(Error::ContextMismatch)
        }
    }
}

/// Encrypt `payload` into an envelope with a fresh random nonce.
pub fn seal(keys: &Subkeys, meta: &ObjectMeta, payload: &[u8]) -> Result<Vec<u8>> {
    seal_with(&mut OsRandom, keys, meta, payload)
}

pub(crate) fn seal_with(
    rng: &mut impl RandomSource,
    keys: &Subkeys,
    meta: &ObjectMeta,
    payload: &[u8],
) -> Result<Vec<u8>> {
    if meta.obj_type == ObjType::LocalState && meta.scope_id != [0u8; 16] {
        return Err(Error::MalformedEnvelope(
            "local state scope_id must be zero",
        ));
    }
    let nonce: [u8; 24] = rng.array()?;
    let mut out = Vec::with_capacity(ENVELOPE_HEADER_LEN + payload.len() + 300);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(ALG_XCHACHA_PADDED);
    out.push(meta.obj_type as u8);
    out.push(0);
    out.extend_from_slice(&keys.vault_id);
    out.extend_from_slice(&meta.object_id);
    out.extend_from_slice(&meta.scope_id);
    out.extend_from_slice(&nonce);
    debug_assert_eq!(out.len(), ENVELOPE_HEADER_LEN);
    let padded = Zeroizing::new(pad(payload)?);
    let ct = XChaCha20Poly1305::new(meta.obj_type.key(keys).into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &padded,
                aad: &out,
            },
        )
        .map_err(|_| Error::Authentication)?;
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Authenticate and decrypt an envelope. Returns no plaintext on any failure.
pub fn open(keys: &Subkeys, bytes: &[u8]) -> Result<OpenedObject> {
    if bytes.len() < MIN_ENVELOPE_LEN {
        return Err(Error::MalformedEnvelope("too short"));
    }
    if bytes[..4] != MAGIC {
        return Err(Error::MalformedEnvelope("bad magic"));
    }
    if bytes[4] != VERSION {
        return Err(Error::UnsupportedVersion);
    }
    if bytes[5] != ALG_XCHACHA_PADDED {
        return Err(Error::UnsupportedAlgorithm);
    }
    let obj_type =
        ObjType::from_u8(bytes[6]).ok_or(Error::MalformedEnvelope("unknown object type"))?;
    if bytes[7] != 0 {
        return Err(Error::MalformedEnvelope("non-zero flags"));
    }
    let mut vault_id = [0u8; VAULT_ID_LEN];
    vault_id.copy_from_slice(&bytes[8..24]);
    if vault_id != keys.vault_id {
        return Err(Error::VaultMismatch);
    }
    let mut object_id = [0u8; 16];
    object_id.copy_from_slice(&bytes[24..40]);
    let mut scope_id = [0u8; 16];
    scope_id.copy_from_slice(&bytes[40..56]);
    let (aad, ct) = bytes.split_at(ENVELOPE_HEADER_LEN);
    let pt = XChaCha20Poly1305::new(obj_type.key(keys).into())
        .decrypt(XNonce::from_slice(&bytes[56..80]), Payload { msg: ct, aad })
        .map_err(|_| Error::Authentication)?;
    let pt = Zeroizing::new(pt);
    if obj_type == ObjType::LocalState && scope_id != [0u8; 16] {
        return Err(Error::MalformedEnvelope(
            "local state scope_id must be zero",
        ));
    }
    let payload = Zeroizing::new(unpad(&pt)?.to_vec());
    Ok(OpenedObject {
        meta: ObjectMeta {
            obj_type,
            object_id,
            scope_id,
        },
        plaintext: payload,
    })
}
