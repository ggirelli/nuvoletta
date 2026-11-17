//! Joining a new device to an existing vault from the sync location (ADR 0008): with the
//! passphrase and with the recovery key, over a folder and over WebDAV, refusals of forged or
//! unauthenticated headers, several vaults in one folder, the KDF limit and cancelling.

use nuvoletta_app_core::{
    App, JoinCancel, JoinCandidate, JoinSearchEvent, JoinSecret, JoinSource, KdfLimit, ManualClock,
    Phase, Settings, SyncEvent, WebDavInput,
};
use nuvoletta_crypto::{KdfParams, Vault};
use nuvoletta_webdav::testing::{PASS, Policy, TestServer, USER};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const PW: &str = "correct horse battery";
const NEW_PW: &str = "a brand new passphrase";

fn new_app() -> App {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    app.set_kdf_params(KdfParams::MIN_ACCEPTED);
    app
}

fn folder_setup() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    fs::create_dir_all(&shared).unwrap();
    (tmp, shared)
}

/// Device A: a vault with notes, synced to `shared`, then lost. Returns the recovery key.
fn lost_device(tmp: &Path, shared: &Path) -> (String, BTreeMap<String, String>) {
    let dir = tmp.join("a");
    let mut a = new_app();
    let key = a.create_vault(&dir, PW).unwrap().to_string();
    a.configure_sync(shared).unwrap();
    write(&mut a, "Plans", "# Plans\nsee [[Other]]");
    write(&mut a, "Other", "second");
    a.sync_now().unwrap();
    a.wait_sync().unwrap();
    let want = bodies(&mut a);
    drop(a);
    fs::remove_dir_all(dir).unwrap();
    (key, want)
}

fn write(app: &mut App, name: &str, body: &str) {
    let id = app.create_note("").unwrap();
    app.rename_note(id, name).unwrap();
    app.edit(id, body).unwrap();
    app.save_now().unwrap();
}

fn bodies(app: &mut App) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for n in app.notes() {
        app.open_note(n.id).unwrap();
        out.insert(n.title, app.buffer().unwrap().to_owned());
    }
    out
}

fn search(app: &mut App, source: &JoinSource) -> JoinSearchEvent {
    app.join_search_start(source).unwrap();
    let start = Instant::now();
    loop {
        if let Some(e) = app.join_search_poll() {
            return e;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "search timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn found(e: JoinSearchEvent) -> Vec<JoinCandidate> {
    match e {
        JoinSearchEvent::Found(c) => c,
        other => panic!("{other:?}"),
    }
}

fn first_sync(app: &mut App) -> SyncEvent {
    app.wait_sync().expect("the first sync was started")
}

fn header_bytes(shared: &Path, generation: u32) -> Vec<u8> {
    fs::read(shared.join(format!("h/{generation:08x}.nvhdr"))).unwrap()
}

fn grouped(header: &[u8]) -> String {
    let id = &header[14..30];
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn joining_with_the_passphrase_fetches_the_notes_and_stores_the_location() {
    let (tmp, shared) = folder_setup();
    let (_key, want) = lost_device(tmp.path(), &shared);

    let mut b = new_app();
    let cands = found(search(&mut b, &JoinSource::Folder(shared.clone())));
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].newest_generation, 1);
    assert_eq!(cands[0].fingerprint, grouped(&header_bytes(&shared, 1)));
    assert!(!cands[0].diverged && cands[0].too_costly.is_none());

    let preview = b.join_verify(0, JoinSecret::Passphrase(PW), false).unwrap();
    assert_eq!(preview.generation, 1);
    assert_eq!(preview.fingerprint, cands[0].fingerprint);
    assert!(preview.warnings.is_empty() && !preview.recovery_key);
    // Nothing has been written yet.
    let dir = tmp.path().join("new");
    assert!(!dir.exists());

    b.join_confirm(&dir).unwrap();
    assert_eq!(b.phase(), Phase::Unlocked);
    assert!(b.join_in_progress());
    assert!(!b.must_change_passphrase());
    match first_sync(&mut b) {
        SyncEvent::Finished(f) if f.ok => {}
        other => panic!("{other:?} {:?}", b.sync_status().problem),
    }
    assert!(!b.join_in_progress());
    assert!(!b.must_change_passphrase());
    assert_eq!(bodies(&mut b), want);
    assert!(b.sync_settings().active());
    assert!(
        dir.join(".nuvoletta/sync-state/verified-header-gen")
            .exists()
    );
    assert_eq!(
        fs::read(dir.join(".nuvoletta/vault-header")).unwrap(),
        header_bytes(&shared, 1)
    );
    // It is a normal vault afterwards: it can lock and unlock again.
    b.lock(false).unwrap();
    b.unlock_with_passphrase(PW).unwrap();
}

#[test]
fn joining_with_the_recovery_key_asks_for_a_new_passphrase_only_after_the_first_sync() {
    let (tmp, shared) = folder_setup();
    let (key, want) = lost_device(tmp.path(), &shared);

    let mut b = new_app();
    found(search(&mut b, &JoinSource::Folder(shared.clone())));
    // A recovery key typed in lower case without the hyphens is accepted as everywhere else.
    let typed = key.to_lowercase().replace('-', "");
    let preview = b
        .join_verify(0, JoinSecret::RecoveryKey(&typed), false)
        .unwrap();
    assert!(preview.recovery_key);
    let dir = tmp.path().join("new");
    b.join_confirm(&dir).unwrap();
    // Not before the first sync: the device has published nothing yet.
    assert!(!b.must_change_passphrase());
    assert!(b.join_in_progress());
    match first_sync(&mut b) {
        SyncEvent::Finished(f) if f.ok => {}
        other => panic!("{other:?}"),
    }
    assert!(b.must_change_passphrase());
    assert!(!b.join_in_progress());
    assert_eq!(bodies(&mut b), want);
    // The new passphrase is a new header generation, published by the next sync.
    b.change_passphrase(NEW_PW).unwrap();
    assert!(!b.must_change_passphrase());
    b.sync_now().unwrap();
    b.wait_sync().unwrap();
    assert!(shared.join("h/00000002.nvhdr").exists());
}

#[test]
fn a_wrong_passphrase_or_recovery_key_is_refused_and_nothing_is_written() {
    let (tmp, shared) = folder_setup();
    let (key, _) = lost_device(tmp.path(), &shared);
    let mut b = new_app();
    found(search(&mut b, &JoinSource::Folder(shared)));
    let generic = "Wrong passphrase or recovery key, or the vault header is damaged.";
    let e = b
        .join_verify(0, JoinSecret::Passphrase("not the passphrase"), false)
        .unwrap_err();
    assert_eq!(e.user_message(), generic);
    let wrong_key = nuvoletta_crypto::RecoveryKey::generate().unwrap();
    let e = b
        .join_verify(
            0,
            JoinSecret::RecoveryKey(&wrong_key.to_display_string()),
            false,
        )
        .unwrap_err();
    assert_eq!(e.user_message(), generic);
    // Confirming without a verified header does nothing.
    let dir = tmp.path().join("new");
    assert!(b.join_confirm(&dir).is_err());
    assert!(!dir.exists());
    assert_eq!(b.phase(), Phase::NoVault);
    // The right one still works after failures.
    b.join_verify(0, JoinSecret::RecoveryKey(&key), false)
        .unwrap();
}

/// A vault of someone else (or an attacker) with a passphrase the attacker knows.
fn other_vault_header(pw: &str) -> Vec<u8> {
    Vault::create(pw, KdfParams::MIN_ACCEPTED)
        .unwrap()
        .vault
        .header()
        .as_bytes()
        .to_vec()
}

#[test]
fn forged_or_tampered_headers_never_open_a_vault() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    let genuine_bytes = header_bytes(&shared, 1);
    let genuine = nuvoletta_crypto::VaultHeader::parse(&genuine_bytes).unwrap();
    // A forged name: generation 1 content under the name of generation 9 is ignored.
    fs::write(shared.join("h/00000009.nvhdr"), &genuine_bytes).unwrap();
    // Generation 2 with the right passphrase but a flipped byte in the MAC: it parses and the
    // passphrase opens the wrap, but the MAC fails, so it must not open.
    let v1 = genuine.unlock_with_passphrase(PW).unwrap();
    let h2 = v1
        .change_passphrase(NEW_PW, KdfParams::MIN_ACCEPTED)
        .unwrap();
    let mut bad_mac = *h2.as_bytes();
    bad_mac[235] ^= 1;
    fs::write(shared.join("h/00000002.nvhdr"), bad_mac).unwrap();
    // Generation 3 with a flipped byte in the wrap: the AEAD fails.
    let v2 = h2.unlock_with_passphrase(NEW_PW).unwrap();
    let h3 = v2
        .change_passphrase("a third passphrase", KdfParams::MIN_ACCEPTED)
        .unwrap();
    let mut bad_wrap = *h3.as_bytes();
    bad_wrap[90] ^= 1;
    fs::write(shared.join("h/00000003.nvhdr"), bad_wrap).unwrap();

    let mut b = new_app();
    let cands = found(search(&mut b, &JoinSource::Folder(shared)));
    assert_eq!(cands.len(), 1);
    // The forged name is gone; the tampered headers are the newest.
    assert_eq!(cands[0].newest_generation, 3);
    for pw in [PW, NEW_PW, "a third passphrase"] {
        assert!(b.join_verify(0, JoinSecret::Passphrase(pw), false).is_err());
    }
    // Even when the user falls back to older headers, the tampered ones never open.
    assert!(
        b.join_verify(0, JoinSecret::Passphrase(NEW_PW), true)
            .is_err()
    );
    assert!(
        b.join_verify(0, JoinSecret::Passphrase("a third passphrase"), true)
            .is_err()
    );
    // The genuine generation 1 opens, with a warning that it is not the newest.
    let preview = b.join_verify(0, JoinSecret::Passphrase(PW), true).unwrap();
    assert_eq!(preview.generation, 1);
    assert_eq!(preview.newest_generation, 3);
    assert!(preview.warnings.iter().any(|w| w.contains("older header")));
}

#[test]
fn a_header_of_another_vault_opens_only_with_its_own_passphrase_and_is_told_apart() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    // Another vault in the same folder, stored as a conflict-style copy of generation 1.
    let other = other_vault_header("an attacker chosen passphrase");
    fs::write(shared.join("h/00000001.nvhdr (other).nvhdr"), &other).unwrap();

    let mut b = new_app();
    let cands = found(search(&mut b, &JoinSource::Folder(shared.clone())));
    assert_eq!(cands.len(), 2);
    assert_ne!(cands[0].fingerprint, cands[1].fingerprint);
    let mine = cands
        .iter()
        .find(|c| c.fingerprint == grouped(&header_bytes(&shared, 1)))
        .unwrap()
        .index;
    let theirs = cands.iter().find(|c| c.index != mine).unwrap().index;
    assert_eq!(cands[theirs].fingerprint, grouped(&other));
    // Each opens only with its own passphrase.
    assert!(
        b.join_verify(
            mine,
            JoinSecret::Passphrase("an attacker chosen passphrase"),
            false
        )
        .is_err()
    );
    assert!(
        b.join_verify(theirs, JoinSecret::Passphrase(PW), false)
            .is_err()
    );
    let p = b
        .join_verify(mine, JoinSecret::Passphrase(PW), false)
        .unwrap();
    assert_eq!(p.fingerprint, cands[mine].fingerprint);
    let p = b
        .join_verify(
            theirs,
            JoinSecret::Passphrase("an attacker chosen passphrase"),
            false,
        )
        .unwrap();
    assert_eq!(p.fingerprint, cands[theirs].fingerprint);
    // An index that does not exist is refused.
    assert!(b.join_verify(5, JoinSecret::Passphrase(PW), false).is_err());
}

#[test]
fn two_headers_of_one_generation_use_the_one_the_passphrase_opens_and_warn_about_the_other() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    // Same vault, two passphrase changes from generation 1: two different generation-2 headers.
    let genuine = nuvoletta_crypto::VaultHeader::parse(&header_bytes(&shared, 1)).unwrap();
    let vault = genuine.unlock_with_passphrase(PW).unwrap();
    let x = vault
        .change_passphrase("passphrase chosen on X", KdfParams::MIN_ACCEPTED)
        .unwrap();
    let y = vault
        .change_passphrase("passphrase chosen on Y", KdfParams::MIN_ACCEPTED)
        .unwrap();
    fs::write(shared.join("h/00000002.nvhdr"), x.as_bytes()).unwrap();
    fs::write(shared.join("h/00000002.nvhdr (copy).nvhdr"), y.as_bytes()).unwrap();

    let mut b = new_app();
    let cands = found(search(&mut b, &JoinSource::Folder(shared)));
    assert!(cands[0].diverged);
    for (pw, _) in [("passphrase chosen on X", 0), ("passphrase chosen on Y", 1)] {
        let p = b.join_verify(0, JoinSecret::Passphrase(pw), false).unwrap();
        assert_eq!(p.generation, 2);
        assert!(
            p.warnings.iter().any(|w| w.contains("other header")),
            "{p:?}"
        );
    }
    // The generation-1 passphrase opens neither (and the older header is not used silently).
    assert!(b.join_verify(0, JoinSecret::Passphrase(PW), false).is_err());
}

#[test]
fn the_kdf_limit_refuses_a_costly_header_for_a_passphrase_but_not_for_the_recovery_key() {
    let (tmp, shared) = folder_setup();
    let (key, _) = lost_device(tmp.path(), &shared);
    // Rewrite generation 1 as the same vault would look with a costlier setting: a passphrase
    // change at m = 16 MiB, t = 2 published as generation 2.
    let genuine = nuvoletta_crypto::VaultHeader::parse(&header_bytes(&shared, 1)).unwrap();
    let vault = genuine.unlock_with_passphrase(PW).unwrap();
    let costly = vault
        .change_passphrase(
            NEW_PW,
            KdfParams {
                m_kib: 16384,
                t: 2,
                p: 1,
            },
        )
        .unwrap();
    fs::write(shared.join("h/00000002.nvhdr"), costly.as_bytes()).unwrap();

    let mut b = new_app();
    b.set_kdf_limit(KdfLimit {
        max_m_kib: 8192,
        max_t: 8,
    });
    let cands = found(search(&mut b, &JoinSource::Folder(shared.clone())));
    let why = cands[0].too_costly.clone().expect("flagged");
    assert!(why.contains("16 MiB") && why.contains("8 MiB"), "{why}");
    assert_eq!(cands[0].kdf_mib, 16);
    let e = b
        .join_verify(0, JoinSecret::Passphrase(NEW_PW), false)
        .unwrap_err();
    assert!(
        e.user_message().contains("more than this device accepts"),
        "{e}"
    );
    // The recovery key does not run Argon2, so it is not subject to the limit.
    assert!(
        b.join_verify(0, JoinSecret::RecoveryKey(&key), false)
            .is_ok()
    );
    // Without a limit (desktop) the same passphrase works.
    b.set_kdf_limit(KdfLimit::FORMAT);
    assert!(
        b.join_verify(0, JoinSecret::Passphrase(NEW_PW), false)
            .is_ok()
    );
}

#[test]
fn a_folder_without_a_header_or_that_does_not_exist_is_reported_and_never_created() {
    let (tmp, shared) = folder_setup();
    let mut b = new_app();
    match search(&mut b, &JoinSource::Folder(shared.clone())) {
        JoinSearchEvent::NoVault { message } => assert!(message.contains("no vault header")),
        other => panic!("{other:?}"),
    }
    let missing = tmp.path().join("missing");
    match search(&mut b, &JoinSource::Folder(missing.clone())) {
        JoinSearchEvent::Failed { message, .. } => assert!(message.contains("does not exist")),
        other => panic!("{other:?}"),
    }
    assert!(!missing.exists());
    // Garbage in h/ is skipped and counted.
    fs::create_dir_all(shared.join("h")).unwrap();
    fs::write(shared.join("h/00000001.nvhdr"), b"junk").unwrap();
    match search(&mut b, &JoinSource::Folder(shared)) {
        JoinSearchEvent::NoVault { message } => assert!(message.contains("skipped")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_sync_folder_inside_the_new_vault_is_refused_and_leaves_nothing_behind() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    let mut b = new_app();
    found(search(&mut b, &JoinSource::Folder(shared.clone())));
    b.join_verify(0, JoinSecret::Passphrase(PW), false).unwrap();
    // The vault directory would contain the sync folder.
    assert!(b.join_confirm(tmp.path()).is_err());
    assert_eq!(b.phase(), Phase::NoVault);
    assert!(!tmp.path().join(".nuvoletta").exists());
    assert!(shared.exists());
}

#[test]
fn cancelling_before_anything_is_written_just_drops_the_state() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    let mut b = new_app();
    found(search(&mut b, &JoinSource::Folder(shared)));
    b.join_verify(0, JoinSecret::Passphrase(PW), false).unwrap();
    assert_eq!(b.join_cancel().unwrap(), JoinCancel::Discarded);
    assert!(b.join_confirm(&tmp.path().join("new")).is_err());
}

/// Make the first sync fail: `h` becomes a symlink, which the folder provider refuses.
#[cfg(unix)]
fn break_folder(shared: &Path) -> PathBuf {
    let aside = shared.with_file_name("h-aside");
    fs::rename(shared.join("h"), &aside).unwrap();
    std::os::unix::fs::symlink(&aside, shared.join("h")).unwrap();
    aside
}

#[cfg(unix)]
fn join_with_broken_first_sync(tmp: &Path, shared: &Path) -> (App, PathBuf, PathBuf) {
    let mut b = new_app();
    found(search(&mut b, &JoinSource::Folder(shared.to_path_buf())));
    b.join_verify(0, JoinSecret::Passphrase(PW), false).unwrap();
    let aside = break_folder(shared);
    let dir = tmp.join("new");
    b.join_confirm(&dir).unwrap();
    match first_sync(&mut b) {
        SyncEvent::Finished(f) => assert!(!f.ok),
        other => panic!("{other:?}"),
    }
    assert!(b.join_in_progress());
    assert!(b.sync_status().problem.is_some());
    (b, dir, aside)
}

#[cfg(unix)]
#[test]
fn a_failed_first_sync_can_be_retried_and_then_completes_the_join() {
    let (tmp, shared) = folder_setup();
    let (_k, want) = lost_device(tmp.path(), &shared);
    let (mut b, _dir, aside) = join_with_broken_first_sync(tmp.path(), &shared);
    fs::remove_file(shared.join("h")).unwrap();
    fs::rename(aside, shared.join("h")).unwrap();
    b.sync_now().unwrap();
    match first_sync(&mut b) {
        SyncEvent::Finished(f) if f.ok => {}
        other => panic!("{other:?}"),
    }
    assert!(!b.join_in_progress());
    assert_eq!(bodies(&mut b), want);
}

#[cfg(unix)]
#[test]
fn cancelling_a_join_without_notes_removes_the_vault_directory() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    let (mut b, dir, _aside) = join_with_broken_first_sync(tmp.path(), &shared);
    assert_eq!(b.join_cancel().unwrap(), JoinCancel::Removed);
    assert_eq!(b.phase(), Phase::NoVault);
    assert!(!dir.exists());
    // The sync folder is untouched.
    assert!(shared.join("d").exists());
}

#[cfg(unix)]
#[test]
fn cancelling_a_join_that_holds_a_note_keeps_the_directory_and_says_where() {
    let (tmp, shared) = folder_setup();
    lost_device(tmp.path(), &shared);
    let (mut b, dir, _aside) = join_with_broken_first_sync(tmp.path(), &shared);
    write(&mut b, "Mine", "typed while joining");
    match b.join_cancel().unwrap() {
        JoinCancel::Kept(p) => assert_eq!(p, dir),
        other => panic!("{other:?}"),
    }
    assert_eq!(b.phase(), Phase::NoVault);
    assert!(dir.join(".nuvoletta/vault-header").exists());
    // The note is still there for the user.
    let mut again = new_app();
    again.open_vault(&dir).unwrap();
    again.unlock_with_passphrase(PW).unwrap();
    assert_eq!(
        bodies(&mut again).get("Mine").map(String::as_str),
        Some("typed while joining")
    );
}

fn dav_input(s: &TestServer) -> WebDavInput {
    WebDavInput {
        url: s.url(),
        username: USER.to_owned(),
        password: Zeroizing::new(PASS.to_owned()),
        pin: Some(nuvoletta_webdav::format_fingerprint(&s.fingerprint)),
    }
}

#[test]
fn joining_over_webdav_checks_the_account_and_stores_sealed_credentials() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("a");
    let mut a = new_app();
    a.create_vault(&dir_a, PW).unwrap();
    a.webdav_check_start(&dav_input(&s)).unwrap();
    let start = Instant::now();
    while a.poll_webdav().is_none() {
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(20));
    }
    write(&mut a, "Alpha", "one");
    a.sync_now().unwrap();
    a.wait_sync().unwrap();
    let want = bodies(&mut a);
    drop(a);
    fs::remove_dir_all(&dir_a).unwrap();

    let mut b = new_app();
    found(search(&mut b, &JoinSource::WebDav(dav_input(&s))));
    b.join_verify(0, JoinSecret::Passphrase(PW), false).unwrap();
    let dir = tmp.path().join("new");
    b.join_confirm(&dir).unwrap();
    match first_sync(&mut b) {
        SyncEvent::Finished(f) if f.ok => {}
        other => panic!("{other:?} {:?}", b.sync_status().problem),
    }
    assert_eq!(bodies(&mut b), want);
    assert!(dir.join(".nuvoletta/device-state/webdav").exists());
    assert!(b.webdav_summary().is_some());
    // The password is in the sealed file only.
    let sealed = fs::read(dir.join(".nuvoletta/device-state/webdav")).unwrap();
    assert!(!sealed.windows(PASS.len()).any(|w| w == PASS.as_bytes()));
}

#[test]
fn joining_over_webdav_refuses_an_account_that_can_delete_and_reads_nothing() {
    let s = TestServer::start(Policy::full());
    let mut b = new_app();
    match search(&mut b, &JoinSource::WebDav(dav_input(&s))) {
        JoinSearchEvent::Refused { title, .. } => assert!(title.contains("not used")),
        other => panic!("{other:?}"),
    }
    assert!(b.join_verify(0, JoinSecret::Passphrase(PW), false).is_err());
}
