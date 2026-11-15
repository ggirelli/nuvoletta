//! Shared helpers of the fuzz targets.
//!
//! Keys are derived once per process from a throwaway vault at the cheapest accepted Argon2id
//! cost. They are random, so seed files never contain ciphertext: targets that need to get past
//! authentication seal the fuzzer's bytes themselves and then hand them to the parser.

use nuvoletta_crypto::{KdfParams, Subkeys, Vault};
use std::sync::OnceLock;

static VAULT: OnceLock<Vault> = OnceLock::new();

/// The process-wide throwaway keys.
///
/// # Panics
/// If the throwaway vault cannot be created (the OS RNG failed).
pub fn keys() -> &'static Subkeys {
    VAULT
        .get_or_init(|| {
            Vault::create("fuzz throwaway passphrase", KdfParams::MIN_ACCEPTED)
                .expect("create throwaway vault")
                .vault
        })
        .keys()
}

/// Split the first byte off as a selector.
pub fn split_mode(data: &[u8]) -> (u8, &[u8]) {
    match data.split_first() {
        Some((m, rest)) => (*m, rest),
        None => (0, data),
    }
}
