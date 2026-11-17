//! Behavioural tests of the filesystem vault.

mod common;

use common::{FAST, PASSPHRASE, all_files, create, note};
use nuvoletta_core::NoteId;
use nuvoletta_crypto::{Error as CryptoError, RecoveryKey};
use nuvoletta_storage::layout::{HEADER_FILE, META_DIR, NOTES_DIR, note_file_name};
use nuvoletta_storage::{Error, IssueKind, Locked};
use std::fs;

fn is_unlock_error(e: &Error) -> bool {
    matches!(e, Error::Crypto(CryptoError::Unlock))
}

#[test]
fn notes_round_trip_through_create_read_update_delete() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _rk) = create(dir.path());
    let mut n = note("Title", "a/b", "first body");
    v.create_note(&n).unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
    let rev1 = v.summary(&n.id).unwrap().revision_id;

    n.body = "second body".into();
    n.tags.clear();
    v.update_note(&n).unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
    assert_ne!(v.summary(&n.id).unwrap().revision_id, rev1);

    v.delete_note(&n.id).unwrap();
    assert!(matches!(v.read_note(&n.id), Err(Error::NotFound(_))));
    assert!(v.list().is_empty());
    assert!(matches!(v.delete_note(&n.id), Err(Error::NotFound(_))));
}

#[test]
fn creating_a_duplicate_id_or_updating_a_missing_note_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let n = note("t", "p", "b");
    v.create_note(&n).unwrap();
    assert!(matches!(v.create_note(&n), Err(Error::AlreadyExists(_))));
    let other = note("t", "p", "b");
    assert!(matches!(v.update_note(&other), Err(Error::NotFound(_))));
}

#[test]
fn list_is_sorted_by_path_and_has_no_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    for p in ["z", "a/b", "a"] {
        v.create_note(&note("t", p, "b")).unwrap();
    }
    let paths: Vec<_> = v.list().iter().map(|s| s.path.to_string()).collect();
    assert_eq!(paths, ["a", "a/b", "z"]);
}

#[test]
fn creating_a_vault_twice_in_one_directory_fails() {
    let dir = tempfile::tempdir().unwrap();
    create(dir.path());
    assert!(matches!(
        Locked::create(dir.path(), PASSPHRASE, FAST),
        Err(Error::VaultExists)
    ));
}

#[test]
fn opening_a_directory_without_a_vault_fails() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(Locked::open(dir.path()), Err(Error::NotAVault)));
    fs::create_dir(dir.path().join(META_DIR)).unwrap();
    assert!(matches!(Locked::open(dir.path()), Err(Error::NotAVault)));
}

#[test]
fn short_passphrase_is_rejected_at_creation() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Locked::create(dir.path(), "short", FAST),
        Err(Error::Crypto(CryptoError::PassphraseTooShort { .. }))
    ));
    assert!(matches!(Locked::open(dir.path()), Err(Error::NotAVault)));
}

#[test]
fn wrong_passphrase_and_wrong_recovery_key_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    create(dir.path());
    let l = Locked::open(dir.path()).unwrap();
    assert!(is_unlock_error(
        &l.unlock_with_passphrase("not the passphrase").unwrap_err()
    ));
    let wrong = RecoveryKey::generate().unwrap();
    assert!(is_unlock_error(
        &l.unlock_with_recovery_key(&wrong).unwrap_err()
    ));
}

#[test]
fn lock_returns_a_locked_vault_that_unlocks_again() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let n = note("t", "p", "body");
    v.create_note(&n).unwrap();
    let locked = v.lock();
    let v = locked.unlock_with_passphrase(PASSPHRASE).unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
}

#[test]
fn a_second_opener_is_refused_until_the_first_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (v, _) = create(dir.path());
    assert!(matches!(
        Locked::open(dir.path()).unwrap_err(),
        Error::VaultInUse
    ));
    // Locking keeps the lock: the vault is still open in this process.
    let locked = v.lock();
    assert!(matches!(
        Locked::open(dir.path()).unwrap_err(),
        Error::VaultInUse
    ));
    // A clone of the lock follows the unlocked session; dropping everything releases it.
    let v = locked.unlock_with_passphrase(PASSPHRASE).unwrap();
    drop(locked);
    assert!(matches!(
        Locked::open(dir.path()).unwrap_err(),
        Error::VaultInUse
    ));
    drop(v);
    Locked::open(dir.path()).unwrap();
}

#[test]
fn recovery_key_unlocks_and_new_passphrase_replaces_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, rk) = create(dir.path());
    let n = note("t", "p", "body");
    v.create_note(&n).unwrap();
    drop(v);

    let l = Locked::open(dir.path()).unwrap();
    let mut v = l.unlock_with_recovery_key(&rk).unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
    v.change_passphrase("a brand new passphrase", FAST).unwrap();
    drop(v);
    drop(l);

    let l = Locked::open(dir.path()).unwrap();
    assert!(is_unlock_error(
        &l.unlock_with_passphrase(PASSPHRASE).unwrap_err()
    ));
    let v = l.unlock_with_passphrase("a brand new passphrase").unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
    // The recovery key still works after a passphrase change.
    assert!(l.unlock_with_recovery_key(&rk).is_ok());
}

#[test]
fn recovery_key_rotation_invalidates_the_old_key_and_keeps_notes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, old) = create(dir.path());
    let n = note("t", "p", "body");
    v.create_note(&n).unwrap();
    assert!(is_unlock_error(
        &v.rotate_recovery_key("wrong passphrase!!")
            .map(drop)
            .unwrap_err()
    ));
    let new = v.rotate_recovery_key(PASSPHRASE).unwrap();
    // Session keeps working, including a further header operation.
    v.change_passphrase("another long passphrase", FAST)
        .unwrap();
    drop(v);

    let l = Locked::open(dir.path()).unwrap();
    assert!(is_unlock_error(
        &l.unlock_with_recovery_key(&old).unwrap_err()
    ));
    let v = l.unlock_with_recovery_key(&new).unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
    assert_eq!(l.header().generation(), 3);
}

#[test]
fn a_header_changed_by_another_session_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    create(dir.path());
    let l = Locked::open(dir.path()).unwrap();
    let mut a = l.unlock_with_passphrase(PASSPHRASE).unwrap();
    let mut b = l.unlock_with_passphrase(PASSPHRASE).unwrap();
    a.change_passphrase("first new passphrase", FAST).unwrap();
    assert!(matches!(
        b.change_passphrase("second new passphrase", FAST),
        Err(Error::HeaderChanged)
    ));
}

#[test]
fn flipped_or_truncated_note_files_are_reported_corrupt_and_never_returned() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let good = note("good", "g", "good body");
    let victim = note("victim", "v", "victim body");
    v.create_note(&good).unwrap();
    v.create_note(&victim).unwrap();
    drop(v);
    let file = dir.path().join(NOTES_DIR).join(note_file_name(&victim.id));
    let original = fs::read(&file).unwrap();

    let check = |bytes: &[u8]| {
        fs::write(&file, bytes).unwrap();
        let l = Locked::open(dir.path()).unwrap();
        let v = l.unlock_with_passphrase(PASSPHRASE).unwrap();
        assert_eq!(v.read_note(&good.id).unwrap(), good);
        assert!(matches!(v.read_note(&victim.id), Err(Error::NotFound(_))));
        assert_eq!(v.issues().len(), 1);
        assert_eq!(v.issues()[0].kind, IssueKind::Corrupt);
    };
    for i in (0..original.len()).filter(|i| *i < 96 || i % 31 == 0) {
        let mut t = original.clone();
        t[i] ^= 0x01;
        check(&t);
    }
    for len in (0..original.len()).filter(|l| *l < 100 || l % 29 == 0) {
        check(&original[..len]);
    }
    let mut extended = original.clone();
    extended.push(0);
    check(&extended);
}

#[test]
fn tampered_after_unlock_is_detected_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let n = note("t", "p", "body");
    v.create_note(&n).unwrap();
    let file = dir.path().join(NOTES_DIR).join(note_file_name(&n.id));
    let mut b = fs::read(&file).unwrap();
    let last = b.len() - 1;
    b[last] ^= 1;
    fs::write(&file, b).unwrap();
    assert!(matches!(
        v.read_note(&n.id),
        Err(Error::Crypto(CryptoError::Authentication))
    ));
}

#[test]
fn a_note_file_moved_to_another_id_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let a = note("a", "a", "a");
    v.create_note(&a).unwrap();
    drop(v);
    let other = NoteId::generate().unwrap();
    let notes = dir.path().join(NOTES_DIR);
    fs::copy(
        notes.join(note_file_name(&a.id)),
        notes.join(note_file_name(&other)),
    )
    .unwrap();
    let v = Locked::open(dir.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    assert_eq!(v.list().len(), 1);
    assert_eq!(v.issues().len(), 1);
    assert_eq!(v.issues()[0].kind, IssueKind::Corrupt);
}

#[test]
fn header_damage_is_always_rejected() {
    let dir = tempfile::tempdir().unwrap();
    create(dir.path());
    let file = dir.path().join(META_DIR).join(HEADER_FILE);
    let original = fs::read(&file).unwrap();
    let attempt = |bytes: &[u8]| {
        fs::write(&file, bytes).unwrap();
        match Locked::open(dir.path()) {
            Err(_) => {}
            Ok(l) => assert!(l.unlock_with_passphrase(PASSPHRASE).is_err()),
        }
    };
    for i in (0..original.len()).filter(|i| i % 5 == 0 || *i > 200) {
        let mut t = original.clone();
        t[i] ^= 0x80;
        attempt(&t);
    }
    for len in (0..original.len()).step_by(11) {
        attempt(&original[..len]);
    }
    let mut longer = original.clone();
    longer.push(0);
    attempt(&longer);
}

#[test]
fn leftover_temp_files_from_a_crash_are_cleaned_and_notes_survive() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let n = note("t", "p", "body");
    v.create_note(&n).unwrap();
    drop(v);
    let notes = dir.path().join(NOTES_DIR);
    let leftovers = [
        notes.join(".tmp-0123456789abcdef"),
        notes.join(".tmp-partial"),
        dir.path().join(META_DIR).join(".tmp-fedcba9876543210"),
    ];
    fs::write(&leftovers[0], b"").unwrap();
    // A partially written temporary copy of a real note envelope.
    let real = fs::read(notes.join(note_file_name(&n.id))).unwrap();
    fs::write(&leftovers[1], &real[..real.len() / 2]).unwrap();
    fs::write(&leftovers[2], b"NVLTHDR\0half").unwrap();

    let l = Locked::open(dir.path()).unwrap();
    assert!(leftovers.iter().all(|p| !p.exists()));
    let v = l.unlock_with_passphrase(PASSPHRASE).unwrap();
    assert_eq!(v.read_note(&n.id).unwrap(), n);
    assert!(v.issues().is_empty());
}

#[test]
fn normal_operation_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let mut n = note("t", "p", "b");
    v.create_note(&n).unwrap();
    n.body = "c".into();
    v.update_note(&n).unwrap();
    v.change_passphrase("yet another passphrase", FAST).unwrap();
    for f in all_files(dir.path()) {
        assert!(
            !f.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".tmp-"),
            "{f:?}"
        );
    }
}

#[test]
fn stray_files_and_copies_are_reported_not_read_or_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, _) = create(dir.path());
    let n = note("t", "p", "body");
    v.create_note(&n).unwrap();
    drop(v);
    let notes = dir.path().join(NOTES_DIR);
    let copy = notes.join(format!("{} (1).nvnote", n.id));
    fs::copy(notes.join(note_file_name(&n.id)), &copy).unwrap();
    let upper = notes.join("README.TXT");
    fs::write(&upper, b"hello").unwrap();
    fs::create_dir(notes.join("subdir")).unwrap();

    let v = Locked::open(dir.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    assert_eq!(v.list().len(), 1);
    assert_eq!(v.issues().len(), 3);
    assert!(v.issues().iter().all(|i| i.kind == IssueKind::Stray));
    assert!(copy.exists() && upper.exists());
}

#[cfg(unix)]
mod symlinks {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn symlinked_note_file_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut v, _) = create(dir.path());
        let n = note("t", "p", "body");
        v.create_note(&n).unwrap();
        drop(v);
        let notes = dir.path().join(NOTES_DIR);
        let real = notes.join(note_file_name(&n.id));
        let outside = dir.path().join("outside.bin");
        fs::rename(&real, &outside).unwrap();
        symlink(&outside, &real).unwrap();

        let v = Locked::open(dir.path())
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .unwrap();
        assert!(v.list().is_empty());
        assert_eq!(v.issues()[0].kind, IssueKind::Symlink);
    }

    #[test]
    fn symlinked_header_or_directories_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path());
        let header = dir.path().join(META_DIR).join(HEADER_FILE);
        let copy = dir.path().join("header-copy");
        fs::rename(&header, &copy).unwrap();
        symlink(&copy, &header).unwrap();
        assert!(matches!(
            Locked::open(dir.path()),
            Err(Error::UnsafeEntry(_))
        ));
        fs::remove_file(&header).unwrap();
        fs::rename(&copy, &header).unwrap();

        let notes = dir.path().join(NOTES_DIR);
        let moved = dir.path().join("elsewhere");
        fs::rename(&notes, &moved).unwrap();
        symlink(&moved, &notes).unwrap();
        assert!(matches!(
            Locked::open(dir.path()),
            Err(Error::UnsafeEntry(_))
        ));
    }
}

#[test]
fn no_plaintext_appears_anywhere_in_the_vault_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (mut v, rk) = create(dir.path());
    let secrets = [
        "SECRET-TITLE-zq81",
        "secret/path-zq82/leaf",
        "SECRET-BODY-zq83 lorem ipsum",
        "secret-tag-zq84",
    ];
    let mut n =
        nuvoletta_core::Note::new(secrets[0], secrets[1], vec![secrets[3].into()], secrets[2])
            .unwrap();
    v.create_note(&n).unwrap();
    n.body.push_str(" updated SECRET-BODY-zq85");
    v.update_note(&n).unwrap();
    v.change_passphrase("a long new passphrase", FAST).unwrap();
    drop(v);

    let rk_text = rk.to_display_string().to_string();
    let mut needles: Vec<String> = secrets.iter().map(|s| (*s).to_owned()).collect();
    needles.extend(
        [
            "SECRET-BODY-zq85",
            PASSPHRASE,
            "a long new passphrase",
            &rk_text,
            "zq8",
        ]
        .map(String::from),
    );
    for file in all_files(dir.path()) {
        let name = file.to_string_lossy().into_owned();
        let bytes = fs::read(&file).unwrap();
        for needle in &needles {
            assert!(!name.contains(needle.as_str()), "{name}");
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle} found in {name}"
            );
        }
    }
}

// ---- local trash --------------------------------------------------------------------------

mod trash {
    use super::*;
    use nuvoletta_storage::Unlocked;
    use std::path::{Path, PathBuf};

    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn trash_file(root: &Path, id: &nuvoletta_core::NoteId) -> PathBuf {
        root.join(".nuvoletta/trash")
            .join(nuvoletta_storage::layout::note_file_name(id))
    }

    fn live_file(root: &Path, id: &nuvoletta_core::NoteId) -> PathBuf {
        root.join("notes")
            .join(nuvoletta_storage::layout::note_file_name(id))
    }

    fn setup() -> (tempfile::TempDir, Unlocked, nuvoletta_core::Note) {
        let dir = tempfile::tempdir().unwrap();
        let (mut v, _) = create(dir.path());
        let n = note("Gone", "a/b", "secret body");
        v.create_note(&n).unwrap();
        (dir, v, n)
    }

    #[test]
    fn delete_moves_the_file_unchanged_and_restore_moves_it_back() {
        let (dir, mut v, n) = setup();
        let before = fs::read(live_file(dir.path(), &n.id)).unwrap();
        v.delete_note(&n.id).unwrap();
        assert!(!live_file(dir.path(), &n.id).exists());
        assert_eq!(fs::read(trash_file(dir.path(), &n.id)).unwrap(), before);
        let t = v.list_trash();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].summary.title, "Gone");
        assert_eq!(v.restore_from_trash(&n.id).unwrap(), n.id);
        assert_eq!(fs::read(live_file(dir.path(), &n.id)).unwrap(), before);
        assert!(v.list_trash().is_empty());
        assert_eq!(v.read_note(&n.id).unwrap().body, "secret body");
    }

    #[test]
    fn deleting_a_revived_id_again_keeps_the_older_trashed_version() {
        let (_dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        // The same id comes back with different content, then is deleted again.
        let mut again = n.clone();
        again.title = "Second life".into();
        v.create_note(&again).unwrap();
        v.delete_note(&n.id).unwrap();
        let titles: Vec<String> = v
            .list_trash()
            .into_iter()
            .map(|t| t.summary.title.clone())
            .collect();
        assert_eq!(titles.len(), 2, "{titles:?}");
        assert!(titles.contains(&"Second life".to_string()));
        assert!(titles.contains(&"Gone (earlier version)".to_string()));
    }

    #[test]
    fn deleting_an_identical_copy_again_does_not_duplicate_it() {
        let (dir, mut v, n) = setup();
        let bytes = fs::read(live_file(dir.path(), &n.id)).unwrap();
        v.delete_note(&n.id).unwrap();
        drop(v);
        fs::write(live_file(dir.path(), &n.id), &bytes).unwrap();
        let mut v = Locked::open(dir.path())
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .unwrap();
        v.delete_note(&n.id).unwrap();
        assert_eq!(v.list_trash().len(), 1);
        assert!(!live_file(dir.path(), &n.id).exists());
    }

    #[test]
    fn trash_survives_relock_and_holds_only_ciphertext() {
        let (dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        drop(v);
        for f in all_files(&dir.path().join(".nuvoletta/trash")) {
            let bytes = fs::read(f).unwrap();
            assert!(!bytes.windows(6).any(|w| w == b"secret"));
        }
        let v = Locked::open(dir.path())
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .unwrap();
        assert_eq!(v.list_trash().len(), 1);
    }

    #[test]
    fn entries_expire_after_thirty_days_and_not_before() {
        let (dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        let t = v.list_trash()[0].deleted_ms;
        v.purge_expired_at(t + 30 * DAY - 1).unwrap();
        assert_eq!(v.list_trash().len(), 1);
        v.purge_expired_at(t + 30 * DAY + 1).unwrap();
        assert!(v.list_trash().is_empty());
        assert!(!trash_file(dir.path(), &n.id).exists());
    }

    #[test]
    fn crash_after_index_write_before_move_keeps_the_note_live() {
        let (dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        // Simulate: file never moved (undo the move), index entry remains.
        fs::rename(trash_file(dir.path(), &n.id), live_file(dir.path(), &n.id)).unwrap();
        drop(v);
        let mut v = Locked::open(dir.path())
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .unwrap();
        assert_eq!(v.read_note(&n.id).unwrap().title, "Gone");
        assert!(v.list_trash().is_empty());
        v.purge_expired().unwrap();
        assert!(live_file(dir.path(), &n.id).exists());
    }

    #[test]
    fn note_in_both_places_is_live_and_identical_trash_copy_is_cleaned() {
        let (dir, mut v, n) = setup();
        let live = live_file(dir.path(), &n.id);
        fs::create_dir_all(dir.path().join(".nuvoletta/trash")).unwrap();
        fs::copy(&live, trash_file(dir.path(), &n.id)).unwrap();
        assert!(v.list_trash().is_empty());
        v.purge_expired().unwrap();
        assert!(!trash_file(dir.path(), &n.id).exists());
        assert!(live.exists());
    }

    #[test]
    fn differing_trash_copy_is_kept_hidden_and_restore_never_overwrites() {
        let (dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        // Same id created again (e.g. by sync) while the old version sits in the trash.
        let mut again = n.clone();
        again.body = "newer".into();
        v.create_note(&again).unwrap();
        assert!(v.list_trash().is_empty());
        v.purge_expired().unwrap();
        assert!(trash_file(dir.path(), &n.id).exists());
        // Force it visible by removing the live note's index presence: restore keeps both.
        let new_id = v.restore_from_trash(&n.id).unwrap();
        assert_ne!(new_id, n.id);
        assert_eq!(v.read_note(&n.id).unwrap().body, "newer");
        let copy = v.read_note(&new_id).unwrap();
        assert_eq!(copy.body, "secret body");
        assert!(copy.title.ends_with("(restored)"));
        assert!(!trash_file(dir.path(), &n.id).exists());
    }

    #[test]
    fn missing_or_tampered_index_never_purges_early_and_restore_unknown_fails() {
        let (dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        fs::write(dir.path().join(".nuvoletta/trash/index"), b"garbage").unwrap();
        v.purge_expired().unwrap();
        assert_eq!(v.list_trash().len(), 1);
        let other = nuvoletta_core::NoteId::generate().unwrap();
        assert!(matches!(
            v.restore_from_trash(&other),
            Err(nuvoletta_storage::Error::NotFound(_))
        ));
    }

    #[test]
    fn export_ignores_the_trash() {
        let (_dir, mut v, n) = setup();
        v.delete_note(&n.id).unwrap();
        let out = tempfile::tempdir().unwrap();
        let report = nuvoletta_storage::portable::export_markdown(&v, out.path()).unwrap();
        let _ = report;
        assert!(all_files(out.path()).is_empty());
    }
}
