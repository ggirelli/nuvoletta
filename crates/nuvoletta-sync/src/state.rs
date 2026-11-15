//! Local sync state under `.nuvoletta/sync-state/` (`SYNC_PROTOCOL.md` section 6.1).
//!
//! Every file except `device-id` and `revs/` is one CBOR document `[kind, value]` sealed as
//! envelope type 4 (key `K_local`) and written atomically; the `kind` tag stops a file being
//! swapped for another one. `revs/<rev>.nvobj` holds verbatim copies of type 1 revision envelopes
//! (already encrypted with `K_note`). Everything but `device-id` and `journal` is rebuildable from
//! the remote logs; a state file that fails to open is treated as lost and rebuilt, but a corrupt
//! `journal` is an error (it may hold unsent edits).

use crate::error::{Error, Result};
use crate::graph::NoteGraph;
use crate::ids::{DeviceId, Hash32, Id16, RevId};
use crate::log::SeenPos;
use minicbor::{Decode, Encode};
use nuvoletta_crypto::{ObjType, ObjectMeta, Subkeys, open, seal};
use nuvoletta_storage::fsutil::{read_regular, write_atomic};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Directory of the sync state inside `.nuvoletta/`.
pub const STATE_DIR: &str = "sync-state";
const REVS_DIR: &str = "revs";
const MAX_STATE_FILE: u64 = 256 * 1024 * 1024;

/// An own change not yet sealed into a segment (or whose object is not yet uploaded).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Intent {
    /// Note id.
    #[n(0)]
    pub note: Id16,
    /// `KIND_PUT` or `KIND_DELETE`.
    #[n(1)]
    pub kind: u8,
    /// Revision id (`object_id` or `tombstone_id`).
    #[n(2)]
    pub rev: RevId,
    /// Object hash (PUT).
    #[n(3)]
    #[cbor(skip_if = "Option::is_none")]
    pub ct_hash: Option<Hash32>,
    /// Parents.
    #[n(4)]
    pub parents: Vec<RevId>,
    /// Creation time (ms, informational).
    #[n(5)]
    pub time: i64,
    /// The object file is confirmed present on the remote (always true for DELETE).
    #[n(6)]
    pub uploaded: bool,
}

/// A segment sealed but not yet confirmed uploaded.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Sealed {
    /// Sequence number of the first entry (the file name).
    #[n(0)]
    pub first_seq: u64,
    /// The exact envelope bytes; every retry uploads these.
    #[n(1)]
    #[cbor(with = "minicbor::bytes")]
    pub bytes: Vec<u8>,
}

/// The journal: own entries not yet confirmed uploaded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Journal {
    /// Changes waiting to be sealed.
    #[n(0)]
    pub intents: Vec<Intent>,
    /// The sealed segment being uploaded.
    #[n(1)]
    #[cbor(skip_if = "Option::is_none")]
    pub sealed: Option<Sealed>,
    /// Number of own entries sealed so far (cross-check against `peers`).
    #[n(2)]
    pub own_len: u64,
    /// Time of the last checkpoint or `seen` attachment (ms).
    #[n(3)]
    pub last_checkpoint_ms: i64,
    /// The seen vector last written, to detect change.
    #[n(4)]
    pub last_seen: BTreeMap<DeviceId, SeenPos>,
    /// Foreign PUT/DELETE entries were applied since the last checkpoint was written.
    #[n(5)]
    pub foreign_since_checkpoint: bool,
    /// The device id this journal belongs to (authoritative over the `device-id` file, which
    /// is rewritten after the journal when the id changes).
    #[n(6)]
    #[cbor(skip_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    /// Uploads that found a half-written file under the name they need, on a provider where
    /// that is permanent (`Capabilities::authoritative`): when it was first seen. Keyed by the
    /// revision id (object) or by the own device id (log segment).
    #[n(7)]
    #[cbor(skip_if = "Option::is_none")]
    pub blocked: Option<BTreeMap<Id16, i64>>,
}

/// Evidence of a fork of a device id (section 10.4).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct ForkRec {
    /// First sequence number with two different entries; entries from here on are not applied.
    #[n(0)]
    pub seq: u64,
    /// Distinct hashes seen at that sequence number (or the claimed predecessor hashes).
    #[n(1)]
    pub hashes: Vec<Hash32>,
}

/// What is known about one device's log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Peer {
    /// Verified entry hashes, index `seq - 1`: the contiguous prefix applied locally.
    #[n(0)]
    pub hashes: Vec<Hash32>,
    /// Segments seen and verified: `first_seq -> last_seq`.
    #[n(1)]
    pub segments: BTreeMap<u64, u64>,
    /// First time seen (ms).
    #[n(2)]
    pub first_seen: i64,
    /// Last time its log advanced (ms).
    #[n(3)]
    pub last_seen: i64,
    /// Latest checkpoint claims of this device: what it verified of others.
    #[n(4)]
    pub claims: BTreeMap<DeviceId, SeenPos>,
    /// Fork record, if one was detected: the stream is frozen below `fork.seq`.
    #[n(5)]
    #[cbor(skip_if = "Option::is_none")]
    pub fork: Option<ForkRec>,
    /// Sequence number of the last checkpoint seen from this device.
    #[n(6)]
    pub last_checkpoint_seq: u64,
}

impl Peer {
    /// Highest verified sequence number.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.hashes.len() as u64
    }

    /// Hash of the highest verified entry (zero if none).
    #[must_use]
    pub fn last_hash(&self) -> Hash32 {
        self.hashes.last().copied().unwrap_or(Hash32::ZERO)
    }
}

/// Per-note sync record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct NoteRec {
    /// The version DAG built from verified entries and own unsealed intents.
    #[n(0)]
    pub graph: NoteGraph,
    /// The revision (or tombstone) the local vault currently holds for this note.
    #[n(1)]
    #[cbor(skip_if = "Option::is_none")]
    pub local_rev: Option<RevId>,
    /// The last local revision confirmed uploaded and referenced, or applied from the remote.
    #[n(2)]
    #[cbor(skip_if = "Option::is_none")]
    pub base: Option<RevId>,
    /// The local revision was introduced by entries removed by a fork freeze: no automatic
    /// change is made until the user looks at it.
    #[n(3)]
    pub needs_attention: bool,
    /// A planned vault change (install of this revision, or removal for a tombstone) that has
    /// been persisted but may not have been written to the vault yet. While set, a missing or
    /// old vault note is not a local edit or delete.
    #[n(4)]
    #[cbor(skip_if = "Option::is_none")]
    pub applying: Option<RevId>,
}

/// An entry of the last remote listing.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Inv {
    /// Size at the time the file was processed.
    #[n(0)]
    pub size: u64,
    /// Etag at that time.
    #[n(1)]
    #[cbor(skip_if = "Option::is_none")]
    pub etag: Option<String>,
    /// When it was processed (ms).
    #[n(2)]
    pub seen_at: i64,
}

/// Retry bookkeeping of something that has not arrived yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Retry {
    /// Cycles in which it was missing.
    #[n(0)]
    pub tries: u32,
    /// First time it was missing (ms).
    #[n(1)]
    pub since: i64,
}

/// A verified log segment that arrived after a gap and waits for the missing segments
/// (section 6.3 step 4). Only the bytes as stored remotely (already AEAD-protected with K_log)
/// are kept, inside the K_local-sealed `pending` file; applying it re-verifies everything.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Buffered {
    /// Device that wrote it.
    #[n(0)]
    pub device: DeviceId,
    /// Listed size when it was fetched (with `etag`, tells whether the remote file changed).
    #[n(1)]
    pub size: u64,
    /// Listed etag or modification stamp when it was fetched.
    #[n(2)]
    #[cbor(skip_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The envelope bytes.
    #[n(3)]
    #[cbor(with = "minicbor::bytes")]
    pub bytes: Vec<u8>,
}

/// Things waiting for propagation (section 6.3 steps 4 and 5), plus own orphan objects.
///
/// Local and rebuildable: a lost or unreadable `pending` only costs retry counters, a refetch of
/// buffered segments, and the chance to garbage collect old orphan objects.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Pending {
    /// Devices whose stream stops at a gap, by the sequence number expected next.
    #[n(0)]
    pub gaps: BTreeMap<DeviceId, (u64, Retry)>,
    /// Referenced objects not yet fetched or failing verification.
    #[n(1)]
    pub objects: BTreeMap<RevId, Retry>,
    /// Entries claimed by a checkpoint that this device cannot see: possibly withheld.
    #[n(2)]
    pub withheld: BTreeMap<DeviceId, (u64, Retry)>,
    /// Segments buffered after a gap, by remote name.
    #[n(3)]
    #[cbor(default)]
    pub buffered: BTreeMap<String, Buffered>,
    /// Own objects uploaded but dropped from the journal before any entry referenced them (an
    /// intermediate local revision, section 6.2), with the time that happened: orphans that
    /// section 9 rule 5 lets this device delete after a week.
    #[n(4)]
    #[cbor(default)]
    pub orphans: BTreeMap<RevId, i64>,
}

#[derive(Clone, Copy, Default)]
struct Dirty {
    journal: bool,
    peers: bool,
    notes: bool,
    inventory: bool,
    pending: bool,
    header_gen: bool,
}

/// The in-memory sync state, persisted file by file.
pub struct State {
    dir: PathBuf,
    /// This device's id.
    pub device_id: DeviceId,
    /// Unsent own entries.
    pub journal: Journal,
    /// Per-device log knowledge, including this device.
    pub peers: BTreeMap<DeviceId, Peer>,
    /// Per-note records.
    pub notes: BTreeMap<Id16, NoteRec>,
    /// Remote files already processed.
    pub inventory: BTreeMap<String, Inv>,
    /// Retry bookkeeping.
    pub pending: Pending,
    /// Highest vault header generation seen.
    pub max_header_gen: u32,
    /// The notes/peers state did not exist or was unreadable at load: rebuilding from the remote.
    pub rebuilt: bool,
    dirty: Dirty,
}

impl core::fmt::Debug for State {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "State(device {})", self.device_id)
    }
}

fn seal_doc<T: Encode<()>>(keys: &Subkeys, kind: &str, value: &T) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut e = minicbor::Encoder::new(&mut buf);
    e.array(2)
        .and_then(|e| e.str(kind))
        .and_then(|e| e.encode(value))
        .map_err(|_| Error::State("encode"))?;
    let meta = ObjectMeta::new(ObjType::LocalState, [0; 16])?;
    Ok(seal(keys, &meta, &buf)?)
}

fn open_doc<T>(keys: &Subkeys, kind: &str, bytes: &[u8]) -> Result<T>
where
    T: for<'b> Decode<'b, ()>,
{
    let opened = open(keys, bytes)?;
    if opened.meta.obj_type != ObjType::LocalState {
        return Err(Error::State("wrong object type"));
    }
    let mut d = minicbor::Decoder::new(&opened.plaintext);
    if d.array().map_err(|_| Error::State("decode"))? != Some(2) {
        return Err(Error::State("bad document"));
    }
    if d.str().map_err(|_| Error::State("decode"))? != kind {
        return Err(Error::State("document kind mismatch"));
    }
    d.decode().map_err(|_| Error::State("decode"))
}

/// Fuzzing entry point: feed `value` (raw CBOR, not validated) as the value of every state
/// document kind, sealed under `keys`, through the real decoder. Returns the kinds that decoded
/// and did not survive a re-encode and decode unchanged (always empty unless there is a bug).
#[doc(hidden)]
#[must_use]
pub fn fuzz_decode_docs(keys: &Subkeys, value: &[u8]) -> Vec<&'static str> {
    fn one<T>(keys: &Subkeys, kind: &'static str, value: &[u8], bad: &mut Vec<&'static str>)
    where
        T: for<'b> Decode<'b, ()> + Encode<()> + PartialEq,
    {
        let mut buf = Vec::new();
        let mut e = minicbor::Encoder::new(&mut buf);
        if e.array(2).and_then(|e| e.str(kind)).is_err() {
            return;
        }
        buf.extend_from_slice(value);
        let Ok(meta) = ObjectMeta::new(ObjType::LocalState, [0; 16]) else {
            return;
        };
        let Ok(sealed) = seal(keys, &meta, &buf) else {
            return;
        };
        let Ok(first) = open_doc::<T>(keys, kind, &sealed) else {
            return;
        };
        let again = seal_doc(keys, kind, &first).and_then(|b| open_doc::<T>(keys, kind, &b));
        if !again.is_ok_and(|second| second == first) {
            bad.push(kind);
        }
    }
    let mut bad = Vec::new();
    one::<Journal>(keys, "journal", value, &mut bad);
    one::<BTreeMap<DeviceId, Peer>>(keys, "peers", value, &mut bad);
    one::<BTreeMap<Id16, NoteRec>>(keys, "notes", value, &mut bad);
    one::<BTreeMap<String, Inv>>(keys, "inventory", value, &mut bad);
    one::<Pending>(keys, "pending", value, &mut bad);
    one::<u32>(keys, "max-header-gen", value, &mut bad);
    bad
}

fn load_file<T>(dir: &Path, keys: &Subkeys, name: &str) -> Result<Option<T>>
where
    T: for<'b> Decode<'b, ()>,
{
    match read_regular(&dir.join(name), MAX_STATE_FILE) {
        Ok(bytes) => open_doc(keys, name, &bytes).map(Some),
        Err(nuvoletta_storage::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

impl State {
    /// Load the state from `dir` (creating it, and a fresh device id, if missing).
    ///
    /// # Errors
    /// I/O errors, or a corrupt `journal` or `device-id`.
    pub fn load(dir: &Path, keys: &Subkeys) -> Result<Self> {
        fs::create_dir_all(dir.join(REVS_DIR))?;
        // Our own interrupted writes only ever hold ciphertext.
        for d in [dir.to_path_buf(), dir.join(REVS_DIR)] {
            nuvoletta_storage::fsutil::clean_temp_files(&d)?;
        }
        let device_id = match fs::read_to_string(dir.join("device-id")) {
            Ok(s) => Id16::from_hex(s.trim()).ok_or(Error::State("bad device-id"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = Id16::random()?;
                write_atomic(dir, "device-id", format!("{id}\n").as_bytes())?;
                id
            }
            Err(e) => return Err(e.into()),
        };
        let journal: Option<Journal> = load_file(dir, keys, "journal")?;
        let peers: Result<Option<BTreeMap<DeviceId, Peer>>> = load_file(dir, keys, "peers");
        let notes: Result<Option<BTreeMap<Id16, NoteRec>>> = load_file(dir, keys, "notes");
        let (peers, notes, rebuilt) = match (peers, notes) {
            (Ok(Some(p)), Ok(Some(n))) => (p, n, false),
            _ => (BTreeMap::new(), BTreeMap::new(), true),
        };
        let inventory = if rebuilt {
            BTreeMap::new()
        } else {
            load_file(dir, keys, "inventory")
                .ok()
                .flatten()
                .unwrap_or_default()
        };
        let pending = load_file(dir, keys, "pending")
            .ok()
            .flatten()
            .unwrap_or_default();
        let max_header_gen = load_file::<u32>(dir, keys, "max-header-gen")
            .ok()
            .flatten()
            .unwrap_or(0);
        let mut s = Self {
            dir: dir.to_path_buf(),
            device_id,
            journal: journal.unwrap_or_default(),
            peers,
            notes,
            inventory,
            pending,
            max_header_gen,
            rebuilt,
            dirty: Dirty::default(),
        };
        if rebuilt {
            s.dirty.peers = true;
            s.dirty.notes = true;
        }
        Ok(s)
    }

    /// Mark the journal changed.
    pub fn touch_journal(&mut self) {
        self.dirty.journal = true;
    }
    /// Mark peers changed.
    pub fn touch_peers(&mut self) {
        self.dirty.peers = true;
    }
    /// Mark notes changed.
    pub fn touch_notes(&mut self) {
        self.dirty.notes = true;
    }
    /// Mark the inventory changed.
    pub fn touch_inventory(&mut self) {
        self.dirty.inventory = true;
    }
    /// Mark pending changed.
    pub fn touch_pending(&mut self) {
        self.dirty.pending = true;
    }
    /// Mark the header generation changed.
    pub fn touch_header_gen(&mut self) {
        self.dirty.header_gen = true;
    }

    /// Persist every changed file atomically. The journal is written first, so a crash between
    /// files never loses unsent entries.
    ///
    /// # Errors
    /// I/O or crypto failures.
    pub fn save(&mut self, keys: &Subkeys) -> Result<()> {
        let d = self.dir.clone();
        if self.dirty.journal {
            write_atomic(&d, "journal", &seal_doc(keys, "journal", &self.journal)?)?;
            self.dirty.journal = false;
        }
        if self.dirty.notes {
            write_atomic(&d, "notes", &seal_doc(keys, "notes", &self.notes)?)?;
            self.dirty.notes = false;
        }
        if self.dirty.peers {
            write_atomic(&d, "peers", &seal_doc(keys, "peers", &self.peers)?)?;
            self.dirty.peers = false;
        }
        if self.dirty.inventory {
            write_atomic(
                &d,
                "inventory",
                &seal_doc(keys, "inventory", &self.inventory)?,
            )?;
            self.dirty.inventory = false;
        }
        if self.dirty.pending {
            write_atomic(&d, "pending", &seal_doc(keys, "pending", &self.pending)?)?;
            self.dirty.pending = false;
        }
        if self.dirty.header_gen {
            write_atomic(
                &d,
                "max-header-gen",
                &seal_doc(keys, "max-header-gen", &self.max_header_gen)?,
            )?;
            self.dirty.header_gen = false;
        }
        Ok(())
    }

    /// Replace the device id (fork recovery): written atomically.
    ///
    /// # Errors
    /// I/O failures.
    pub fn set_device_id(&mut self, id: DeviceId) -> Result<()> {
        write_atomic(&self.dir, "device-id", format!("{id}\n").as_bytes())?;
        self.device_id = id;
        Ok(())
    }

    fn rev_path(&self, rev: &RevId) -> PathBuf {
        self.dir.join(REVS_DIR).join(format!("{rev}.nvobj"))
    }

    /// Cache a verbatim revision envelope.
    ///
    /// # Errors
    /// I/O failures.
    pub fn put_rev(&self, rev: &RevId, bytes: &[u8]) -> Result<()> {
        write_atomic(&self.dir.join(REVS_DIR), &format!("{rev}.nvobj"), bytes)?;
        Ok(())
    }

    /// Read a cached revision envelope, if present.
    ///
    /// # Errors
    /// I/O failures other than "not found".
    pub fn get_rev(&self, rev: &RevId) -> Result<Option<Vec<u8>>> {
        match read_regular(&self.rev_path(rev), MAX_STATE_FILE) {
            Ok(b) => Ok(Some(b)),
            Err(nuvoletta_storage::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Size in bytes of a cached revision, if present.
    #[must_use]
    pub fn rev_size(&self, rev: &RevId) -> Option<u64> {
        fs::symlink_metadata(self.rev_path(rev))
            .ok()
            .filter(fs::Metadata::is_file)
            .map(|m| m.len())
    }

    /// Whether a revision is cached.
    #[must_use]
    pub fn has_rev(&self, rev: &RevId) -> bool {
        fs::symlink_metadata(self.rev_path(rev)).is_ok_and(|m| m.is_file())
    }

    /// Remove a cached revision (never uploaded intermediate revisions, pruning).
    ///
    /// # Errors
    /// I/O failures other than "not found".
    pub fn remove_rev(&self, rev: &RevId) -> Result<()> {
        match fs::remove_file(self.rev_path(rev)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Cached revision ids.
    ///
    /// # Errors
    /// I/O failures.
    pub fn cached_revs(&self) -> Result<Vec<RevId>> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.dir.join(REVS_DIR))? {
            let name = e?.file_name();
            if let Some(id) = name
                .to_str()
                .and_then(|n| n.strip_suffix(".nvobj"))
                .and_then(Id16::from_hex)
            {
                out.push(id);
            }
        }
        Ok(out)
    }
}
