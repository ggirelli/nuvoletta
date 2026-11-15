//! Note revision payload: CBOR decode of the plaintext of an authenticated envelope.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_crypto::NotePayload;

fuzz_target!(|data: &[u8]| {
    let Ok(p) = NotePayload::decode(data) else {
        return;
    };
    // What decodes re-encodes and decodes to the same value.
    let again = p.encode().expect("a decoded payload encodes");
    assert!(NotePayload::decode(&again).is_ok_and(|q| q == p));
    let _ = p.check_scope(&p.note_id);
});
