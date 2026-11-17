//! Adopting a newer vault header after a passphrase change on another device (ADR 0008, part C):
//! the happy paths on an unlocked and a locked device, and every refusal.

use nuvoletta_app_core::{
    AdoptSearchEvent, App, JoinSearchEvent, JoinSecret, JoinSource, KdfLimit, ManualClock, Phase,
    Settings, SyncEvent,
};
use nuvoletta_crypto::KdfParams;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PW: &str = "correct horse battery";
const PW2: &str = "the second passphrase";
const PW3: &str = "the third passphrase";
const FAST: KdfParams = KdfParams::MIN_ACCEPTED;

fn new_app() -> App {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    app.set_kdf_params(FAST);
    app
}

fn sync_ok(app: &mut App) {
    app.sync_now().unwrap();
    match app.wait_sync().unwrap() {
        SyncEvent::Finished(f) if f.ok => {}
        other => panic!("{other:?} {:?}", app.sync_status().problem),
    }
}

fn write(app: &mut App, name: &str, body: &str) {
    let id = app.create_note("").unwrap();
    app.rename_note(id, name).unwrap();
    app.edit(id, body).unwrap();
    app.save_now().unwrap();
}

fn header_on_disk(dir: &Path) -> Vec<u8> {
    fs::read(dir.join(".nuvoletta/vault-header")).unwrap()
}

/// Device A creates the vault and syncs to `shared`.
fn first(tmp: &Path, shared: &Path) -> (App, PathBuf) {
    let dir = tmp.join("a");
    let mut a = new_app();
    a.create_vault(&dir, PW).unwrap();
    a.configure_sync(shared).unwrap();
    write(&mut a, "Plans", "# Plans");
    sync_ok(&mut a);
    (a, dir)
}

/// A new device joins from the folder (the supported way to get a second device).
fn join(tmp: &Path, shared: &Path, name: &str) -> (App, PathBuf) {
    let mut b = new_app();
    b.join_search_start(&JoinSource::Folder(shared.to_path_buf()))
        .unwrap();
    let start = Instant::now();
    loop {
        match b.join_search_poll() {
            Some(JoinSearchEvent::Found(_)) => break,
            Some(other) => panic!("{other:?}"),
            None => {
                assert!(start.elapsed() < Duration::from_secs(30));
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    b.join_verify(0, JoinSecret::Passphrase(PW), false).unwrap();
    let dir = tmp.join(name);
    b.join_confirm(&dir).unwrap();
    assert!(matches!(b.wait_sync(), Some(SyncEvent::Finished(f)) if f.ok));
    (b, dir)
}

fn search(app: &mut App, source: Option<&JoinSource>) -> AdoptSearchEvent {
    app.adopt_search_start(source).unwrap();
    let start = Instant::now();
    loop {
        if let Some(e) = app.adopt_search_poll() {
            return e;
        }
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn notices(app: &App) -> Vec<String> {
    app.sync_notices().iter().map(|n| n.code.clone()).collect()
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    fs::create_dir_all(&shared).unwrap();
    (tmp, shared)
}

#[test]
fn an_unlocked_device_adopts_the_new_passphrase_and_the_notice_goes_away() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, bdir) = join(tmp.path(), &shared, "b");
    a.change_passphrase(PW2).unwrap();
    sync_ok(&mut a);
    sync_ok(&mut b);
    assert!(notices(&b).contains(&"header-newer".to_owned()));

    assert!(matches!(
        search(&mut b, None),
        AdoptSearchEvent::Found {
            current_generation: 1,
            newest_generation: 2,
            diverged: false,
            ..
        }
    ));
    // The wrong passphrase, including the old one, changes nothing.
    let before = header_on_disk(&bdir);
    for wrong in ["not the passphrase", PW] {
        let e = b.adopt_verify(JoinSecret::Passphrase(wrong)).unwrap_err();
        assert!(e.user_message().starts_with("Wrong passphrase"));
    }
    assert_eq!(header_on_disk(&bdir), before);

    let preview = b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap();
    assert_eq!((preview.generation, preview.jump), (2, 1));
    // Nothing is written before the commit.
    assert_eq!(header_on_disk(&bdir), before);
    // Test vaults use the cheapest parameters, which is below a new vault's cost: confirm.
    assert!(preview.weaker_kdf.is_some());
    let e = b.adopt_commit(false).unwrap_err();
    assert!(e.user_message().contains("cheaper"), "{e}");
    assert_eq!(header_on_disk(&bdir), before);
    let out = b.adopt_commit(true).unwrap();
    assert_eq!(out.generation, 2);
    assert_ne!(header_on_disk(&bdir), before);
    assert!(!notices(&b).contains(&"header-newer".to_owned()));

    // The session keeps working and syncing; nothing was re-encrypted.
    write(&mut b, "From b", "hello");
    sync_ok(&mut b);
    sync_ok(&mut a);
    assert_eq!(a.notes().len(), 2);
    assert!(!notices(&b).contains(&"header-newer".to_owned()));
    // After a restart only the new passphrase unlocks it.
    b.lock(false).unwrap();
    assert!(b.unlock_with_passphrase(PW).is_err());
    b.unlock_with_passphrase(PW2).unwrap();
    assert_eq!(b.notes().len(), 2);
}

#[test]
fn adoption_works_with_the_rotated_recovery_key_and_skips_generations() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, _) = join(tmp.path(), &shared, "b");
    a.change_passphrase(PW2).unwrap();
    let new_key = a.rotate_recovery_key(PW2).unwrap().to_string();
    a.change_passphrase(PW3).unwrap();
    sync_ok(&mut a);
    sync_ok(&mut b);
    assert!(matches!(
        search(&mut b, None),
        AdoptSearchEvent::Found {
            newest_generation: 4,
            ..
        }
    ));
    let preview = b.adopt_verify(JoinSecret::RecoveryKey(&new_key)).unwrap();
    assert_eq!((preview.generation, preview.jump), (4, 3));
    b.adopt_commit(true).unwrap();
    b.lock(false).unwrap();
    // The passphrase of generation 4 opens it now; the older ones do not.
    assert!(b.unlock_with_passphrase(PW2).is_err());
    b.unlock_with_passphrase(PW3).unwrap();
}

#[test]
fn a_replayed_older_header_is_refused_and_never_written() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, bdir) = join(tmp.path(), &shared, "b");
    a.change_passphrase(PW2).unwrap();
    sync_ok(&mut a); // publishes generation 2
    a.change_passphrase(PW3).unwrap();
    sync_ok(&mut a);
    sync_ok(&mut b); // b has now verified generation 3
    // The provider withholds generation 3 and serves the genuine generation 2.
    fs::remove_file(shared.join("h/00000003.nvhdr")).unwrap();
    assert!(matches!(
        search(&mut b, None),
        AdoptSearchEvent::Found {
            newest_generation: 2,
            ..
        }
    ));
    let before = header_on_disk(&bdir);
    let e = b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap_err();
    assert!(e.user_message().contains("older"), "{e}");
    assert!(b.adopt_commit(true).is_err());
    assert_eq!(header_on_disk(&bdir), before);
    // The next cycle reports the rollback as before.
    sync_ok(&mut b);
    assert!(notices(&b).contains(&"header-rollback".to_owned()));
}

#[test]
fn another_vaults_junk_and_unauthenticated_headers_are_not_candidates() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, bdir) = join(tmp.path(), &shared, "b");
    // A header of another vault, junk, and a forged high generation of this vault.
    let other_dir = tmp.path().join("other");
    let mut other = new_app();
    other
        .create_vault(&other_dir, "an unrelated passphrase")
        .unwrap();
    let foreign = header_on_disk(&other_dir);
    drop(other);
    fs::write(shared.join("h/00000005.nvhdr (x).nvhdr"), &foreign).unwrap();
    fs::write(shared.join("h/00000009.nvhdr"), b"junk").unwrap();
    let mut forged = header_on_disk(&bdir);
    forged[10..14].copy_from_slice(&7u32.to_be_bytes());
    fs::write(shared.join("h/00000007.nvhdr"), &forged).unwrap();
    // The forged one has this vault's id and a high generation: it is a candidate by structure,
    // but no secret opens it (MAC), so it can never be adopted.
    let before = header_on_disk(&bdir);
    match search(&mut b, None) {
        AdoptSearchEvent::Found {
            newest_generation: 7,
            ..
        } => {
            for pw in [PW, PW2] {
                assert!(b.adopt_verify(JoinSecret::Passphrase(pw)).is_err());
            }
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(header_on_disk(&bdir), before);
    a.lock(false).unwrap();
}

#[test]
fn a_header_that_opens_but_lowers_the_cost_needs_confirmation_and_a_raised_one_does_not() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, _) = join(tmp.path(), &shared, "b");
    // Same cost as held here, and the cost of a new vault in the test: raise it.
    a.set_kdf_params(KdfParams::DEFAULT);
    a.change_passphrase(PW2).unwrap();
    sync_ok(&mut a);
    sync_ok(&mut b);
    search(&mut b, None);
    let p = b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap();
    assert!(p.weaker_kdf.is_none(), "{p:?}");
    b.adopt_commit(false).unwrap();
}

#[test]
fn the_android_kdf_limit_refuses_a_costly_header_for_a_passphrase_only() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, _) = join(tmp.path(), &shared, "b");
    a.set_kdf_params(KdfParams {
        m_kib: 16384,
        t: 2,
        p: 1,
    });
    a.change_passphrase(PW2).unwrap();
    let key = a.rotate_recovery_key(PW2).unwrap().to_string();
    sync_ok(&mut a);
    sync_ok(&mut b);
    b.set_kdf_limit(KdfLimit {
        max_m_kib: 8192,
        max_t: 8,
    });
    match search(&mut b, None) {
        AdoptSearchEvent::Found { too_costly, .. } => assert!(too_costly.is_some()),
        other => panic!("{other:?}"),
    }
    let e = b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap_err();
    assert!(e.user_message().contains("more than this device accepts"));
    // The recovery key runs no Argon2, so the limit does not apply to it.
    b.adopt_verify(JoinSecret::RecoveryKey(&key)).unwrap();
    b.adopt_cancel();
    assert!(b.adopt_commit(true).is_err());
}

#[test]
fn two_devices_changing_the_passphrase_at_once_adopt_the_header_the_passphrase_opens() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, bdir) = join(tmp.path(), &shared, "b");
    let (mut c, cdir) = join(tmp.path(), &shared, "c");
    a.change_passphrase("passphrase chosen on a").unwrap();
    b.change_passphrase("passphrase chosen on b").unwrap();
    c.change_passphrase("passphrase chosen on c").unwrap();
    sync_ok(&mut a); // a wins the name of generation 2
    // c's file is also a genuine generation 2: a synced folder keeps it as a conflict copy.
    fs::write(
        shared.join("h/00000002.nvhdr (conflict).nvhdr"),
        header_on_disk(&cdir),
    )
    .unwrap();
    sync_ok(&mut b);
    // The lost race is its own notice, not "invalid".
    assert!(
        notices(&b).contains(&"header-diverged".to_owned()),
        "{:?}",
        notices(&b)
    );
    assert!(!notices(&b).contains(&"header-invalid".to_owned()));

    match search(&mut b, None) {
        AdoptSearchEvent::Found {
            current_generation: 2,
            newest_generation: 2,
            diverged: true,
            ..
        } => {}
        other => panic!("{other:?}"),
    }
    let own = header_on_disk(&bdir);
    // The passphrase of a opens a's header; the warning names the other one.
    let p = b
        .adopt_verify(JoinSecret::Passphrase("passphrase chosen on a"))
        .unwrap();
    assert_eq!(p.generation, 2);
    assert!(
        p.warnings.iter().any(|w| w.contains("other header")),
        "{p:?}"
    );
    b.adopt_commit(true).unwrap();
    assert_ne!(header_on_disk(&bdir), own);
    assert!(!notices(&b).contains(&"header-diverged".to_owned()));
    // The other header is kept in view, not dropped.
    assert!(notices(&b).contains(&"header-diverged-other".to_owned()));
    b.lock(false).unwrap();
    assert!(b.unlock_with_passphrase("passphrase chosen on b").is_err());
    b.unlock_with_passphrase("passphrase chosen on a").unwrap();
    // c's passphrase opens c's header, which can be adopted the same way later.
    drop(c.lock(false));
}

#[test]
fn a_locked_device_adopts_with_only_the_new_passphrase_known() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, bdir) = join(tmp.path(), &shared, "b");
    b.lock(false).unwrap();
    a.change_passphrase(PW2).unwrap();
    sync_ok(&mut a);

    assert!(!b.adopt_needs_source());
    assert!(matches!(
        search(&mut b, None),
        AdoptSearchEvent::Found {
            newest_generation: 2,
            ..
        }
    ));
    // The old header cannot open with the new passphrase: the candidate is opened and checked
    // against the notes and state held here before the header is replaced.
    let before = header_on_disk(&bdir);
    assert!(
        b.adopt_verify(JoinSecret::Passphrase("not the passphrase"))
            .is_err()
    );
    b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap();
    assert_eq!(header_on_disk(&bdir), before);
    b.adopt_commit(true).unwrap();
    assert_ne!(header_on_disk(&bdir), before);
    assert_eq!(b.phase(), Phase::Locked);
    assert!(b.unlock_with_passphrase(PW).is_err());
    b.unlock_with_passphrase(PW2).unwrap();
    assert_eq!(b.notes().len(), 1);
    // The verified generation was recorded, so the next sync raises no notice.
    sync_ok(&mut b);
    assert!(!notices(&b).contains(&"header-newer".to_owned()));
}

#[test]
fn a_locked_device_refuses_a_header_whose_keys_do_not_open_its_data() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    let (mut b, bdir) = join(tmp.path(), &shared, "b");
    b.lock(false).unwrap();
    a.change_passphrase(PW2).unwrap();
    sync_ok(&mut a);
    // Replace everything sealed on b by data of another vault: the candidate's keys open none.
    let foreign_dir = tmp.path().join("foreign");
    let (mut f, _) =
        nuvoletta_storage::Locked::create(&foreign_dir, "foreign passphrase!", FAST).unwrap();
    let n = nuvoletta_core::Note::new("f", "f", vec![], "foreign").unwrap();
    f.create_note(&n).unwrap();
    let bytes = f.read_revision_bytes(&n.id).unwrap();
    drop(f);
    for d in ["sync-state", "device-state", "trash"] {
        let _ = fs::remove_dir_all(bdir.join(".nuvoletta").join(d));
    }
    for e in fs::read_dir(bdir.join("notes")).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    fs::write(bdir.join(format!("notes/{}.nvnote", n.id)), bytes).unwrap();
    let before = header_on_disk(&bdir);
    search(&mut b, Some(&JoinSource::Folder(shared.clone())));
    let e = b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap_err();
    assert!(e.user_message().contains("do not match"), "{e}");
    assert!(b.adopt_commit(true).is_err());
    assert_eq!(header_on_disk(&bdir), before);
}

#[test]
fn a_locked_device_with_no_data_to_check_refuses() {
    let (tmp, shared) = setup();
    let (mut a, adir) = first(tmp.path(), &shared);
    // b is a vault copy with an empty sync state and no notes: nothing is sealed there.
    let bdir = tmp.path().join("b");
    fs::create_dir_all(bdir.join(".nuvoletta")).unwrap();
    fs::copy(
        adir.join(".nuvoletta/vault-header"),
        bdir.join(".nuvoletta/vault-header"),
    )
    .unwrap();
    a.change_passphrase(PW2).unwrap();
    sync_ok(&mut a);
    let mut b = new_app();
    b.open_vault(&bdir).unwrap();
    // No synced folder is known here: the location has to be given.
    assert!(b.adopt_needs_source());
    assert!(b.adopt_search_start(None).is_err());
    search(&mut b, Some(&JoinSource::Folder(shared)));
    let e = b.adopt_verify(JoinSecret::Passphrase(PW2)).unwrap_err();
    assert!(e.user_message().contains("no notes or sync data"), "{e}");
}

#[test]
fn up_to_date_devices_and_missing_sync_are_reported() {
    let (tmp, shared) = setup();
    let (mut a, _) = first(tmp.path(), &shared);
    assert_eq!(search(&mut a, None), AdoptSearchEvent::UpToDate);
    let mut lone = new_app();
    lone.create_vault(&tmp.path().join("lone"), PW).unwrap();
    assert!(lone.adopt_search_start(None).is_err());
    assert!(lone.adopt_verify(JoinSecret::Passphrase(PW)).is_err());
}
