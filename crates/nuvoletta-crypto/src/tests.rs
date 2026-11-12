//! Unit, property and vector tests.

use crate::envelope::{ENVELOPE_HEADER_LEN, MIN_ENVELOPE_LEN, seal_with};
use crate::header::HEADER_LEN;
use crate::padding::{padded_len, padme};
use crate::rng::{DeterministicRandom, RandomSource};
use crate::*;
use proptest::prelude::*;
use std::collections::HashSet;
use std::sync::OnceLock;

const PW: &str = "correct horse battery staple";
const FAST: KdfParams = KdfParams::MIN_ACCEPTED;

fn created() -> CreatedVault {
    Vault::create(PW, FAST).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn meta(t: ObjType) -> ObjectMeta {
    let scope = if t == ObjType::LocalState {
        [0; 16]
    } else {
        [7; 16]
    };
    ObjectMeta::new(t, scope).unwrap()
}

// ---------- header ----------

#[test]
fn defaults_match_spec() {
    assert_eq!(
        KdfParams::default(),
        KdfParams {
            m_kib: 65536,
            t: 3,
            p: 1
        }
    );
}

#[test]
fn create_unlock_roundtrip() {
    let c = created();
    let bytes = c.vault.header().as_bytes().to_vec();
    assert_eq!(bytes.len(), HEADER_LEN);
    let h = VaultHeader::parse(&bytes).unwrap();
    assert_eq!(&h, c.vault.header());
    assert_eq!(h.generation(), 1);
    let v = h.unlock_with_passphrase(PW).unwrap();
    assert_eq!(v.keys().vault_id(), c.vault.keys().vault_id());
    let v2 = h.unlock_with_recovery_key(&c.recovery_key).unwrap();
    let m = meta(ObjType::NoteRevision);
    let sealed = seal(v.keys(), &m, b"hello").unwrap();
    assert_eq!(&*open(v2.keys(), &sealed).unwrap().plaintext, b"hello");
}

#[test]
fn wrong_passphrase_is_generic_unlock_error() {
    let c = created();
    let err = c
        .vault
        .header()
        .unlock_with_passphrase("wrong passphrase!!")
        .unwrap_err();
    assert_eq!(err, Error::Unlock);
}

#[test]
fn short_passphrase_rejected_on_create_and_change() {
    assert_eq!(
        Vault::create("short", FAST).unwrap_err(),
        Error::PassphraseTooShort { min: 12 }
    );
    let c = created();
    assert!(matches!(
        c.vault.change_passphrase("short", FAST),
        Err(Error::PassphraseTooShort { .. })
    ));
}

#[test]
fn passphrase_is_nfkc_normalised_not_trimmed() {
    // Fullwidth letters normalise to ASCII under NFKC.
    let c = Vault::create("ｐａｓｓｐｈｒａｓｅ１２３４", FAST).unwrap();
    let h = c.vault.header();
    assert!(h.unlock_with_passphrase("passphrase1234").is_ok());
    assert!(h.unlock_with_passphrase(" passphrase1234").is_err());
}

#[test]
fn out_of_range_params_rejected_on_create() {
    for p in [
        KdfParams {
            m_kib: 8191,
            t: 1,
            p: 1,
        },
        KdfParams {
            m_kib: 8192,
            t: 0,
            p: 1,
        },
        KdfParams {
            m_kib: 8192,
            t: 17,
            p: 1,
        },
        KdfParams {
            m_kib: 8192,
            t: 1,
            p: 0,
        },
        KdfParams {
            m_kib: 8192,
            t: 1,
            p: 9,
        },
        KdfParams {
            m_kib: 1_048_577,
            t: 1,
            p: 1,
        },
    ] {
        assert_eq!(
            Vault::create(PW, p).unwrap_err(),
            Error::KdfParamsOutOfRange
        );
    }
}

#[test]
fn header_parser_rejects_oversized_params_before_argon() {
    let c = created();
    let mut b = c.vault.header().as_bytes().to_vec();
    b[31..35].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        VaultHeader::parse(&b).unwrap_err(),
        Error::KdfParamsOutOfRange
    );
}

#[test]
fn header_parser_rejects_unknown_versions_and_algs() {
    let c = created();
    let good = c.vault.header().as_bytes().to_vec();
    let mut b = good.clone();
    b[9] = 2;
    assert_eq!(
        VaultHeader::parse(&b).unwrap_err(),
        Error::UnsupportedVersion
    );
    let mut b = good.clone();
    b[8] = 1;
    assert_eq!(
        VaultHeader::parse(&b).unwrap_err(),
        Error::UnsupportedVersion
    );
    let mut b = good.clone();
    b[30] = 2;
    assert_eq!(
        VaultHeader::parse(&b).unwrap_err(),
        Error::UnsupportedAlgorithm
    );
    let mut b = good.clone();
    b[59] = 2;
    assert_eq!(
        VaultHeader::parse(&b).unwrap_err(),
        Error::UnsupportedAlgorithm
    );
    let mut b = good;
    b[0] ^= 1;
    assert!(matches!(
        VaultHeader::parse(&b),
        Err(Error::MalformedHeader(_))
    ));
}

#[test]
fn tamper_every_header_byte_fails_recovery_unlock() {
    let c = created();
    let good = c.vault.header().as_bytes().to_vec();
    for i in 0..HEADER_LEN {
        for mask in [0x01u8, 0x80] {
            let mut b = good.clone();
            b[i] ^= mask;
            let r =
                VaultHeader::parse(&b).and_then(|h| h.unlock_with_recovery_key(&c.recovery_key));
            assert!(r.is_err(), "byte {i} mask {mask:#x} accepted (recovery)");
        }
    }
}

#[test]
fn tamper_every_header_byte_fails_passphrase_unlock() {
    let c = created();
    let good = c.vault.header().as_bytes().to_vec();
    for i in 0..HEADER_LEN {
        let mut b = good.clone();
        b[i] ^= 0x01;
        let r = VaultHeader::parse(&b).and_then(|h| h.unlock_with_passphrase(PW));
        assert!(r.is_err(), "byte {i} accepted (passphrase)");
    }
}

#[test]
fn truncated_or_extended_header_rejected_at_every_length() {
    let c = created();
    let good = c.vault.header().as_bytes().to_vec();
    for n in 0..HEADER_LEN {
        assert!(VaultHeader::parse(&good[..n]).is_err(), "len {n}");
    }
    let mut longer = good;
    longer.push(0);
    assert!(VaultHeader::parse(&longer).is_err());
}

#[test]
fn spliced_wraps_from_another_header_fail() {
    let a = created();
    let b = created();
    let mut spliced = a.vault.header().as_bytes().to_vec();
    spliced[84..204].copy_from_slice(&b.vault.header().as_bytes()[84..204]);
    let r = VaultHeader::parse(&spliced).and_then(|h| h.unlock_with_recovery_key(&a.recovery_key));
    assert!(r.is_err());
}

#[test]
fn wrong_recovery_key_fails_generic() {
    let c = created();
    let other = RecoveryKey::generate().unwrap();
    assert_eq!(
        c.vault
            .header()
            .unlock_with_recovery_key(&other)
            .unwrap_err(),
        Error::Unlock
    );
}

#[test]
fn passphrase_change_rewraps_only() {
    let c = created();
    let m = meta(ObjType::NoteRevision);
    let sealed = seal(c.vault.keys(), &m, b"note").unwrap();
    let new_header = c
        .vault
        .change_passphrase("another long passphrase", FAST)
        .unwrap();
    let old = c.vault.header().as_bytes();
    let new = new_header.as_bytes();
    assert_eq!(new_header.generation(), 2);
    assert_eq!(&old[14..30], &new[14..30], "vault_id immutable");
    assert_ne!(&old[43..59], &new[43..59], "salt regenerated");
    assert_ne!(&old[60..132], &new[60..132], "pw wrap replaced");
    assert_eq!(&old[132..204], &new[132..204], "recovery wrap untouched");
    assert!(
        new_header.unlock_with_passphrase(PW).is_err(),
        "old passphrase"
    );
    let v = new_header
        .unlock_with_passphrase("another long passphrase")
        .unwrap();
    assert_eq!(
        &*open(v.keys(), &sealed).unwrap().plaintext,
        b"note",
        "DEK unchanged"
    );
    let v = new_header
        .unlock_with_recovery_key(&c.recovery_key)
        .unwrap();
    assert_eq!(&*open(v.keys(), &sealed).unwrap().plaintext, b"note");
}

#[test]
fn passphrase_change_can_come_from_recovery_unlock() {
    let c = created();
    let v = c
        .vault
        .header()
        .unlock_with_recovery_key(&c.recovery_key)
        .unwrap();
    let h = v.change_passphrase("brand new passphrase", FAST).unwrap();
    assert!(h.unlock_with_passphrase("brand new passphrase").is_ok());
}

#[test]
fn recovery_key_rotation() {
    let c = created();
    let (h, rk2) = c.vault.rotate_recovery_key(PW).unwrap();
    assert_eq!(h.generation(), 2);
    assert!(h.unlock_with_recovery_key(&c.recovery_key).is_err());
    assert!(h.unlock_with_recovery_key(&rk2).is_ok());
    assert!(h.unlock_with_passphrase(PW).is_ok());
    assert_eq!(
        c.vault.rotate_recovery_key("nope nope nope!").unwrap_err(),
        Error::Unlock
    );
}

#[test]
fn debug_never_prints_secrets() {
    let c = created();
    let s = format!("{:?} {:?} {:?}", c.vault, c.recovery_key, c.vault.keys());
    assert!(s.contains("redacted"));
    let rk = c.recovery_key.to_display_string();
    assert!(!s.contains(rk.replace('-', "").get(..8).unwrap()));
}

// ---------- recovery key ----------

#[test]
fn recovery_roundtrip_and_format() {
    let rk = RecoveryKey::generate().unwrap();
    let s = rk.to_display_string();
    assert_eq!(s.len(), 56 + 13);
    let groups: Vec<&str> = s.split('-').collect();
    assert_eq!(groups.len(), 14);
    assert!(groups.iter().all(|g| g.len() == 4));
    assert!(
        s.chars()
            .all(|c| c == '-' || "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c))
    );
    let back = RecoveryKey::parse(&s).unwrap();
    assert_eq!(back.0, rk.0);
}

#[test]
fn recovery_parse_is_tolerant() {
    let rk = RecoveryKey::generate().unwrap();
    let s = rk.to_display_string();
    let lower = s.to_lowercase();
    assert_eq!(RecoveryKey::parse(&lower).unwrap().0, rk.0);
    let spaced = s.replace('-', " \n\t");
    assert_eq!(RecoveryKey::parse(&spaced).unwrap().0, rk.0);
    let nohyphen = s.replace('-', "");
    assert_eq!(RecoveryKey::parse(&nohyphen).unwrap().0, rk.0);
    // Ambiguous characters: map 0->O, 1->I/L (they decode to the same value).
    let alt = s.replace('0', "o").replace('1', "l");
    assert_eq!(RecoveryKey::parse(&alt).unwrap().0, rk.0);
    let alt = s.replace('1', "I");
    assert_eq!(RecoveryKey::parse(&alt).unwrap().0, rk.0);
}

#[test]
fn recovery_typo_reports_checksum_and_garbage_reports_format() {
    let rk = RecoveryKey::generate().unwrap();
    let s = rk.to_display_string();
    for i in (0..s.len()).filter(|&i| s.as_bytes()[i] != b'-') {
        let mut b = s.as_bytes().to_vec();
        b[i] = if b[i] == b'2' { b'3' } else { b'2' };
        let t = String::from_utf8(b).unwrap();
        assert_eq!(
            RecoveryKey::parse(&t).unwrap_err(),
            Error::RecoveryKeyChecksum,
            "pos {i}"
        );
    }
    assert_eq!(
        RecoveryKey::parse("").unwrap_err(),
        Error::RecoveryKeyFormat
    );
    assert_eq!(
        RecoveryKey::parse(&s[1..]).unwrap_err(),
        Error::RecoveryKeyFormat
    );
    assert_eq!(
        RecoveryKey::parse(&format!("{}A", &**s)).unwrap_err(),
        Error::RecoveryKeyFormat
    );
    assert_eq!(
        RecoveryKey::parse(&s.replacen(&s[..1], "U", 1)).unwrap_err(),
        Error::RecoveryKeyFormat
    );
    assert_eq!(
        RecoveryKey::parse("ünïcode").unwrap_err(),
        Error::RecoveryKeyFormat
    );
}

#[test]
fn recovery_unlock_through_text() {
    let c = created();
    let text = c.recovery_key.to_display_string().to_lowercase();
    let rk = RecoveryKey::parse(&text).unwrap();
    assert!(c.vault.header().unlock_with_recovery_key(&rk).is_ok());
}

// ---------- keys ----------

#[test]
fn subkeys_are_independent_and_deterministic() {
    let dek = Dek([9; 32]);
    let a = Subkeys::derive(&dek, &[1; 16]);
    let b = Subkeys::derive(&dek, &[1; 16]);
    let c = Subkeys::derive(&dek, &[2; 16]);
    assert_eq!(a.k_note, b.k_note);
    assert_ne!(a.k_note, c.k_note);
    let all = [a.k_header, a.k_note, a.k_log, a.k_local];
    for i in 0..4 {
        for j in i + 1..4 {
            assert_ne!(all[i], all[j]);
        }
    }
}

// ---------- padding ----------

#[test]
fn padme_known_values_and_properties() {
    assert_eq!(padme(300), 304);
    assert_eq!(padme(256), 256);
    assert_eq!(padme(9), 10);
    for l in (1u64..=(1 << 20)).step_by(7) {
        let p = padme(l);
        assert!(p >= l);
        assert_eq!(padme(p), p, "idempotent at {l}");
        if l >= 256 {
            assert!((p - l) as f64 <= 0.12 * l as f64 + 1.0, "overhead at {l}");
        }
    }
}

#[test]
fn padded_len_floor_and_hiding() {
    for n in 0..=252 {
        assert_eq!(padded_len(n).unwrap(), 256, "payload {n}");
    }
    assert_eq!(padded_len(253).unwrap(), padme(257));
    // Distinct sizes collapse to the same padded length.
    assert_eq!(padded_len(1000).unwrap(), padded_len(1003).unwrap());
    assert_ne!(padded_len(1000).unwrap(), padded_len(2000).unwrap());
}

#[test]
fn envelope_lengths_match_padding() {
    let c = created();
    for n in [0usize, 1, 100, 252, 253, 1000, 5000, 70_000] {
        let e = seal(c.vault.keys(), &meta(ObjType::NoteRevision), &vec![1u8; n]).unwrap();
        assert_eq!(e.len(), 80 + padded_len(n).unwrap() as usize + 16, "n={n}");
    }
    let tiny = seal(c.vault.keys(), &meta(ObjType::NoteRevision), b"").unwrap();
    assert_eq!(tiny.len(), MIN_ENVELOPE_LEN);
}

// ---------- envelope ----------

fn sample_envelope(t: ObjType) -> (CreatedVault, Vec<u8>) {
    let c = created();
    let e = seal(c.vault.keys(), &meta(t), b"secret payload").unwrap();
    (c, e)
}

#[test]
fn envelope_roundtrip_all_types() {
    for t in [
        ObjType::NoteRevision,
        ObjType::LogSegment,
        ObjType::Snapshot,
        ObjType::LocalState,
    ] {
        let (c, e) = sample_envelope(t);
        let o = open(c.vault.keys(), &e).unwrap();
        assert_eq!(&*o.plaintext, b"secret payload");
        assert_eq!(o.meta.obj_type, t);
    }
}

#[test]
fn tamper_every_envelope_byte_fails() {
    let (c, e) = sample_envelope(ObjType::NoteRevision);
    for i in 0..e.len() {
        let mut b = e.clone();
        b[i] ^= 0x01;
        assert!(open(c.vault.keys(), &b).is_err(), "byte {i}");
    }
}

#[test]
fn truncate_every_envelope_length_fails() {
    let (c, e) = sample_envelope(ObjType::NoteRevision);
    for n in 0..e.len() {
        assert!(open(c.vault.keys(), &e[..n]).is_err(), "len {n}");
    }
    let mut longer = e;
    longer.push(0);
    assert!(open(c.vault.keys(), &longer).is_err());
}

#[test]
fn aad_binds_ids_and_type() {
    let (c, e) = sample_envelope(ObjType::NoteRevision);
    for (range, name) in [
        (24..40, "object_id"),
        (40..56, "scope_id"),
        (56..80, "nonce"),
    ] {
        let mut b = e.clone();
        b[range.start] ^= 0xff;
        assert_eq!(
            open(c.vault.keys(), &b).unwrap_err(),
            Error::Authentication,
            "{name}"
        );
    }
    // Swapping the type byte changes the key as well as the AAD.
    for t in [2u8, 3, 4] {
        let mut b = e.clone();
        b[6] = t;
        assert!(open(c.vault.keys(), &b).is_err());
    }
    // Swapping ids between two valid objects fails.
    let e2 = seal(c.vault.keys(), &meta(ObjType::NoteRevision), b"other").unwrap();
    let mut swapped = e.clone();
    swapped[24..56].copy_from_slice(&e2[24..56]);
    assert!(open(c.vault.keys(), &swapped).is_err());
}

#[test]
fn context_check_detects_moved_objects() {
    let (c, e) = sample_envelope(ObjType::NoteRevision);
    let o = open(c.vault.keys(), &e).unwrap();
    assert!(o.check_context(&o.meta.object_id, &o.meta.scope_id).is_ok());
    assert_eq!(
        o.check_context(&[0; 16], &o.meta.scope_id).unwrap_err(),
        Error::ContextMismatch
    );
    assert_eq!(
        o.check_context(&o.meta.object_id, &[0; 16]).unwrap_err(),
        Error::ContextMismatch
    );
}

#[test]
fn other_vault_rejected() {
    let (_, e) = sample_envelope(ObjType::NoteRevision);
    let other = created();
    assert_eq!(
        open(other.vault.keys(), &e).unwrap_err(),
        Error::VaultMismatch
    );
}

#[test]
fn envelope_parser_rejects_unknown_fields() {
    let (c, e) = sample_envelope(ObjType::NoteRevision);
    let k = c.vault.keys();
    let cases: [(usize, u8, Error); 5] = [
        (4, 2, Error::UnsupportedVersion),
        (5, 2, Error::UnsupportedAlgorithm),
        (6, 9, Error::MalformedEnvelope("unknown object type")),
        (6, 0, Error::MalformedEnvelope("unknown object type")),
        (7, 1, Error::MalformedEnvelope("non-zero flags")),
    ];
    for (i, v, err) in cases {
        let mut b = e.clone();
        b[i] = v;
        assert_eq!(open(k, &b).unwrap_err(), err);
    }
    let mut b = e;
    b[0] ^= 1;
    assert!(matches!(
        open(k, &b),
        Err(Error::MalformedEnvelope("bad magic"))
    ));
}

#[test]
fn local_state_requires_zero_scope() {
    let c = created();
    let m = ObjectMeta::new(ObjType::LocalState, [1; 16]).unwrap();
    assert!(seal(c.vault.keys(), &m, b"x").is_err());
}

#[test]
fn nonces_unique_across_many_seals() {
    let c = created();
    let m = meta(ObjType::NoteRevision);
    let mut seen = HashSet::new();
    for _ in 0..20_000 {
        let e = seal(c.vault.keys(), &m, b"same content").unwrap();
        assert!(seen.insert(e[56..80].to_vec()), "nonce reuse");
    }
}

#[test]
fn same_plaintext_encrypts_differently() {
    let c = created();
    let m = meta(ObjType::NoteRevision);
    let a = seal(c.vault.keys(), &m, b"x").unwrap();
    let b = seal(c.vault.keys(), &m, b"x").unwrap();
    assert_ne!(a[80..], b[80..]);
}

/// Encrypt arbitrary padded plaintext (bypassing `pad`) to test the reader.
fn seal_raw(keys: &Subkeys, padded: &[u8]) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    let mut out =
        seal(keys, &meta(ObjType::NoteRevision), b"").unwrap()[..ENVELOPE_HEADER_LEN].to_vec();
    let ct = XChaCha20Poly1305::new((&keys.k_note).into())
        .encrypt(
            XNonce::from_slice(&out[56..80]),
            Payload {
                msg: padded,
                aad: &out,
            },
        )
        .unwrap();
    out.extend_from_slice(&ct);
    out
}

#[test]
fn invalid_padding_rejected_after_authentication() {
    let c = created();
    let k = c.vault.keys();
    let mut good = vec![0u8; 256];
    good[..4].copy_from_slice(&3u32.to_le_bytes());
    good[4..7].copy_from_slice(b"abc");
    assert_eq!(&*open(k, &seal_raw(k, &good)).unwrap().plaintext, b"abc");
    let mut nonzero = good.clone();
    nonzero[200] = 1;
    assert_eq!(
        open(k, &seal_raw(k, &nonzero)).unwrap_err(),
        Error::InvalidPadding
    );
    let mut toolong = good.clone();
    toolong[..4].copy_from_slice(&1000u32.to_le_bytes());
    assert_eq!(
        open(k, &seal_raw(k, &toolong)).unwrap_err(),
        Error::InvalidPadding
    );
    let mut wrong_total = good.clone();
    wrong_total.resize(300, 0);
    wrong_total[..4].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(
        open(k, &seal_raw(k, &wrong_total)).unwrap_err(),
        Error::InvalidPadding
    );
    let mut huge = good;
    huge[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        open(k, &seal_raw(k, &huge)).unwrap_err(),
        Error::InvalidPadding
    );
}

// ---------- payload ----------

fn note() -> NotePayload {
    NotePayload {
        schema_version: 1,
        note_id: [7; 16],
        title: "Title".into(),
        path: "a/b/c".into(),
        tags: vec!["x".into(), "y".into()],
        created_at: 1_700_000_000_000,
        modified_at: 1_700_000_001_000,
        body: "# Hello\n\nbody".into(),
    }
}

#[test]
fn note_payload_roundtrip_and_envelope() {
    let n = note();
    let bytes = n.encode().unwrap();
    assert_eq!(NotePayload::decode(&bytes).unwrap(), n);
    let c = created();
    let e = seal(c.vault.keys(), &meta(ObjType::NoteRevision), &bytes).unwrap();
    let o = open(c.vault.keys(), &e).unwrap();
    let back = NotePayload::decode(&o.plaintext).unwrap();
    back.check_scope(&o.meta.scope_id).unwrap();
    assert_eq!(
        back.check_scope(&[0; 16]).unwrap_err(),
        Error::ContextMismatch
    );
}

#[test]
fn note_payload_uses_integer_keys_in_order() {
    let bytes = note().encode().unwrap();
    assert_eq!(bytes[0], 0xa8, "definite map of 8");
    assert_eq!(&bytes[1..3], &[0x00, 0x01], "key 0 -> schema 1");
}

#[test]
fn note_payload_ignores_unknown_keys_and_rejects_newer_schema() {
    let mut buf = Vec::new();
    let mut e = minicbor::Encoder::new(&mut buf);
    e.map(9).unwrap();
    e.u8(0).unwrap().u8(1).unwrap();
    e.u8(1).unwrap().bytes(&[7; 16]).unwrap();
    e.u8(2).unwrap().str("t").unwrap();
    e.u8(3).unwrap().str("p").unwrap();
    e.u8(4).unwrap().array(0).unwrap();
    e.u8(5).unwrap().i64(1).unwrap();
    e.u8(6).unwrap().i64(2).unwrap();
    e.u8(7).unwrap().str("b").unwrap();
    e.u8(99).unwrap().str("future").unwrap();
    assert_eq!(NotePayload::decode(&buf).unwrap().body, "b");
    let mut newer = note();
    newer.schema_version = 2;
    assert!(newer.encode().is_err());
}

#[test]
fn note_payload_validation() {
    for bad in ["", "/abs", "a//b", "a/../b", "..", "a/", "e\u{0301}"] {
        let mut n = note();
        n.path = bad.into();
        assert!(n.encode().is_err(), "path {bad:?}");
    }
    let mut n = note();
    n.title = "x".repeat(1025);
    assert!(n.encode().is_err());
    let mut n = note();
    n.tags = vec!["t".into(); 257];
    assert!(n.encode().is_err());
    assert!(NotePayload::decode(&[0xff, 0x00]).is_err());
    assert!(format!("{:?}", note()).contains("redacted"));
}

// ---------- properties ----------

fn shared_keys() -> &'static Subkeys {
    static K: OnceLock<Subkeys> = OnceLock::new();
    K.get_or_init(|| Subkeys::derive(&Dek([5; 32]), &[6; 16]))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_envelope_roundtrip(payload in proptest::collection::vec(any::<u8>(), 0..3000), t in 1u8..=4) {
        let ot = [ObjType::NoteRevision, ObjType::LogSegment, ObjType::Snapshot, ObjType::LocalState][usize::from(t - 1)];
        let m = meta(ot);
        let e = seal(shared_keys(), &m, &payload).unwrap();
        prop_assert_eq!(e.len(), 80 + padded_len(payload.len()).unwrap() as usize + 16);
        let o = open(shared_keys(), &e).unwrap();
        prop_assert_eq!(&*o.plaintext, &payload);
        prop_assert_eq!(o.meta, m);
    }

    #[test]
    fn prop_single_bit_flip_fails(payload in proptest::collection::vec(any::<u8>(), 0..400), pos in any::<usize>(), bit in 0u8..8) {
        let e = seal(shared_keys(), &meta(ObjType::NoteRevision), &payload).unwrap();
        let mut b = e.clone();
        let i = pos % b.len();
        b[i] ^= 1 << bit;
        prop_assert!(open(shared_keys(), &b).is_err());
    }

    #[test]
    fn prop_parsers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600), s in ".{0,100}") {
        let _ = VaultHeader::parse(&bytes);
        let _ = open(shared_keys(), &bytes);
        let _ = NotePayload::decode(&bytes);
        let _ = RecoveryKey::parse(&s);
    }

    #[test]
    fn prop_header_parse_arbitrary_236_never_panics(bytes in proptest::collection::vec(any::<u8>(), HEADER_LEN)) {
        let _ = VaultHeader::parse(&bytes);
    }

    #[test]
    fn prop_recovery_roundtrip(k in any::<[u8; 32]>()) {
        let rk = RecoveryKey::from_bytes(k);
        prop_assert_eq!(RecoveryKey::parse(&rk.to_display_string()).unwrap().0, k);
    }

    #[test]
    fn prop_padme(l in 1u64..(1 << 40)) {
        let p = padme(l);
        prop_assert!(p >= l);
        prop_assert!(p - l <= l / 8 + 1);
    }
}

// ---------- slow ----------

#[test]
#[ignore = "full-cost Argon2id (64 MiB, t=3); run nightly via `just test-slow`"]
fn full_cost_argon2_roundtrip() {
    let c = Vault::create(PW, KdfParams::DEFAULT).unwrap();
    assert_eq!(c.vault.header().kdf_params(), KdfParams::DEFAULT);
    let h = VaultHeader::parse(c.vault.header().as_bytes()).unwrap();
    assert!(h.unlock_with_passphrase(PW).is_ok());
    assert!(h.unlock_with_passphrase("not the passphrase").is_err());
    let h2 = c
        .vault
        .change_passphrase("a different passphrase", KdfParams::DEFAULT)
        .unwrap();
    assert!(h2.unlock_with_passphrase("a different passphrase").is_ok());
}

// ---------- vectors ----------

fn vectors_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

/// Compare generated JSON to the committed file; `NUVOLETTA_BLESS=1` rewrites it.
fn check_vector(name: &str, value: &serde_json::Value) {
    let path = vectors_dir().join(name);
    let text = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var_os("NUVOLETTA_BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let on_disk =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing vector {name}"));
    assert_eq!(
        on_disk, text,
        "vector {name} differs (re-bless only for an intended format change)"
    );
}

fn det_vault() -> (CreatedVault, DeterministicRandom) {
    let mut rng = DeterministicRandom::new("nuvoletta-test-vectors-v1");
    let c = Vault::create_with(&mut rng, PW, FAST).unwrap();
    (c, rng)
}

#[test]
fn vector_header_and_subkeys() {
    let (c, _) = det_vault();
    let h = c.vault.header();
    // Verify the committed bytes unlock, not only that regeneration matches.
    let v = serde_json::json!({
        "description": "Header for passphrase (utf-8) with Argon2id m=8192,t=1,p=1; all randomness from a deterministic test RNG",
        "passphrase": PW,
        "kdf": {"m_kib": 8192, "t": 1, "p": 1},
        "recovery_key": &**c.recovery_key.to_display_string(),
        "recovery_key_raw_hex": hex(&c.recovery_key.0),
        "dek_hex": hex(&c.vault.dek.0),
        "vault_id_hex": hex(&h.vault_id()),
        "header_hex": hex(h.as_bytes()),
        "subkeys": {
            "header-auth": hex(&c.vault.keys().k_header),
            "note-content": hex(&c.vault.keys().k_note),
            "device-log": hex(&c.vault.keys().k_log),
            "local-cache": hex(&c.vault.keys().k_local),
        },
    });
    check_vector("header.json", &v);
    let bytes: Vec<u8> = (0..HEADER_LEN)
        .map(|i| {
            let s = v["header_hex"].as_str().unwrap();
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()
        })
        .collect();
    let parsed = VaultHeader::parse(&bytes).unwrap();
    assert!(parsed.unlock_with_passphrase(PW).is_ok());
    let rk = RecoveryKey::parse(v["recovery_key"].as_str().unwrap()).unwrap();
    assert!(parsed.unlock_with_recovery_key(&rk).is_ok());
}

#[test]
fn vector_envelopes() {
    let (c, mut rng) = det_vault();
    let mut cases = Vec::new();
    for (name, t, payload) in [
        (
            "note_revision",
            ObjType::NoteRevision,
            note().encode().unwrap().to_vec(),
        ),
        (
            "log_segment",
            ObjType::LogSegment,
            b"log segment payload".to_vec(),
        ),
        ("snapshot", ObjType::Snapshot, b"snapshot payload".to_vec()),
        ("local_state", ObjType::LocalState, vec![0xAB; 300]),
    ] {
        let scope: [u8; 16] = if t == ObjType::LocalState {
            [0; 16]
        } else {
            rng.array().unwrap()
        };
        let m = ObjectMeta::new_with(&mut rng, t, scope).unwrap();
        let e = seal_with(&mut rng, c.vault.keys(), &m, &payload).unwrap();
        let o = open(c.vault.keys(), &e).unwrap();
        assert_eq!(&*o.plaintext, &payload);
        cases.push(serde_json::json!({
            "name": name,
            "obj_type": t as u8,
            "object_id_hex": hex(&m.object_id),
            "scope_id_hex": hex(&m.scope_id),
            "payload_hex": hex(&payload),
            "envelope_hex": hex(&e),
        }));
    }
    let v = serde_json::json!({
        "description": "Envelopes sealed under the keys of header.json (deterministic nonces)",
        "vault_id_hex": hex(c.vault.keys().vault_id()),
        "cases": cases,
    });
    check_vector("envelope.json", &v);
}

#[test]
fn vector_recovery_and_padme() {
    let rk = RecoveryKey::from_bytes(core::array::from_fn(|i| i as u8));
    let s = rk.to_display_string();
    let mut typo = s.as_bytes().to_vec();
    typo[0] = if typo[0] == b'2' { b'3' } else { b'2' };
    let padme_table: Vec<_> = [
        1u64,
        255,
        256,
        257,
        300,
        1000,
        4096,
        65_535,
        65_536,
        1 << 20,
    ]
    .iter()
    .map(|&l| serde_json::json!([l, padme(l)]))
    .collect();
    let v = serde_json::json!({
        "recovery_key_raw_hex": hex(&rk.0),
        "recovery_key": &**s,
        "lowercase_no_hyphens": s.to_lowercase().replace('-', ""),
        "typo": String::from_utf8(typo).unwrap(),
        "typo_error": "checksum",
        "padme_input_output": padme_table,
        "padded_len_for_payload": [[0, 256], [252, 256], [253, padded_len(253).unwrap()], [1000, padded_len(1000).unwrap()]],
    });
    check_vector("recovery_padme.json", &v);
    assert_eq!(
        RecoveryKey::parse(v["lowercase_no_hyphens"].as_str().unwrap())
            .unwrap()
            .0,
        rk.0
    );
    assert_eq!(
        RecoveryKey::parse(v["typo"].as_str().unwrap()).unwrap_err(),
        Error::RecoveryKeyChecksum
    );
}
