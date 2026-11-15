//! Writes the seed inputs that need code to produce (CBOR payloads) into `fuzz/seeds/`.
//!
//! Run with `just fuzz-seeds`. Text seeds and the test-vector seeds are plain files in the same
//! directory tree. Seeds never contain key material or real notes.

use nuvoletta_crypto::{NotePayload, ObjType, open};
use nuvoletta_sync::ids::{Hash32, Id16};
use nuvoletta_sync::log::{Entry, KIND_CHECKPOINT, KIND_DELETE, KIND_PUT, seal_segment};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn put(dir: &str, name: &str, bytes: &[u8]) {
    let d = Path::new("seeds").join(dir);
    fs::create_dir_all(&d).expect("create seed dir");
    fs::write(d.join(name), bytes).expect("write seed");
}

fn cbor<T: minicbor::Encode<()>>(v: &T) -> Vec<u8> {
    minicbor::to_vec(v).expect("encode")
}

fn main() {
    let keys = nuvoletta_fuzz::keys();

    // A note revision payload.
    let payload = NotePayload {
        schema_version: 1,
        note_id: [7; 16],
        title: "Seed".into(),
        path: "a/b".into(),
        tags: vec!["x".into(), "y z".into()],
        created_at: 1,
        modified_at: 2,
        body: "# Seed\n\n[[Other]] body\n".into(),
    };
    put("note_payload", "valid", &payload.encode().expect("encode"));

    // A three-entry segment: PUT, DELETE, CHECKPOINT, chained.
    let mut put_entry = Entry::body(KIND_PUT, 1_000);
    put_entry.note_id = Some(Id16([1; 16]));
    put_entry.rev_id = Some(Id16([2; 16]));
    put_entry.ct_hash = Some(Hash32([3; 32]));
    put_entry.parents = Some(vec![Id16([4; 16])]);
    let mut del = Entry::body(KIND_DELETE, 2_000);
    del.note_id = Some(Id16([1; 16]));
    del.rev_id = Some(Id16([5; 16]));
    del.parents = Some(vec![Id16([2; 16])]);
    let mut cp = Entry::body(KIND_CHECKPOINT, 3_000);
    cp.seen = Some(BTreeMap::new());
    let seg = seal_segment(
        keys,
        Id16([3; 16]),
        1,
        Hash32::ZERO,
        vec![put_entry, del, cp],
    )
    .expect("seal segment");
    let opened = open(keys, &seg.bytes).expect("open own segment");
    assert_eq!(opened.meta.obj_type, ObjType::LogSegment);
    // Selector byte 1: seal the rest as a segment plaintext.
    let mut input = vec![1u8];
    input.extend_from_slice(&opened.plaintext);
    put("log_segment", "three_entries", &input);

    // State documents: the value of every kind starts from empty/default values.
    use nuvoletta_sync::state::{Journal, Pending};
    put("state_docs", "journal", &cbor(&Journal::default()));
    put("state_docs", "pending", &cbor(&Pending::default()));
    put("state_docs", "empty_map", &[0xa0]);
    put("state_docs", "gen", &cbor(&1u32));
    let inv: BTreeMap<String, u8> = BTreeMap::new();
    put("state_docs", "empty_inventory", &cbor(&inv));
}
