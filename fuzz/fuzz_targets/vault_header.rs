//! Vault header parser: arbitrary bytes, structural validation before any key derivation.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_crypto::VaultHeader;

fuzz_target!(|data: &[u8]| {
    let Ok(h) = VaultHeader::parse(data) else {
        return;
    };
    // Parsing is lossless and the accepted KDF cost is bounded.
    assert_eq!(h.as_bytes().as_slice(), data);
    assert!(h.generation() >= 1);
    let p = h.kdf_params();
    assert!(
        (8192..=1_048_576).contains(&p.m_kib) && (1..=16).contains(&p.t) && (1..=8).contains(&p.p)
    );
    assert_eq!(VaultHeader::parse(h.as_bytes()).as_ref(), Ok(&h));
    // An unauthenticated header must never unlock. Only run cheap KDF settings: the fuzzer is
    // free to ask for 1 GiB.
    if p.m_kib <= 16_384 && p.t <= 2 {
        assert!(h.unlock_with_passphrase("not the passphrase").is_err());
    }
});
