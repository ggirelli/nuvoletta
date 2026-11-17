//! Joining a device from a header, and removing an unused vault again.

mod common;

use common::{PASSPHRASE, create, note};
use nuvoletta_crypto::{Error as CryptoError, RecoveryKey};
use nuvoletta_storage::layout::{HEADER_FILE, META_DIR};
use nuvoletta_storage::{Cleanup, Error, Locked, Secret, remove_unused_vault};
use std::fs;

#[test]
fn joining_with_the_passphrase_writes_the_verified_header_and_unlocks() {
    let src = tempfile::tempdir().unwrap();
    let (mut origin, _rk) = create(src.path());
    let n = note("Title", "a", "body");
    origin.create_note(&n).unwrap();
    let header = origin.header().clone();

    let vault = Secret::Passphrase(PASSPHRASE).open(&header).unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = dst.path().join("new");
    let joined = Locked::join(&root, vault).unwrap();
    assert_eq!(joined.header().as_bytes(), header.as_bytes());
    assert!(joined.list().is_empty());
    let on_disk = fs::read(root.join(META_DIR).join(HEADER_FILE)).unwrap();
    assert_eq!(on_disk, header.as_bytes());
    drop(joined);
    // The directory is an ordinary vault now.
    let reopened = Locked::open(&root)
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    assert!(reopened.keys().same_keys(origin.keys()));
}

#[test]
fn joining_with_the_recovery_key_works_and_a_wrong_secret_is_refused() {
    let src = tempfile::tempdir().unwrap();
    let (origin, rk) = create(src.path());
    let header = origin.header().clone();

    let wrong = Secret::Passphrase("not the passphrase at all").open(&header);
    assert!(matches!(wrong, Err(Error::Crypto(CryptoError::Unlock))));

    let key = RecoveryKey::parse(&rk.to_display_string()).unwrap();
    let vault = Secret::RecoveryKey(&key).open(&header).unwrap();
    let dst = tempfile::tempdir().unwrap();
    let joined = Locked::join(dst.path().join("v"), vault).unwrap();
    assert_eq!(joined.header().as_bytes(), header.as_bytes());
}

#[test]
fn a_header_with_a_flipped_byte_never_opens() {
    let src = tempfile::tempdir().unwrap();
    let (origin, _) = create(src.path());
    for i in (0..236).step_by(7) {
        let mut b = *origin.header().as_bytes();
        b[i] ^= 1;
        let opened = nuvoletta_crypto::VaultHeader::parse(&b)
            .map_err(Error::from)
            .and_then(|h| Secret::Passphrase(PASSPHRASE).open(&h));
        assert!(opened.is_err(), "byte {i}");
    }
}

#[test]
fn joining_into_a_directory_that_has_a_vault_is_refused() {
    let src = tempfile::tempdir().unwrap();
    let (origin, _) = create(src.path());
    let vault = Secret::Passphrase(PASSPHRASE)
        .open(origin.header())
        .unwrap();
    drop(origin); // releases the single-instance lock
    assert!(matches!(
        Locked::join(src.path(), vault),
        Err(Error::VaultExists)
    ));
}

fn joined_vault(root: &std::path::Path) -> nuvoletta_storage::Unlocked {
    let src = tempfile::tempdir().unwrap();
    let (origin, _) = create(src.path());
    let vault = Secret::Passphrase(PASSPHRASE)
        .open(origin.header())
        .unwrap();
    Locked::join(root, vault).unwrap()
}

#[test]
fn cancelling_a_join_without_notes_removes_the_vault_directory() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("vault");
    drop(joined_vault(&root));
    assert_eq!(remove_unused_vault(&root).unwrap(), Cleanup::Removed);
    assert!(!root.exists());
}

#[test]
fn cancelling_a_join_keeps_a_directory_with_other_files_but_removes_the_vault() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("vault");
    drop(joined_vault(&root));
    fs::write(root.join("mine.txt"), "mine").unwrap();
    assert_eq!(remove_unused_vault(&root).unwrap(), Cleanup::Removed);
    assert!(root.join("mine.txt").exists());
    assert!(!root.join(META_DIR).exists());
}

#[test]
fn cancelling_a_join_that_holds_a_note_removes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("vault");
    let mut v = joined_vault(&root);
    let n = note("t", "p", "precious");
    v.create_note(&n).unwrap();
    drop(v);
    assert_eq!(remove_unused_vault(&root).unwrap(), Cleanup::Kept);
    assert!(root.join(META_DIR).join(HEADER_FILE).exists());
    let again = Locked::open(&root)
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    assert_eq!(again.list().len(), 1);
}

#[test]
fn cancelling_a_join_whose_only_note_is_in_the_trash_removes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("vault");
    let mut v = joined_vault(&root);
    let n = note("t", "p", "precious");
    v.create_note(&n).unwrap();
    v.delete_note(&n.id).unwrap();
    drop(v);
    assert_eq!(remove_unused_vault(&root).unwrap(), Cleanup::Kept);
    assert!(root.exists());
}

#[test]
fn only_a_vault_is_ever_removed() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("keep.txt"), "x").unwrap();
    assert_eq!(remove_unused_vault(d.path()).unwrap(), Cleanup::Kept);
    assert!(d.path().join("keep.txt").exists());
}
