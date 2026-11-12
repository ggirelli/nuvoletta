//! Phase 1 exit criterion: create a vault, write notes, close it, reopen it and verify the
//! contents, without a database.

mod common;

use common::{PASSPHRASE, create, note};
use nuvoletta_core::Note;
use nuvoletta_storage::Locked;

#[test]
fn vault_survives_close_and_reopen_with_all_notes_intact() {
    let dir = tempfile::tempdir().unwrap();
    let vault_dir = dir.path().join("MyVault");

    let (mut v, recovery_key) = create(&vault_dir);
    let mut notes: Vec<Note> = (0..25)
        .map(|i| {
            note(
                &format!("Note {i}"),
                &format!("folder{}/note{i}", i % 3),
                &format!("# Heading {i}\n\nBody with unicode: caf\u{e9} \u{1f600} {i}\n"),
            )
        })
        .collect();
    for n in &notes {
        v.create_note(n).unwrap();
    }
    notes[3].body = "edited".into();
    v.update_note(&notes[3]).unwrap();
    let removed = notes.remove(7);
    v.delete_note(&removed.id).unwrap();
    drop(v.lock()); // close

    // Reopen with the passphrase.
    let mut v = Locked::open(&vault_dir)
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    assert!(v.issues().is_empty());
    assert_eq!(v.list().len(), notes.len());
    for n in &notes {
        assert_eq!(&v.read_note(&n.id).unwrap(), n);
    }
    assert!(v.read_note(&removed.id).is_err());
    v.change_passphrase("a different passphrase", common::FAST)
        .unwrap();
    drop(v);

    // Reopen with the recovery key: same contents.
    let v = Locked::open(&vault_dir)
        .unwrap()
        .unlock_with_recovery_key(&recovery_key)
        .unwrap();
    for n in &notes {
        assert_eq!(&v.read_note(&n.id).unwrap(), n);
    }
}
