//! Local sync state documents (journal, peers, notes, inventory, pending, header generation):
//! the CBOR decoders of everything `State::load` reads after authentication.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_fuzz::keys;

fuzz_target!(|data: &[u8]| {
    let bad = nuvoletta_sync::state::fuzz_decode_docs(keys(), data);
    assert!(bad.is_empty(), "decode/encode round trip changed {bad:?}");
});
