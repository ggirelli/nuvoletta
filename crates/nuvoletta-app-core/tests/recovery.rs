//! Backup, restore, device-loss and recovery-key exercises, run as automated tests through the
//! application core: a vault restored from a filesystem copy, a new device joining after a loss
//! (synced folder and WebDAV), unlocking with the recovery key, lost or corrupted local state,
//! and a restore from a plaintext export. They mirror the procedures in `docs/user/`.

use nuvoletta_app_core::{
    App, ManualClock, Phase, Settings, SyncEvent, SyncNotice, WebDavEvent, WebDavInput,
};
use nuvoletta_core::NoteId;
use nuvoletta_crypto::KdfParams;
use nuvoletta_webdav::testing::{PASS, Policy, TestServer, USER};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const PW: &str = "correct horse battery";
const NEW_PW: &str = "a brand new passphrase";

struct Dev {
    app: App,
    dir: PathBuf,
}

fn new_app() -> App {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    app.set_kdf_params(KdfParams::MIN_ACCEPTED);
    app
}

fn reopen(dir: &Path, pw: &str) -> App {
    let mut app = new_app();
    app.open_vault(dir).unwrap();
    app.unlock_with_passphrase(pw).unwrap();
    app
}

impl Dev {
    fn create(dir: PathBuf) -> (Self, String) {
        let mut app = new_app();
        let key = app.create_vault(&dir, PW).unwrap().to_string();
        (Self { app, dir }, key)
    }

    fn sync(&mut self) -> SyncEvent {
        self.app.sync_now().unwrap();
        self.app.wait_sync().unwrap()
    }

    fn sync_ok(&mut self) {
        match self.sync() {
            SyncEvent::Finished(f) if f.ok => {}
            other => panic!(
                "sync failed: {other:?} {:?}",
                self.app.sync_status().problem
            ),
        }
    }

    fn write(&mut self, name: &str, body: &str) -> NoteId {
        let id = self.app.create_note("").unwrap();
        self.app.rename_note(id, name).unwrap();
        self.app.edit(id, body).unwrap();
        self.app.save_now().unwrap();
        id
    }

    fn set_body(&mut self, id: NoteId, body: &str) {
        self.app.open_note(id).unwrap();
        self.app.edit(id, body).unwrap();
        self.app.save_now().unwrap();
    }

    /// Title to body of every note.
    fn bodies(&mut self) -> BTreeMap<String, String> {
        let notes = self.app.notes();
        let mut out = BTreeMap::new();
        for n in notes {
            self.app.open_note(n.id).unwrap();
            let body = self.app.buffer().unwrap().to_owned();
            out.insert(n.title, body);
        }
        out
    }

    fn device_id(&self) -> String {
        fs::read_to_string(self.dir.join(".nuvoletta/sync-state/device-id")).unwrap()
    }

    fn notices(&self) -> Vec<String> {
        self.app
            .sync_notices()
            .iter()
            .map(|n: &SyncNotice| n.code.clone())
            .collect()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// What a new device needs besides the passphrase: the vault header, taken from the sync folder
/// (`h/<generation>.nvhdr` has the same bytes as the local `vault-header`).
fn install_header(dir: &Path, header: &[u8]) {
    fs::create_dir_all(dir.join(".nuvoletta")).unwrap();
    fs::write(dir.join(".nuvoletta/vault-header"), header).unwrap();
}

fn folder_header(shared: &Path, generation: u32) -> Vec<u8> {
    fs::read(shared.join(format!("h/{generation:08x}.nvhdr"))).unwrap()
}

fn folder_setup() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    fs::create_dir_all(&shared).unwrap();
    (tmp, shared)
}

fn first_with_folder(tmp: &Path, shared: &Path, name: &str) -> (Dev, String) {
    let (mut d, key) = Dev::create(tmp.join(name));
    d.app.configure_sync(shared).unwrap();
    (d, key)
}

// ---- (a) restore from a filesystem backup ----------------------------------------------------

#[test]
fn restoring_a_backup_copy_regenerates_the_device_id_and_loses_nothing() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let n1 = a.write("One", "first");
    let n2 = a.write("Two", "second");
    a.sync_ok();

    // The backup: a plain copy of the vault directory, taken while the vault is locked.
    a.app.lock(false).unwrap();
    let backup = tmp.path().join("backup");
    copy_dir(&a.dir, &backup);
    a.app.unlock_with_passphrase(PW).unwrap();
    let original_id = a.device_id();

    // Life goes on: A edits and syncs, and a new note appears. Then the disk "dies" and the
    // user restores the (older) backup elsewhere while A keeps running as the same device.
    a.set_body(n1, "first, edited after the backup");
    a.write("Three", "made after the backup");
    a.sync_ok();

    let mut r = Dev {
        app: reopen(&backup, PW),
        dir: backup,
    };
    // The copy still has the old device id and a stale local state: it makes its own edit under
    // the same id before it ever syncs (the fork case).
    assert_eq!(r.device_id(), original_id);
    r.app.configure_sync(&shared).unwrap();
    r.set_body(n2, "second, edited on the restored copy");
    r.sync_ok();
    a.sync_ok();
    r.sync_ok();

    // The fork was detected: the restored copy continues under a new id, nothing was lost, and
    // both devices converge.
    assert_ne!(
        r.device_id(),
        original_id,
        "the clone must regenerate its id"
    );
    let want: BTreeMap<String, String> = [
        ("One", "first, edited after the backup"),
        ("Two", "second, edited on the restored copy"),
        ("Three", "made after the backup"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(a.bodies(), want);
    assert_eq!(r.bodies(), want);
    assert_eq!(a.app.sync_status().conflicts, 0);
    assert_eq!(r.app.sync_status().conflicts, 0);
}

#[test]
fn a_restored_copy_that_edits_the_same_note_yields_a_conflict_with_both_versions() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let n = a.write("Shared", "base");
    a.sync_ok();
    a.app.lock(false).unwrap();
    let backup = tmp.path().join("backup");
    copy_dir(&a.dir, &backup);
    a.app.unlock_with_passphrase(PW).unwrap();

    a.set_body(n, "alpha version");
    a.sync_ok();
    let mut r = Dev {
        app: reopen(&backup, PW),
        dir: backup,
    };
    r.app.configure_sync(&shared).unwrap();
    r.set_body(n, "omega version");
    r.sync_ok();
    a.sync_ok();
    r.sync_ok();

    // Never a silent overwrite: both devices see two versions of the note.
    assert_eq!(a.app.sync_status().conflicts, 1);
    assert_eq!(r.app.sync_status().conflicts, 1);
    let heads: Vec<_> = a.app.sync_conflicts();
    assert_eq!(heads.len(), 1);
    let text = |d: &mut Dev| -> String {
        let c = d.app.sync_conflicts();
        let mut all = String::new();
        for h in &c[0].heads {
            all += d
                .app
                .conflict_head(c[0].note, &h.rev)
                .unwrap()
                .body
                .as_deref()
                .unwrap_or("");
            all.push('|');
        }
        all
    };
    for d in [&mut a, &mut r] {
        let t = text(d);
        assert!(
            t.contains("alpha version") && t.contains("omega version"),
            "{t}"
        );
    }
}

// ---- (b) device loss --------------------------------------------------------------------------

#[test]
fn a_new_device_joins_from_the_synced_folder_with_the_passphrase_alone() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let id = a.write("Plans", "# Plans\nsee [[Other]]");
    a.write("Other", "second");
    a.app.set_tags(id, vec!["work".into()]).unwrap();
    a.sync_ok();
    let want = a.bodies();

    // Device A is lost: its directory is gone. Only the folder and the passphrase remain.
    drop(a);
    fs::remove_dir_all(tmp.path().join("a")).unwrap();

    let dir = tmp.path().join("new");
    install_header(&dir, &folder_header(&shared, 1));
    // Checked before the device is opened: a vault is open in one instance at a time.
    assert!(
        new_app_wrong_pw(&dir),
        "a wrong passphrase must not unlock the new device"
    );
    let mut b = Dev {
        app: reopen(&dir, PW),
        dir,
    };
    b.app.configure_sync(&shared).unwrap();
    b.sync_ok();
    assert_eq!(b.bodies(), want);
    assert_eq!(b.app.tags()[0].tag, "work");
    assert!(b.app.sync_conflicts().is_empty());
    // It can write too, and its edits reach a second new device.
    b.write("From the new device", "hello");
    b.sync_ok();
    let dir2 = tmp.path().join("new2");
    install_header(&dir2, &folder_header(&shared, 1));
    let mut c = Dev {
        app: reopen(&dir2, PW),
        dir: dir2,
    };
    c.app.configure_sync(&shared).unwrap();
    c.sync_ok();
    assert_eq!(c.bodies().len(), 3);
}

fn new_app_wrong_pw(dir: &Path) -> bool {
    let mut app = new_app();
    app.open_vault(dir).unwrap();
    app.unlock_with_passphrase("not the passphrase").is_err() && app.phase() == Phase::Locked
}

fn pin_input(s: &TestServer) -> WebDavInput {
    WebDavInput {
        url: s.url(),
        username: USER.to_owned(),
        password: Zeroizing::new(PASS.to_owned()),
        pin: Some(nuvoletta_webdav::format_fingerprint(&s.fingerprint)),
    }
}

fn webdav_setup(app: &mut App, s: &TestServer) {
    app.webdav_check_start(&pin_input(s)).unwrap();
    let start = Instant::now();
    loop {
        if let Some(e) = app.poll_webdav() {
            assert_eq!(e, WebDavEvent::Saved, "{e:?}");
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "check timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_new_device_joins_over_webdav_with_the_passphrase_and_the_account() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, _key) = Dev::create(tmp.path().join("a"));
    webdav_setup(&mut a.app, &s);
    a.write("Alpha", "one");
    a.write("Beta", "two");
    a.sync_ok();
    let want = a.bodies();
    drop(a);
    fs::remove_dir_all(tmp.path().join("a")).unwrap();

    // The header is fetched from the server's `h/` (a download with the account the user holds).
    let header = s.with(|st| {
        st.files
            .iter()
            .find(|(k, _)| k.starts_with("h/") && k.ends_with(".nvhdr"))
            .map(|(_, f)| f.data.clone())
            .unwrap()
    });
    let dir = tmp.path().join("new");
    install_header(&dir, &header);
    let mut b = Dev {
        app: reopen(&dir, PW),
        dir,
    };
    webdav_setup(&mut b.app, &s);
    b.sync_ok();
    assert_eq!(b.bodies(), want);
    // Credentials are device-local and sealed; the server only ever got create-only requests.
    assert!(b.dir.join(".nuvoletta/device-state/webdav").exists());
}

// ---- (c) recovery key -------------------------------------------------------------------------

#[test]
fn recovery_key_unlock_sets_a_new_passphrase_and_other_devices_continue() {
    let (tmp, shared) = folder_setup();
    let (mut a, key) = first_with_folder(tmp.path(), &shared, "a");
    let n = a.write("Note", "v1");
    a.sync_ok();
    let b_dir = tmp.path().join("b");
    install_header(&b_dir, &folder_header(&shared, 1));
    let mut b = Dev {
        app: reopen(&b_dir, PW),
        dir: b_dir,
    };
    b.app.configure_sync(&shared).unwrap();
    b.sync_ok();

    // The passphrase on A is forgotten: unlock with the recovery key, then set a new one.
    a.app.lock(false).unwrap();
    assert!(a.app.unlock_with_passphrase("forgotten forgotten").is_err());
    a.app.unlock_with_recovery_key(&key).unwrap();
    assert!(a.app.must_change_passphrase());
    a.app.change_passphrase(NEW_PW).unwrap();
    let new_key = a.app.rotate_recovery_key(NEW_PW).unwrap().to_string();
    assert_ne!(new_key, key);
    a.sync_ok();
    assert!(
        shared.join("h/00000003.nvhdr").exists(),
        "each header change is published as a new generation: {:?}",
        fs::read_dir(shared.join("h")).unwrap().count()
    );

    // B keeps working with its header: the notes are untouched (same data key). It is told that
    // a newer header exists, and its edits still flow in both directions.
    b.sync_ok();
    assert!(
        b.notices().contains(&"header-newer".to_owned()),
        "{:?}",
        b.notices()
    );
    b.set_body(n, "v2 from b");
    b.sync_ok();
    a.sync_ok();
    assert_eq!(a.bodies()["Note"], "v2 from b");

    // B adopts the new header (copied from the folder) and then needs the new passphrase.
    // The header is read when the vault is opened, so the vault is closed and opened again.
    b.app.close_vault(false).unwrap();
    install_header(&b.dir, &folder_header(&shared, 3));
    b.app = {
        let mut app = new_app();
        app.open_vault(&b.dir).unwrap();
        assert!(app.unlock_with_passphrase(PW).is_err());
        app.unlock_with_passphrase(NEW_PW).unwrap();
        app
    };
    b.sync_ok();
    assert!(
        !b.notices().contains(&"header-newer".to_owned()) || b.app.sync_status().problem.is_none()
    );
    assert_eq!(b.bodies()["Note"], "v2 from b");
    // The old recovery key no longer opens the vault.
    b.app.lock(false).unwrap();
    assert!(b.app.unlock_with_recovery_key(&key).is_err());
    b.app.unlock_with_recovery_key(&new_key).unwrap();
}

// ---- (d) lost or corrupted local state ---------------------------------------------------------

#[test]
fn losing_the_whole_sync_state_is_rebuilt_from_the_folder_without_data_loss() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let b_dir = tmp.path().join("b");
    let n = a.write("Kept", "synced");
    a.sync_ok();
    install_header(&b_dir, &folder_header(&shared, 1));
    let mut b = Dev {
        app: reopen(&b_dir, PW),
        dir: b_dir,
    };
    b.app.configure_sync(&shared).unwrap();
    b.sync_ok();

    // An unsynced edit, then the sync-state directory (including the device id) is lost.
    a.set_body(n, "edited offline");
    let w = a.write("Unsent", "never synced yet");
    a.app.lock(false).unwrap();
    fs::remove_dir_all(a.dir.join(".nuvoletta/sync-state")).unwrap();
    a.app.unlock_with_passphrase(PW).unwrap();
    a.sync_ok();
    b.sync_ok();
    a.sync_ok();

    // The edit made while the state was lost has an unknown base: it is never dropped and never
    // silently overwrites the synced version, it shows up as a visible two-version conflict
    // (SYNC_PROTOCOL section 10, rule 2). The unsent new note is simply sent.
    assert_eq!(a.app.sync_status().conflicts, 1);
    assert_eq!(b.app.sync_status().conflicts, 1);
    for d in [&mut a, &mut b] {
        let c = d.app.sync_conflicts();
        let mut all = String::new();
        for h in &c[0].heads {
            all += d
                .app
                .conflict_head(c[0].note, &h.rev)
                .unwrap()
                .body
                .as_deref()
                .unwrap_or("");
            all.push('|');
        }
        assert!(
            all.contains("edited offline") && all.contains("synced"),
            "{all}"
        );
    }
    assert_eq!(a.bodies()["Unsent"], "never synced yet");
    assert_eq!(b.bodies()["Unsent"], "never synced yet");
    let _ = w;
}

#[test]
fn every_corrupted_sync_state_file_is_survived() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let n = a.write("Note", "one");
    a.sync_ok();
    let b_dir = tmp.path().join("b");
    install_header(&b_dir, &folder_header(&shared, 1));
    let mut b = Dev {
        app: reopen(&b_dir, PW),
        dir: b_dir,
    };
    b.app.configure_sync(&shared).unwrap();
    b.sync_ok();

    for (round, file) in ["peers", "notes", "inventory", "pending", "max-header-gen"]
        .into_iter()
        .enumerate()
    {
        a.app.lock(false).unwrap();
        let p = a.dir.join(".nuvoletta/sync-state").join(file);
        if p.exists() {
            // Garbage of the right size class: the envelope check fails, never a crash.
            let len = fs::read(&p).unwrap().len();
            fs::write(&p, vec![0xA5; len]).unwrap();
        }
        a.app.unlock_with_passphrase(PW).unwrap();
        // The first cycle rebuilds the state from the folder (verifying every chain again); edits
        // made after that flow normally.
        a.sync_ok();
        let body = format!("round {round} ({file} corrupted)");
        a.set_body(n, &body);
        a.sync_ok();
        b.sync_ok();
        assert_eq!(b.bodies()["Note"], body, "after corrupting {file}");
        a.sync_ok();
    }
    assert_eq!(a.bodies(), b.bodies());
    assert_eq!(a.app.sync_status().conflicts, 0);
}

#[test]
fn corrupted_device_settings_and_notices_do_not_stop_the_vault_from_opening() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    a.write("Note", "kept");
    a.sync_ok();
    a.app.lock(false).unwrap();
    let state = a.dir.join(".nuvoletta/device-state");
    fs::write(state.join("sync.json"), b"{ not json").unwrap();
    fs::write(state.join("sync-notices.json"), b"\xff\xfe garbage").unwrap();
    a.app.unlock_with_passphrase(PW).unwrap();
    // Notes are intact; sync is simply off until the folder is chosen again.
    assert_eq!(a.bodies()["Note"], "kept");
    assert!(!a.app.sync_settings().active());
    a.app.configure_sync(&shared).unwrap();
    a.sync_ok();
    assert_eq!(a.bodies()["Note"], "kept");
}

#[test]
fn losing_the_local_trash_does_not_lose_synced_deletions() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let keep = a.write("Keep", "stays");
    let gone = a.write("Gone", "deleted body");
    a.app.close_note().unwrap();
    a.sync_ok();
    let _ = keep;
    a.app.delete_note(gone).unwrap();
    a.sync_ok();
    assert_eq!(a.app.recently_deleted().unwrap().len(), 1);

    // The local trash is lost: the synced history still has the deleted note.
    a.app.lock(false).unwrap();
    let trash = a.dir.join(".nuvoletta/trash");
    if trash.exists() {
        fs::remove_dir_all(&trash).unwrap();
    }
    a.app.unlock_with_passphrase(PW).unwrap();
    a.sync_ok();
    let items = a.app.recently_deleted().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].can_restore);
    let back = a.app.restore_note(gone).unwrap();
    a.app.open_note(back).unwrap();
    assert_eq!(a.app.buffer(), Some("deleted body"));

    // Without sync, a lost trash only means the deleted note cannot be restored: nothing else.
    let (mut p, _k) = Dev::create(tmp.path().join("plain"));
    let x = p.write("X", "x");
    p.write("Y", "y");
    p.app.close_note().unwrap();
    p.app.delete_note(x).unwrap();
    p.app.lock(false).unwrap();
    fs::remove_dir_all(p.dir.join(".nuvoletta/trash")).unwrap();
    p.app.unlock_with_passphrase(PW).unwrap();
    assert_eq!(p.app.notes().len(), 1);
    assert!(p.app.recently_deleted().unwrap().is_empty());
}

// ---- (e) restore from a plaintext export -------------------------------------------------------

#[test]
fn a_plaintext_export_restores_into_a_brand_new_vault_that_syncs_on_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut a, _key) = Dev::create(tmp.path().join("a"));
    let id = a.write("Plans", "# Plans\nsee [[Other]] #x");
    a.app.set_tags(id, vec!["work".into()]).unwrap();
    a.app.move_note(id, "projects/2026").unwrap();
    a.write("Other", "second");
    let want = a.bodies();
    let export = tmp.path().join("export");
    let report = a.app.export(&export).unwrap();
    assert_eq!(report.exported.len(), 2);
    drop(a);
    fs::remove_dir_all(tmp.path().join("a")).unwrap();

    // A new vault (new keys, new passphrase) takes the notes back.
    let mut n = new_app();
    n.create_vault(&tmp.path().join("new"), NEW_PW).unwrap();
    let mut d = Dev {
        app: n,
        dir: tmp.path().join("new"),
    };
    let imp = d.app.import(&export).unwrap();
    assert_eq!(imp.imported.len(), 2);
    assert!(
        imp.skipped.is_empty() && imp.conflicts.is_empty(),
        "{imp:?}"
    );
    assert_eq!(d.bodies(), want);
    assert_eq!(d.app.tags()[0].tag, "work");
    assert!(d.app.unresolved().is_empty());
    assert!(
        d.app
            .notes()
            .iter()
            .any(|n| n.path.starts_with("projects/2026"))
    );

    // It is a fresh, independent vault: it syncs to its own folder and a second device follows.
    let shared = tmp.path().join("shared-new");
    fs::create_dir_all(&shared).unwrap();
    d.app.configure_sync(&shared).unwrap();
    d.sync_ok();
    let dir2 = tmp.path().join("second");
    install_header(&dir2, &folder_header(&shared, 1));
    let mut s = Dev {
        app: reopen(&dir2, NEW_PW),
        dir: dir2,
    };
    s.app.configure_sync(&shared).unwrap();
    s.sync_ok();
    assert_eq!(s.bodies(), want);
}

#[test]
fn a_corrupted_journal_is_reported_not_guessed_and_deleting_it_recovers() {
    let (tmp, shared) = folder_setup();
    let (mut a, _key) = first_with_folder(tmp.path(), &shared, "a");
    let n = a.write("Note", "one");
    a.sync_ok();
    a.set_body(n, "two, not synced yet");
    a.app.lock(false).unwrap();
    let journal = a.dir.join(".nuvoletta/sync-state/journal");
    let len = fs::read(&journal).unwrap().len();
    fs::write(&journal, vec![0x5A; len]).unwrap();
    a.app.unlock_with_passphrase(PW).unwrap();

    // Sync refuses to run on a journal it cannot verify (it may hold unsent entries) and says
    // so; the notes themselves are untouched.
    assert!(
        a.app.sync_now().is_err()
            || !matches!(a.app.wait_sync(), Some(SyncEvent::Finished(f)) if f.ok)
    );
    assert_eq!(a.bodies()["Note"], "two, not synced yet");

    // Recovery: remove the journal. The next scan finds the note whose revision is not in the
    // own log and sends it again; nothing is lost.
    a.app.lock(false).unwrap();
    fs::remove_file(&journal).unwrap();
    a.app.unlock_with_passphrase(PW).unwrap();
    a.sync_ok();
    let b_dir = tmp.path().join("b");
    install_header(&b_dir, &folder_header(&shared, 1));
    let mut b = Dev {
        app: reopen(&b_dir, PW),
        dir: b_dir,
    };
    b.app.configure_sync(&shared).unwrap();
    b.sync_ok();
    assert_eq!(b.bodies()["Note"], "two, not synced yet");
}
