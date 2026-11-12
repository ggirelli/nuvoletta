//! Vault header (CRYPTO_FORMAT.md section 2) and vault operations.

use crate::error::{Error, Result};
use crate::keys::{Dek, Subkeys, VAULT_ID_LEN, derive_recovery_kek};
use crate::recovery::RecoveryKey;
use crate::rng::{OsRandom, RandomSource};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// Serialized header length.
pub const HEADER_LEN: usize = 236;
/// Minimum passphrase length in characters (after NFKC).
pub const MIN_PASSPHRASE_CHARS: usize = 12;

const MAGIC: [u8; 8] = *b"NVLTHDR\0";
const FORMAT_VERSION: u16 = 1;
const KDF_ARGON2ID: u8 = 1;
const WRAP_XCHACHA: u8 = 1;
const OFF_GEN: usize = 10;
const OFF_VAULT_ID: usize = 14;
const OFF_KDF: usize = 30;
const OFF_SALT: usize = 43;
const OFF_WRAP_ALG: usize = 59;
const OFF_PW_NONCE: usize = 60;
const OFF_PW_WRAP: usize = 84;
const OFF_RK_NONCE: usize = 132;
const OFF_RK_WRAP: usize = 156;
const OFF_MAC: usize = 204;
const WRAP_LEN: usize = 48;

/// Argon2id parameters (stored in the header, validated on parse).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Iterations.
    pub t: u32,
    /// Parallelism.
    pub p: u32,
}

impl KdfParams {
    /// Production defaults: 64 MiB, t = 3, p = 1.
    pub const DEFAULT: Self = Self {
        m_kib: 65536,
        t: 3,
        p: 1,
    };
    /// Cheapest params the parser accepts; for tests only (never for real vaults).
    pub const MIN_ACCEPTED: Self = Self {
        m_kib: 8192,
        t: 1,
        p: 1,
    };

    fn validate(&self) -> Result<()> {
        if (8192..=1_048_576).contains(&self.m_kib)
            && (1..=16).contains(&self.t)
            && (1..=8).contains(&self.p)
        {
            Ok(())
        } else {
            Err(Error::KdfParamsOutOfRange)
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A parsed, structurally validated (but not yet authenticated) vault header.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultHeader {
    bytes: [u8; HEADER_LEN],
}

impl core::fmt::Debug for VaultHeader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VaultHeader")
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn arr<const N: usize>(b: &[u8], off: usize) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&b[off..off + N]);
    a
}

impl VaultHeader {
    /// Parse and validate structure (length, magic, version, ids, KDF ranges).
    /// Performs no key derivation and never panics on arbitrary input.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; HEADER_LEN] = bytes
            .try_into()
            .map_err(|_| Error::MalformedHeader("wrong length"))?;
        if bytes[..8] != MAGIC {
            return Err(Error::MalformedHeader("bad magic"));
        }
        if u16::from_be_bytes([bytes[8], bytes[9]]) != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if bytes[OFF_KDF] != KDF_ARGON2ID || bytes[OFF_WRAP_ALG] != WRAP_XCHACHA {
            return Err(Error::UnsupportedAlgorithm);
        }
        let h = Self { bytes };
        if h.generation() == 0 {
            return Err(Error::MalformedHeader("generation is zero"));
        }
        h.kdf_params().validate()?;
        Ok(h)
    }

    /// Serialized bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; HEADER_LEN] {
        &self.bytes
    }

    /// Header generation (rollback hint).
    #[must_use]
    pub fn generation(&self) -> u32 {
        be32(&self.bytes, OFF_GEN)
    }

    /// Vault id.
    #[must_use]
    pub fn vault_id(&self) -> [u8; VAULT_ID_LEN] {
        arr(&self.bytes, OFF_VAULT_ID)
    }

    /// KDF parameters.
    #[must_use]
    pub fn kdf_params(&self) -> KdfParams {
        KdfParams {
            m_kib: be32(&self.bytes, OFF_KDF + 1),
            t: be32(&self.bytes, OFF_KDF + 5),
            p: be32(&self.bytes, OFF_KDF + 9),
        }
    }

    /// Unlock with the passphrase.
    pub fn unlock_with_passphrase(&self, passphrase: &str) -> Result<Vault> {
        let pw = normalize(passphrase);
        let kek = argon_kek(&pw, &arr::<16>(&self.bytes, OFF_SALT), self.kdf_params())
            .map_err(|_| Error::Unlock)?;
        let dek = self.unwrap_dek(&kek, OFF_PW_NONCE, OFF_PW_WRAP, &pw_aad(&self.bytes))?;
        self.finish_unlock(dek)
    }

    /// Unlock with the recovery key.
    pub fn unlock_with_recovery_key(&self, rk: &RecoveryKey) -> Result<Vault> {
        let kek = derive_recovery_kek(&rk.0, &self.vault_id());
        let dek = self.unwrap_dek(&kek, OFF_RK_NONCE, OFF_RK_WRAP, &rk_aad(&self.bytes))?;
        self.finish_unlock(dek)
    }

    fn unwrap_dek(
        &self,
        kek: &[u8; 32],
        off_nonce: usize,
        off_wrap: usize,
        aad: &[u8],
    ) -> Result<Dek> {
        let cipher = XChaCha20Poly1305::new(kek.into());
        let nonce = XNonce::from_slice(&self.bytes[off_nonce..off_nonce + 24]);
        let pt = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &self.bytes[off_wrap..off_wrap + WRAP_LEN],
                    aad,
                },
            )
            .map_err(|_| Error::Unlock)?;
        let pt = Zeroizing::new(pt);
        let dek: [u8; 32] = pt.as_slice().try_into().map_err(|_| Error::Unlock)?;
        Ok(Dek(dek))
    }

    fn finish_unlock(&self, dek: Dek) -> Result<Vault> {
        let keys = Subkeys::derive(&dek, &self.vault_id());
        let mut mac = new_mac(&keys.k_header).map_err(|_| Error::Unlock)?;
        mac.update(&self.bytes[..OFF_MAC]);
        mac.verify_slice(&self.bytes[OFF_MAC..])
            .map_err(|_| Error::Unlock)?;
        Ok(Vault {
            header: self.clone(),
            dek,
            keys,
        })
    }
}

fn new_mac(key: &[u8]) -> Result<Hmac<Sha256>> {
    <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| Error::Authentication)
}

/// NFKC-normalise, no trimming.
fn normalize(pw: &str) -> Zeroizing<String> {
    Zeroizing::new(pw.nfkc().collect::<String>())
}

fn argon_kek(pw: &str, salt: &[u8; 16], p: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    p.validate()?;
    let params =
        Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|_| Error::KdfParamsOutOfRange)?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    a2.hash_password_into(pw.as_bytes(), salt, out.as_mut())
        .map_err(|_| Error::KdfParamsOutOfRange)?;
    Ok(out)
}

fn pw_aad(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes[..OFF_PW_NONCE].to_vec();
    v.extend_from_slice(b"pw");
    v
}

/// Recovery-wrap AAD: see "Implementation notes" in CRYPTO_FORMAT.md. It binds
/// magic, version and vault_id but not generation/KDF params/salt, so that a
/// passphrase change does not require the recovery key. `header_mac` still
/// authenticates the whole header after unwrap.
fn rk_aad(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes[..OFF_GEN].to_vec();
    v.extend_from_slice(&bytes[OFF_VAULT_ID..OFF_KDF]);
    v.extend_from_slice(b"rk");
    v
}

fn check_new_passphrase(pw: &str) -> Result<Zeroizing<String>> {
    let n = normalize(pw);
    if n.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(Error::PassphraseTooShort {
            min: MIN_PASSPHRASE_CHARS,
        });
    }
    Ok(n)
}

fn wrap(kek: &[u8; 32], nonce: &[u8; 24], dek: &Dek, aad: &[u8]) -> Result<[u8; WRAP_LEN]> {
    let ct = XChaCha20Poly1305::new(kek.into())
        .encrypt(XNonce::from_slice(nonce), Payload { msg: &dek.0, aad })
        .map_err(|_| Error::Authentication)?;
    ct.try_into().map_err(|_| Error::Authentication)
}

/// Fields that change when the passphrase wrap is rewritten.
struct Build<'a> {
    generation: u32,
    vault_id: [u8; 16],
    params: KdfParams,
    salt: [u8; 16],
    pw_nonce: [u8; 24],
    rk_nonce: [u8; 24],
    dek: &'a Dek,
    kek_pw: &'a [u8; 32],
    kek_rk: &'a [u8; 32],
}

impl Build<'_> {
    fn serialize(&self) -> Result<VaultHeader> {
        let mut b = [0u8; HEADER_LEN];
        b[..8].copy_from_slice(&MAGIC);
        b[8..10].copy_from_slice(&FORMAT_VERSION.to_be_bytes());
        b[OFF_GEN..OFF_GEN + 4].copy_from_slice(&self.generation.to_be_bytes());
        b[OFF_VAULT_ID..OFF_VAULT_ID + 16].copy_from_slice(&self.vault_id);
        b[OFF_KDF] = KDF_ARGON2ID;
        b[OFF_KDF + 1..OFF_KDF + 5].copy_from_slice(&self.params.m_kib.to_be_bytes());
        b[OFF_KDF + 5..OFF_KDF + 9].copy_from_slice(&self.params.t.to_be_bytes());
        b[OFF_KDF + 9..OFF_KDF + 13].copy_from_slice(&self.params.p.to_be_bytes());
        b[OFF_SALT..OFF_SALT + 16].copy_from_slice(&self.salt);
        b[OFF_WRAP_ALG] = WRAP_XCHACHA;
        b[OFF_PW_NONCE..OFF_PW_NONCE + 24].copy_from_slice(&self.pw_nonce);
        let pw_wrap = wrap(self.kek_pw, &self.pw_nonce, self.dek, &pw_aad(&b))?;
        b[OFF_PW_WRAP..OFF_PW_WRAP + WRAP_LEN].copy_from_slice(&pw_wrap);
        b[OFF_RK_NONCE..OFF_RK_NONCE + 24].copy_from_slice(&self.rk_nonce);
        let rk_wrap = wrap(self.kek_rk, &self.rk_nonce, self.dek, &rk_aad(&b))?;
        b[OFF_RK_WRAP..OFF_RK_WRAP + WRAP_LEN].copy_from_slice(&rk_wrap);
        let k_header = crate::keys::derive_labelled(self.dek, &self.vault_id, "header-auth");
        let k_header = Zeroizing::new(k_header);
        let mut mac = new_mac(k_header.as_ref())?;
        mac.update(&b[..OFF_MAC]);
        b[OFF_MAC..].copy_from_slice(&mac.finalize().into_bytes());
        Ok(VaultHeader { bytes: b })
    }
}

/// An unlocked vault: header, DEK and subkeys.
#[derive(Debug)]
pub struct Vault {
    header: VaultHeader,
    pub(crate) dek: Dek,
    keys: Subkeys,
}

/// Result of creating a vault.
#[derive(Debug)]
pub struct CreatedVault {
    /// The unlocked vault.
    pub vault: Vault,
    /// The recovery key, to be shown once to the user.
    pub recovery_key: RecoveryKey,
}

impl Vault {
    /// Create a vault: random vault id, DEK, salt, recovery key (OS CSPRNG).
    pub fn create(passphrase: &str, params: KdfParams) -> Result<CreatedVault> {
        Self::create_with(&mut OsRandom, passphrase, params)
    }

    pub(crate) fn create_with(
        rng: &mut impl RandomSource,
        passphrase: &str,
        params: KdfParams,
    ) -> Result<CreatedVault> {
        params.validate()?;
        let pw = check_new_passphrase(passphrase)?;
        let vault_id: [u8; 16] = rng.array()?;
        let dek = Dek(rng.array()?);
        let salt: [u8; 16] = rng.array()?;
        let recovery_key = RecoveryKey::generate_with(rng)?;
        let kek_pw = argon_kek(&pw, &salt, params)?;
        let kek_rk = derive_recovery_kek(&recovery_key.0, &vault_id);
        let header = Build {
            generation: 1,
            vault_id,
            params,
            salt,
            pw_nonce: rng.array()?,
            rk_nonce: rng.array()?,
            dek: &dek,
            kek_pw: &kek_pw,
            kek_rk: &kek_rk,
        }
        .serialize()?;
        let keys = Subkeys::derive(&dek, &vault_id);
        Ok(CreatedVault {
            vault: Vault { header, dek, keys },
            recovery_key,
        })
    }

    /// Current header.
    #[must_use]
    pub fn header(&self) -> &VaultHeader {
        &self.header
    }

    /// Subkeys for sealing/opening objects.
    #[must_use]
    pub fn keys(&self) -> &Subkeys {
        &self.keys
    }

    /// Change the passphrase: new salt/nonce/wrap, `header_gen + 1`, new MAC.
    /// DEK, recovery wrap contents and all objects are unchanged. Returns the
    /// new header; the caller persists it atomically.
    pub fn change_passphrase(
        &self,
        new_passphrase: &str,
        params: KdfParams,
    ) -> Result<VaultHeader> {
        self.change_passphrase_with(&mut OsRandom, new_passphrase, params)
    }

    pub(crate) fn change_passphrase_with(
        &self,
        rng: &mut impl RandomSource,
        new_passphrase: &str,
        params: KdfParams,
    ) -> Result<VaultHeader> {
        params.validate()?;
        let pw = check_new_passphrase(new_passphrase)?;
        let generation = self
            .header
            .generation()
            .checked_add(1)
            .ok_or(Error::MalformedHeader("generation overflow"))?;
        let salt: [u8; 16] = rng.array()?;
        let kek_pw = argon_kek(&pw, &salt, params)?;
        // The recovery wrap is carried over byte for byte (its AAD does not
        // depend on generation/salt), so it is re-MACed but not re-wrapped.
        let mut b = *self.header.as_bytes();
        b[OFF_GEN..OFF_GEN + 4].copy_from_slice(&generation.to_be_bytes());
        b[OFF_KDF + 1..OFF_KDF + 5].copy_from_slice(&params.m_kib.to_be_bytes());
        b[OFF_KDF + 5..OFF_KDF + 9].copy_from_slice(&params.t.to_be_bytes());
        b[OFF_KDF + 9..OFF_KDF + 13].copy_from_slice(&params.p.to_be_bytes());
        b[OFF_SALT..OFF_SALT + 16].copy_from_slice(&salt);
        let pw_nonce: [u8; 24] = rng.array()?;
        b[OFF_PW_NONCE..OFF_PW_NONCE + 24].copy_from_slice(&pw_nonce);
        let pw_wrap = wrap(&kek_pw, &pw_nonce, &self.dek, &pw_aad(&b))?;
        b[OFF_PW_WRAP..OFF_PW_WRAP + WRAP_LEN].copy_from_slice(&pw_wrap);
        self.remac(&mut b)?;
        VaultHeader::parse(&b)
    }

    /// Rotate the recovery key: new RK and `rk_wrap`, same DEK, `header_gen + 1`.
    /// Needs the current passphrase: the passphrase wrap's AAD covers the
    /// generation, so it must be re-wrapped too.
    pub fn rotate_recovery_key(&self, passphrase: &str) -> Result<(VaultHeader, RecoveryKey)> {
        self.rotate_recovery_key_with(&mut OsRandom, passphrase)
    }

    pub(crate) fn rotate_recovery_key_with(
        &self,
        rng: &mut impl RandomSource,
        passphrase: &str,
    ) -> Result<(VaultHeader, RecoveryKey)> {
        let h = &self.header;
        let pw = normalize(passphrase);
        let kek_pw = argon_kek(&pw, &arr::<16>(&h.bytes, OFF_SALT), h.kdf_params())
            .map_err(|_| Error::Unlock)?;
        // Prove the passphrase is the current one before rewriting anything.
        h.unwrap_dek(&kek_pw, OFF_PW_NONCE, OFF_PW_WRAP, &pw_aad(&h.bytes))?;
        let rk = RecoveryKey::generate_with(rng)?;
        let generation = h
            .generation()
            .checked_add(1)
            .ok_or(Error::MalformedHeader("generation overflow"))?;
        let kek_rk = derive_recovery_kek(&rk.0, &h.vault_id());
        let header = Build {
            generation,
            vault_id: h.vault_id(),
            params: h.kdf_params(),
            salt: arr(&h.bytes, OFF_SALT),
            pw_nonce: rng.array()?,
            rk_nonce: rng.array()?,
            dek: &self.dek,
            kek_pw: &kek_pw,
            kek_rk: &kek_rk,
        }
        .serialize()?;
        Ok((header, rk))
    }

    fn remac(&self, b: &mut [u8; HEADER_LEN]) -> Result<()> {
        let mut mac = new_mac(&self.keys.k_header)?;
        mac.update(&b[..OFF_MAC]);
        b[OFF_MAC..].copy_from_slice(&mac.finalize().into_bytes());
        Ok(())
    }
}
