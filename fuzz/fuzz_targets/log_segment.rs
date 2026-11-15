//! Log segments: raw envelope bytes, and arbitrary plaintext sealed as a segment so that the CBOR
//! entry decoding and the chain/sequence verification are reached.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_crypto::{ObjType, ObjectMeta, seal};
use nuvoletta_fuzz::{keys, split_mode};
use nuvoletta_sync::log::{SegmentError, entry_hash, open_segment};

fuzz_target!(|data: &[u8]| {
    let keys = keys();
    let (mode, rest) = split_mode(data);
    if mode & 1 == 0 {
        assert!(matches!(
            open_segment(keys, rest),
            Err(SegmentError::Unreadable)
        ));
        return;
    }
    let Ok(meta) = ObjectMeta::new(ObjType::LogSegment, [3; 16]) else {
        return;
    };
    let Ok(sealed) = seal(keys, &meta, rest) else {
        return;
    };
    let Ok(seg) = open_segment(keys, &sealed) else {
        return;
    };
    // Invariants of a verified segment (SYNC_PROTOCOL.md section 5.2).
    assert_eq!(seg.device.0, [3; 16]);
    assert!(!seg.entries.is_empty());
    assert!(seg.first_seq() >= 1);
    for pair in seg.entries.windows(2) {
        assert_eq!(pair[1].entry.seq, pair[0].entry.seq + 1);
        assert_eq!(pair[1].entry.prev, pair[0].hash);
    }
    for pe in &seg.entries {
        let mut stored = Vec::new();
        minicbor::encode(&pe.entry, &mut stored).expect("entry encodes");
        let _ = entry_hash(&pe.entry.prev, &stored);
        let _ = pe.entry.check();
        let _ = pe.entry.parents_sorted();
    }
});
