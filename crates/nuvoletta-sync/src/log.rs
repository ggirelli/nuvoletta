//! Per-device log entries and segments (`SYNC_PROTOCOL.md` section 5).
//!
//! A segment is one envelope (type 2, key `K_log`, `scope_id = device_id`) whose payload is a CBOR
//! array of entries. Entry hashes are computed over the bytes as stored, never over a re-encoding.

use crate::ids::{DeviceId, Hash32, Id16, RevId};
use minicbor::{Decode, Decoder, Encode};
use nuvoletta_crypto::{ObjType, ObjectMeta, Subkeys, open, seal};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Entry kind: a note revision was introduced.
pub const KIND_PUT: u8 = 1;
/// Entry kind: a note was deleted (introduces a tombstone revision).
pub const KIND_DELETE: u8 = 2;
/// Entry kind: cross-attestation only.
pub const KIND_CHECKPOINT: u8 = 3;

/// Domain separation of entry hashes.
const HASH_DOMAIN: &[u8] = b"nuvoletta/v1/log-entry";
/// Sanity bounds for hostile input.
const MAX_ENTRIES_PER_SEGMENT: u64 = 100_000;
const MAX_PARENTS: usize = 256;
const MAX_SEEN: usize = 4096;

/// A device position recorded in a checkpoint: the highest entry this device verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(array)]
pub struct SeenPos {
    /// Sequence number.
    #[n(0)]
    pub seq: u64,
    /// `entry_hash` at that sequence number.
    #[n(1)]
    pub hash: Hash32,
}

/// A log entry (CBOR map with integer keys, in ascending order).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Entry {
    /// Sequence number, starts at 1, contiguous across segments (key 0).
    #[n(0)]
    pub seq: u64,
    /// Hash of the previous entry; zero for sequence 1 (key 1).
    #[n(1)]
    pub prev: Hash32,
    /// [`KIND_PUT`], [`KIND_DELETE`] or [`KIND_CHECKPOINT`] (key 2).
    #[n(2)]
    pub kind: u8,
    /// Informational time, ms; never used for ordering or resolution (key 3).
    #[n(3)]
    pub time: i64,
    /// Note id (PUT, DELETE) (key 4).
    #[n(4)]
    #[cbor(skip_if = "Option::is_none")]
    pub note_id: Option<Id16>,
    /// `object_id` (PUT) or `tombstone_id` (DELETE) (key 5).
    #[n(5)]
    #[cbor(skip_if = "Option::is_none")]
    pub rev_id: Option<RevId>,
    /// SHA-256 of the object file (PUT) (key 6).
    #[n(6)]
    #[cbor(skip_if = "Option::is_none")]
    pub ct_hash: Option<Hash32>,
    /// Revisions this change is based on (PUT, DELETE) (key 7).
    #[n(7)]
    #[cbor(skip_if = "Option::is_none")]
    pub parents: Option<Vec<RevId>>,
    /// Highest verified `(seq, hash)` per device (CHECKPOINT; may ride on any entry) (key 8).
    #[n(8)]
    #[cbor(skip_if = "Option::is_none")]
    pub seen: Option<BTreeMap<DeviceId, SeenPos>>,
}

impl Entry {
    /// A body to be placed in a log: `seq` and `prev` are filled in when the segment is built.
    #[must_use]
    pub fn body(kind: u8, time: i64) -> Self {
        Self {
            seq: 0,
            prev: Hash32::ZERO,
            kind,
            time,
            note_id: None,
            rev_id: None,
            ct_hash: None,
            parents: None,
            seen: None,
        }
    }

    /// The parents, sorted and deduplicated (the order in the file carries no meaning).
    #[must_use]
    pub fn parents_sorted(&self) -> Vec<RevId> {
        let mut p = self.parents.clone().unwrap_or_default();
        p.sort();
        p.dedup();
        p
    }

    /// Semantic validity of a chain-valid entry (section 5.2). An invalid entry still occupies
    /// its sequence number; it is reported and not applied.
    ///
    /// # Errors
    /// A static description of what is wrong.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.parents.as_ref().is_some_and(|p| p.len() > MAX_PARENTS) {
            return Err("too many parents");
        }
        if self.seen.as_ref().is_some_and(|s| s.len() > MAX_SEEN) {
            return Err("too many seen positions");
        }
        if self
            .seen
            .as_ref()
            .is_some_and(|s| s.values().any(|p| p.seq == 0))
        {
            return Err("seen position with sequence number zero");
        }
        match self.kind {
            KIND_PUT => {
                if self.note_id.is_none() || self.rev_id.is_none() || self.ct_hash.is_none() {
                    return Err("PUT needs note_id, rev_id and ct_hash");
                }
            }
            KIND_DELETE => {
                if self.note_id.is_none() || self.rev_id.is_none() {
                    return Err("DELETE needs note_id and rev_id");
                }
                if self.ct_hash.is_some() {
                    return Err("DELETE must not carry ct_hash");
                }
                if self.parents.as_ref().is_none_or(Vec::is_empty) {
                    return Err("DELETE needs parents");
                }
            }
            KIND_CHECKPOINT => {
                if self.note_id.is_some() || self.rev_id.is_some() || self.ct_hash.is_some() {
                    return Err("CHECKPOINT carries no revision");
                }
            }
            _ => return Err("unknown entry kind"),
        }
        Ok(())
    }
}

/// `entry_hash = SHA-256("nuvoletta/v1/log-entry" ‖ prev ‖ stored entry bytes)`.
#[must_use]
pub fn entry_hash(prev: &Hash32, stored: &[u8]) -> Hash32 {
    let mut h = Sha256::new();
    h.update(HASH_DOMAIN);
    h.update(prev.0);
    h.update(stored);
    Hash32(h.finalize().into())
}

/// An entry together with its computed hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEntry {
    /// The decoded entry.
    pub entry: Entry,
    /// Its `entry_hash`.
    pub hash: Hash32,
}

/// A sealed segment ready to upload.
#[derive(Clone, Debug)]
pub struct SealedSegment {
    /// Envelope bytes (uploaded verbatim, byte-identical on every retry).
    pub bytes: Vec<u8>,
    /// Sequence number of the first entry; the file name stem.
    pub first_seq: u64,
    /// The entries with their hashes.
    pub entries: Vec<ParsedEntry>,
}

/// Build and seal a segment from entry bodies, assigning `seq` and `prev` (the chain) starting at
/// `next_seq` / `last_hash`.
///
/// # Errors
/// Crypto failures (RNG) or an empty entry list.
pub fn seal_segment(
    keys: &Subkeys,
    device: DeviceId,
    next_seq: u64,
    last_hash: Hash32,
    bodies: Vec<Entry>,
) -> crate::Result<SealedSegment> {
    if bodies.is_empty() {
        return Err(crate::Error::State("empty segment"));
    }
    let mut prev = last_hash;
    let mut payload = Vec::new();
    let mut e = minicbor::Encoder::new(&mut payload);
    e.array(bodies.len() as u64)
        .map_err(|_| crate::Error::State("cbor encode"))?;
    let mut entries = Vec::with_capacity(bodies.len());
    for (seq, mut body) in (next_seq..).zip(bodies) {
        body.seq = seq;
        body.prev = prev;
        let mut stored = Vec::new();
        minicbor::encode(&body, &mut stored).map_err(|_| crate::Error::State("cbor encode"))?;
        let hash = entry_hash(&prev, &stored);
        e.writer_mut().extend_from_slice(&stored);
        entries.push(ParsedEntry { entry: body, hash });
        prev = hash;
    }
    let meta = ObjectMeta::new(ObjType::LogSegment, device.0)?;
    let bytes = seal(keys, &meta, &payload)?;
    Ok(SealedSegment {
        bytes,
        first_seq: next_seq,
        entries,
    })
}

/// A verified segment.
#[derive(Clone, Debug)]
pub struct ParsedSegment {
    /// The writer, from the authenticated `scope_id` (never from the file name).
    pub device: DeviceId,
    /// Envelope `object_id`.
    pub object_id: Id16,
    /// Entries with hashes, contiguous and internally chained.
    pub entries: Vec<ParsedEntry>,
}

impl ParsedSegment {
    /// Sequence number of the first entry.
    #[must_use]
    pub fn first_seq(&self) -> u64 {
        self.entries[0].entry.seq
    }

    /// Sequence number of the last entry.
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.entries[self.entries.len() - 1].entry.seq
    }
}

/// Why a segment file could not be used.
#[derive(Debug, PartialEq, Eq)]
pub enum SegmentError {
    /// Did not authenticate or parse as an envelope: truncated, being written, foreign or
    /// tampered. Treated as "not yet arrived".
    Unreadable,
    /// Authenticated but structurally wrong (not a segment, bad CBOR, non-contiguous sequence
    /// numbers, broken chain inside the segment). Reported as invalid.
    Malformed(&'static str),
}

/// Authenticate and parse a segment (`SYNC_PROTOCOL.md` section 5.2, reader checks). Does not
/// check the link to earlier entries or the file name: the caller does.
///
/// # Errors
/// [`SegmentError`].
pub fn open_segment(keys: &Subkeys, bytes: &[u8]) -> Result<ParsedSegment, SegmentError> {
    let opened = open(keys, bytes).map_err(|_| SegmentError::Unreadable)?;
    if opened.meta.obj_type != ObjType::LogSegment {
        return Err(SegmentError::Malformed("not a log segment"));
    }
    let payload: &[u8] = &opened.plaintext;
    let mut d = Decoder::new(payload);
    let n = d
        .array()
        .map_err(|_| SegmentError::Malformed("payload is not an array"))?
        .ok_or(SegmentError::Malformed("indefinite-length array"))?;
    if n == 0 || n > MAX_ENTRIES_PER_SEGMENT {
        return Err(SegmentError::Malformed("bad entry count"));
    }
    let mut entries: Vec<ParsedEntry> = Vec::new();
    for _ in 0..n {
        let start = d.position();
        let entry: Entry = d
            .decode()
            .map_err(|_| SegmentError::Malformed("malformed entry"))?;
        let stored = &payload[start..d.position()];
        if entry.seq == 0 {
            return Err(SegmentError::Malformed("sequence number zero"));
        }
        if let Some(last) = entries.last() {
            if last.entry.seq.checked_add(1) != Some(entry.seq) {
                return Err(SegmentError::Malformed("non-contiguous sequence numbers"));
            }
            if entry.prev != last.hash {
                return Err(SegmentError::Malformed("broken hash chain"));
            }
        }
        let hash = entry_hash(&entry.prev, stored);
        entries.push(ParsedEntry { entry, hash });
    }
    if d.position() != payload.len() {
        return Err(SegmentError::Malformed("trailing bytes"));
    }
    Ok(ParsedSegment {
        device: Id16(opened.meta.scope_id),
        object_id: Id16(opened.meta.object_id),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuvoletta_crypto::{KdfParams, Vault};

    fn keys() -> Vault {
        Vault::create("correct horse battery staple", KdfParams::MIN_ACCEPTED)
            .unwrap()
            .vault
    }

    fn put(n: u8) -> Entry {
        let mut e = Entry::body(KIND_PUT, 1);
        e.note_id = Some(Id16([n; 16]));
        e.rev_id = Some(Id16([n + 1; 16]));
        e.ct_hash = Some(Hash32([n; 32]));
        e.parents = Some(vec![]);
        e
    }

    #[test]
    fn segment_round_trips_and_chains() {
        let v = keys();
        let dev = Id16([9; 16]);
        let s = seal_segment(v.keys(), dev, 5, Hash32([7; 32]), vec![put(1), put(2)]).unwrap();
        let p = open_segment(v.keys(), &s.bytes).unwrap();
        assert_eq!(p.device, dev);
        assert_eq!(p.first_seq(), 5);
        assert_eq!(p.last_seq(), 6);
        assert_eq!(p.entries, s.entries);
        assert_eq!(p.entries[0].entry.prev, Hash32([7; 32]));
        assert_eq!(p.entries[1].entry.prev, p.entries[0].hash);
    }

    #[test]
    fn tampered_or_truncated_segments_are_unreadable() {
        let v = keys();
        let s = seal_segment(v.keys(), Id16([9; 16]), 1, Hash32::ZERO, vec![put(1)]).unwrap();
        assert_eq!(
            open_segment(v.keys(), &s.bytes[..s.bytes.len() - 1]).unwrap_err(),
            SegmentError::Unreadable
        );
        let mut b = s.bytes.clone();
        b[100] ^= 1;
        assert_eq!(
            open_segment(v.keys(), &b).unwrap_err(),
            SegmentError::Unreadable
        );
        assert_eq!(
            open_segment(v.keys(), b"").unwrap_err(),
            SegmentError::Unreadable
        );
    }

    #[test]
    fn a_note_object_is_not_a_segment() {
        let v = keys();
        let meta = ObjectMeta::new(ObjType::NoteRevision, [1; 16]).unwrap();
        // Type 1 uses another key, so it does not even authenticate as a segment key use;
        // a type 2 envelope with a non-array payload is malformed.
        let bad = ObjectMeta::new(ObjType::LogSegment, [1; 16]).unwrap();
        let bytes = seal(v.keys(), &bad, b"\x01").unwrap();
        assert!(matches!(
            open_segment(v.keys(), &bytes),
            Err(SegmentError::Malformed(_))
        ));
        let bytes = seal(v.keys(), &meta, b"\x80").unwrap();
        assert!(matches!(
            open_segment(v.keys(), &bytes),
            Err(SegmentError::Malformed("not a log segment"))
        ));
    }

    #[test]
    fn entry_validity_rules() {
        assert!(put(1).check().is_ok());
        let mut e = put(1);
        e.ct_hash = None;
        assert!(e.check().is_err());
        let mut d = Entry::body(KIND_DELETE, 0);
        d.note_id = Some(Id16([1; 16]));
        d.rev_id = Some(Id16([2; 16]));
        assert!(d.check().is_err(), "delete without parents");
        d.parents = Some(vec![Id16([3; 16])]);
        assert!(d.check().is_ok());
        d.ct_hash = Some(Hash32::ZERO);
        assert!(d.check().is_err());
        assert!(Entry::body(KIND_CHECKPOINT, 0).check().is_ok());
        let mut zero = Entry::body(KIND_CHECKPOINT, 0);
        zero.seen = Some(
            [(
                Id16([1; 16]),
                SeenPos {
                    seq: 0,
                    hash: Hash32::ZERO,
                },
            )]
            .into(),
        );
        assert!(zero.check().is_err());
        assert!(Entry::body(9, 0).check().is_err());
    }

    #[test]
    fn unknown_keys_are_ignored_and_hash_covers_stored_bytes() {
        // A map with an extra key 20 decodes; the hash is over the stored bytes.
        let mut stored = Vec::new();
        let mut e = minicbor::Encoder::new(&mut stored);
        e.map(5).unwrap();
        e.u8(0).unwrap().u64(1).unwrap();
        e.u8(1).unwrap().encode(Hash32::ZERO).unwrap();
        e.u8(2).unwrap().u8(KIND_CHECKPOINT).unwrap();
        e.u8(3).unwrap().i64(0).unwrap();
        e.u8(20).unwrap().str("future").unwrap();
        let entry: Entry = minicbor::decode(&stored).unwrap();
        assert_eq!(entry.kind, KIND_CHECKPOINT);
        assert_ne!(
            entry_hash(&Hash32::ZERO, &stored),
            entry_hash(&Hash32::ZERO, &stored[..stored.len() - 1])
        );
    }
}
