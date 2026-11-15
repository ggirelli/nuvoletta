//! The small API `nuvoletta-sync` builds on: verbatim revision read, authenticated install and
//! create-only atomic writes.

mod common;

use common::*;
use nuvoletta_storage::fsutil::write_new_atomic;

#[test]
fn a_revision_can_be_read_verbatim_and_installed_in_another_vault_of_the_same_key() {
    let t1 = tempfile::tempdir().unwrap();
    let (mut a, _rk) = create(&t1.path().join("a"));
    let n = note("T", "p", "body");
    a.create_note(&n).unwrap();
    let bytes = a.read_revision_bytes(&n.id).unwrap();
    // A second vault instance of the same vault (same header, same keys).
    let t2 = tempfile::tempdir().unwrap();
    let dir = t2.path().join("b");
    std::fs::create_dir_all(dir.join(".nuvoletta")).unwrap();
    std::fs::copy(
        t1.path().join("a/.nuvoletta/vault-header"),
        dir.join(".nuvoletta/vault-header"),
    )
    .unwrap();
    let mut b = nuvoletta_storage::Locked::open(&dir)
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    let summary = b.install_revision(&bytes).unwrap();
    assert_eq!(summary.revision_id, a.summary(&n.id).unwrap().revision_id);
    assert_eq!(b.read_note(&n.id).unwrap().body, "body");
    assert_eq!(
        b.read_revision_bytes(&n.id).unwrap(),
        bytes,
        "stored byte for byte"
    );
    // Installing again replaces atomically; garbage and tampered bytes are refused.
    b.install_revision(&bytes).unwrap();
    let mut bad = bytes.clone();
    bad[100] ^= 1;
    assert!(b.install_revision(&bad).is_err());
    assert!(b.install_revision(b"junk").is_err());
    assert_eq!(b.read_note(&n.id).unwrap().body, "body");
}

#[test]
fn a_revision_of_another_vault_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let (mut a, _) = create(&t.path().join("a"));
    let (mut b, _) = create(&t.path().join("b"));
    let n = note("T", "p", "x");
    a.create_note(&n).unwrap();
    let bytes = a.read_revision_bytes(&n.id).unwrap();
    assert!(b.install_revision(&bytes).is_err());
    assert!(b.list().is_empty());
}

#[test]
fn write_new_atomic_never_replaces_and_leaves_no_temporary_file() {
    let t = tempfile::tempdir().unwrap();
    assert!(write_new_atomic(t.path(), "x", b"one").unwrap());
    assert!(!write_new_atomic(t.path(), "x", b"two").unwrap());
    assert_eq!(std::fs::read(t.path().join("x")).unwrap(), b"one");
    assert_eq!(all_files(t.path()).len(), 1);
}
