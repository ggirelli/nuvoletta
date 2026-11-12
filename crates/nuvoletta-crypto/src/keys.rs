//! Key types and HKDF subkey derivation (CRYPTO_FORMAT.md section 1).

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Length of a vault id.
pub const VAULT_ID_LEN: usize = 16;

/// A 32-byte secret. No `Debug` output of contents, no `Clone`.
pub(crate) type Key32 = Zeroizing<[u8; 32]>;

/// The data-encryption key. Never stored in the clear.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Dek(pub(crate) [u8; 32]);

impl core::fmt::Debug for Dek {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Dek(<redacted>)")
    }
}

/// Subkeys derived from the DEK.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Subkeys {
    pub(crate) vault_id: [u8; VAULT_ID_LEN],
    pub(crate) k_header: [u8; 32],
    pub(crate) k_note: [u8; 32],
    pub(crate) k_log: [u8; 32],
    pub(crate) k_local: [u8; 32],
}

impl core::fmt::Debug for Subkeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Subkeys(<redacted>)")
    }
}

/// HKDF-SHA256 with `salt = vault_id`, `info = "nuvoletta/v1/" || label`.
fn expand(ikm: &[u8], vault_id: &[u8; VAULT_ID_LEN], info: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(vault_id), ikm);
    let mut out = [0u8; 32];
    // 32 <= 255 * 32: expand cannot fail.
    hk.expand(info, &mut out)
        .expect("hkdf output length is valid");
    out
}

pub(crate) fn derive_labelled(dek: &Dek, vault_id: &[u8; VAULT_ID_LEN], label: &str) -> [u8; 32] {
    let mut info = b"nuvoletta/v1/".to_vec();
    info.extend_from_slice(label.as_bytes());
    expand(&dek.0, vault_id, &info)
}

/// `KEK_rk = HKDF-SHA256(ikm = RK, salt = vault_id, info = "nuvoletta/v1/recovery-kek")`.
pub(crate) fn derive_recovery_kek(rk: &[u8; 32], vault_id: &[u8; VAULT_ID_LEN]) -> Key32 {
    Zeroizing::new(expand(rk, vault_id, b"nuvoletta/v1/recovery-kek"))
}

impl Subkeys {
    /// Derive all subkeys from the DEK.
    #[must_use]
    pub fn derive(dek: &Dek, vault_id: &[u8; VAULT_ID_LEN]) -> Self {
        Self {
            vault_id: *vault_id,
            k_header: derive_labelled(dek, vault_id, "header-auth"),
            k_note: derive_labelled(dek, vault_id, "note-content"),
            k_log: derive_labelled(dek, vault_id, "device-log"),
            k_local: derive_labelled(dek, vault_id, "local-cache"),
        }
    }

    /// The vault id these keys belong to.
    #[must_use]
    pub fn vault_id(&self) -> &[u8; VAULT_ID_LEN] {
        &self.vault_id
    }
}
