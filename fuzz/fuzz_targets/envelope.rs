//! Object envelope: the pre-AEAD header parsing on raw bytes, and sealed-then-tampered envelopes.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_crypto::{ObjType, ObjectMeta, open, seal};
use nuvoletta_fuzz::{keys, split_mode};

fuzz_target!(|data: &[u8]| {
    let keys = keys();
    let (mode, rest) = split_mode(data);
    if mode & 1 == 0 {
        // Raw bytes: must be rejected without panicking (they cannot authenticate).
        assert!(open(keys, rest).is_err());
        return;
    }
    // A valid envelope round-trips, and every single-bit change of it is rejected: the whole
    // envelope, header included, is authenticated.
    let ty = match (mode >> 1) % 4 {
        0 => ObjType::NoteRevision,
        1 => ObjType::LogSegment,
        2 => ObjType::Snapshot,
        _ => ObjType::LocalState,
    };
    let scope = if ty == ObjType::LocalState {
        [0; 16]
    } else {
        [7; 16]
    };
    let Ok(meta) = ObjectMeta::new(ty, scope) else {
        return;
    };
    let Ok(sealed) = seal(keys, &meta, rest) else {
        return;
    };
    let opened = open(keys, &sealed).expect("a sealed envelope opens");
    assert_eq!(&*opened.plaintext, rest);
    assert_eq!(opened.meta.obj_type, ty);
    assert_eq!(opened.meta.scope_id, scope);
    let at = (usize::from(mode >> 3) * 251 + rest.len()) % sealed.len();
    let mut bad = sealed.clone();
    bad[at] ^= 1 << (rest.len() % 8);
    assert!(open(keys, &bad).is_err(), "tampered byte {at} accepted");
    assert!(open(keys, &sealed[..sealed.len() - 1]).is_err());
});
