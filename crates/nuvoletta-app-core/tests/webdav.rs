//! The WebDAV set-up flow of the application core against the in-process test server: the
//! account check (accept read+create only, refuse the rest), certificate pinning with explicit
//! confirmation, sealed storage of the credentials, syncing between two devices, and what
//! changes when the permissions change later.

use nuvoletta_app_core::{
    App, Error, ManualClock, Settings, SyncBackend, SyncEvent, SyncState, WEBDAV_NO_CLEANUP,
    WebDavEvent, WebDavInput,
};
use nuvoletta_crypto::KdfParams;
use nuvoletta_webdav::testing::{PASS, Policy, TestServer, USER};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const PW: &str = "correct horse battery";

struct Dev {
    app: App,
    dir: PathBuf,
}

fn app() -> App {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    app.set_kdf_params(KdfParams::MIN_ACCEPTED);
    app
}

fn first(tmp: &Path, name: &str) -> Dev {
    let dir = tmp.join(name);
    let mut a = app();
    a.create_vault(&dir, PW).unwrap();
    Dev { app: a, dir }
}

fn join(tmp: &Path, name: &str, of: &Dev) -> Dev {
    let dir = tmp.join(name);
    fs::create_dir_all(dir.join(".nuvoletta")).unwrap();
    fs::copy(
        of.dir.join(".nuvoletta/vault-header"),
        dir.join(".nuvoletta/vault-header"),
    )
    .unwrap();
    let mut a = app();
    a.open_vault(&dir).unwrap();
    a.unlock_with_passphrase(PW).unwrap();
    Dev { app: a, dir }
}

fn input(s: &TestServer, pin: bool) -> WebDavInput {
    WebDavInput {
        url: s.url(),
        username: USER.to_owned(),
        password: Zeroizing::new(PASS.to_owned()),
        pin: pin.then(|| nuvoletta_webdav::format_fingerprint(&s.fingerprint)),
    }
}

fn wait_check(app: &mut App) -> WebDavEvent {
    let start = Instant::now();
    loop {
        if let Some(e) = app.poll_webdav() {
            return e;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "check timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Run the whole set-up: first without a pin (the certificate is self-signed), then pinned.
fn set_up(app: &mut App, s: &TestServer) -> WebDavEvent {
    app.webdav_check_start(&input(s, false)).unwrap();
    match wait_check(app) {
        WebDavEvent::NeedsPin { fingerprint, .. } => {
            assert_eq!(
                fingerprint,
                nuvoletta_webdav::format_fingerprint(&s.fingerprint)
            );
        }
        other => panic!("expected the pin prompt: {other:?}"),
    }
    assert!(
        app.webdav_summary().is_none(),
        "nothing saved before confirmation"
    );
    app.webdav_check_start(&input(s, true)).unwrap();
    wait_check(app)
}

fn device_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir.join(".nuvoletta/device-state"))
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn sync(d: &mut Dev) -> Option<SyncEvent> {
    d.app.sync_now().unwrap();
    d.app.wait_sync()
}

#[test]
fn a_create_only_account_is_saved_sealed_and_survives_a_lock() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = first(tmp.path(), "a");
    assert_eq!(set_up(&mut a.app, &s), WebDavEvent::Saved);

    let st = a.app.sync_status();
    assert_eq!(st.backend, SyncBackend::WebDav);
    assert!(a.app.sync_settings().active());
    let sum = a.app.webdav_summary().unwrap();
    assert_eq!(sum.url, s.url());
    assert_eq!(sum.username, USER);
    assert!(sum.pinned_fingerprint.is_some());

    // The credentials are sealed: neither the address, the user name nor the password is in any
    // device-state file in the clear.
    let state = a.dir.join(".nuvoletta/device-state");
    assert!(device_files(&a.dir).contains(&"webdav".to_owned()));
    for f in device_files(&a.dir) {
        let bytes = fs::read(state.join(&f)).unwrap();
        for secret in [PASS, USER, "127.0.0.1"] {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{secret} in {f}"
            );
        }
    }

    // Locked: unavailable. Unlocked again: restored.
    a.app.lock(false).unwrap();
    assert!(a.app.webdav_summary().is_none());
    a.app.unlock_with_passphrase(PW).unwrap();
    assert_eq!(a.app.webdav_summary().unwrap(), sum);
    assert!(a.app.sync_settings().active());
    let id = a.app.create_note("").unwrap();
    a.app.edit(id, "over webdav").unwrap();
    a.app.save_now().unwrap();
    assert!(matches!(sync(&mut a), Some(SyncEvent::Finished(f)) if f.ok));
    assert!(a.app.sync_status().problem.is_none());

    // Forgetting removes the file and turns sync off; nothing on the server is touched.
    let before = s.file_names();
    a.app.webdav_forget().unwrap();
    assert!(!device_files(&a.dir).contains(&"webdav".to_owned()));
    assert!(a.app.webdav_summary().is_none());
    assert_eq!(a.app.sync_status().state, SyncState::Disabled);
    assert_eq!(s.file_names(), before);
}

#[test]
fn credentials_are_not_readable_by_another_vault() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = first(tmp.path(), "a");
    assert_eq!(set_up(&mut a.app, &s), WebDavEvent::Saved);
    // Another vault (another key) finds a copy of the file unreadable and says so.
    let mut b = first(tmp.path(), "b");
    fs::create_dir_all(b.dir.join(".nuvoletta/device-state")).unwrap();
    fs::copy(
        a.dir.join(".nuvoletta/device-state/webdav"),
        b.dir.join(".nuvoletta/device-state/webdav"),
    )
    .unwrap();
    fs::write(
        b.dir.join(".nuvoletta/device-state/sync.json"),
        r#"{"enabled":true,"backend":"webdav","webdav":true}"#,
    )
    .unwrap();
    b.app.lock(false).unwrap();
    b.app.unlock_with_passphrase(PW).unwrap();
    assert!(b.app.webdav_summary().is_none());
    assert!(b.app.sync_status().problem.is_some());
}

#[test]
fn accounts_that_can_delete_or_overwrite_are_refused_and_nothing_is_saved() {
    for policy in [
        Policy {
            all_delete: true,
            ..Policy::create_only()
        },
        Policy {
            allow_overwrite: true,
            ..Policy::create_only()
        },
        Policy::full(),
    ] {
        let s = TestServer::start(policy);
        let tmp = tempfile::tempdir().unwrap();
        let mut a = first(tmp.path(), "a");
        match set_up(&mut a.app, &s) {
            WebDavEvent::Refused { title, explanation } => {
                assert!(title.contains("not used"), "{title}");
                assert!(explanation.contains("wipe out"), "{explanation}");
                assert!(explanation.contains("read and create"), "{explanation}");
                assert!(explanation.contains("WEBDAV.md"), "{explanation}");
            }
            other => panic!("{other:?}"),
        }
        assert!(a.app.webdav_summary().is_none());
        assert!(!device_files(&a.dir).contains(&"webdav".to_owned()));
        assert!(!a.app.sync_settings().active());
        assert!(matches!(a.app.sync_now(), Err(Error::SyncNotConfigured)));
    }
}

#[test]
fn bad_input_is_explained_and_never_reaches_the_network() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = first(tmp.path(), "a");
    let mut i = input(&s, true);
    i.url = i.url.replace("https://", "http://");
    match a.app.webdav_check_start(&i) {
        Err(Error::Input(m)) => assert!(m.contains("https://"), "{m}"),
        other => panic!("{other:?}"),
    }
    let mut i = input(&s, true);
    i.pin = Some("not a fingerprint".to_owned());
    assert!(matches!(a.app.webdav_check_start(&i), Err(Error::Input(_))));
    let mut i = input(&s, true);
    i.password = Zeroizing::new(String::new());
    assert!(matches!(a.app.webdav_check_start(&i), Err(Error::Input(_))));
    assert!(s.methods().is_empty());
}

#[test]
fn a_wrong_password_and_a_wrong_pin_are_reported() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = first(tmp.path(), "a");
    let mut i = input(&s, true);
    i.password = Zeroizing::new("wrong".to_owned());
    a.app.webdav_check_start(&i).unwrap();
    match wait_check(&mut a.app) {
        WebDavEvent::Failed { message, .. } => assert!(message.contains("rejected"), "{message}"),
        other => panic!("{other:?}"),
    }
    let mut i = input(&s, true);
    i.pin = Some(nuvoletta_webdav::format_fingerprint(&[9; 32]));
    a.app.webdav_check_start(&i).unwrap();
    match wait_check(&mut a.app) {
        WebDavEvent::Failed { message, .. } => {
            assert!(message.contains("different certificate"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert!(a.app.webdav_summary().is_none());
}

#[test]
fn two_devices_sync_through_webdav_and_cleanup_is_unavailable() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = first(tmp.path(), "a");
    assert_eq!(set_up(&mut a.app, &s), WebDavEvent::Saved);
    let mut b = join(tmp.path(), "b", &a);
    assert_eq!(set_up(&mut b.app, &s), WebDavEvent::Saved);

    let id = a.app.create_note("").unwrap();
    a.app.rename_note(id, "Plan").unwrap();
    a.app.edit(id, "pineapple").unwrap();
    a.app.save_now().unwrap();
    assert!(matches!(sync(&mut a), Some(SyncEvent::Finished(f)) if f.ok));
    assert!(matches!(sync(&mut b), Some(SyncEvent::Finished(f)) if f.ok));
    b.app.open_note(id).unwrap();
    assert_eq!(b.app.buffer().unwrap(), "pineapple");

    let p = a.app.cleanup_preview().unwrap();
    assert_eq!(p.unavailable.as_deref(), Some(WEBDAV_NO_CLEANUP));
    assert_eq!(a.app.cleanup_run().unwrap().old_versions, 0);
    s.with(|st| {
        for (m, path) in &st.log {
            assert!(
                matches!(m.as_str(), "PROPFIND" | "GET" | "PUT" | "MKCOL")
                    || path.contains("probe"),
                "{m} {path}"
            );
        }
    });
}

#[test]
fn a_permission_change_after_set_up_stops_sync_in_the_next_session() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = first(tmp.path(), "a");
    assert_eq!(set_up(&mut a.app, &s), WebDavEvent::Saved);
    assert!(matches!(sync(&mut a), Some(SyncEvent::Finished(f)) if f.ok));

    // The server's permissions change (someone widened the share). The next session's first
    // cycle re-checks and refuses.
    s.set_policy(Policy::full());
    a.app.lock(false).unwrap();
    a.app.unlock_with_passphrase(PW).unwrap();
    let id = a.app.create_note("").unwrap();
    a.app.edit(id, "after the change").unwrap();
    a.app.save_now().unwrap();
    let before = s.file_names();
    assert!(matches!(sync(&mut a), Some(SyncEvent::Finished(f)) if !f.ok));
    let p = a.app.sync_status().problem.unwrap();
    assert!(p.message.contains("can change or delete"), "{p:?}");
    assert!(p.hint.contains("read and create"), "{p:?}");
    // Nothing of Nuvoletta's was written (only probe files, which are not protocol files).
    let protocol = |names: Vec<String>| -> Vec<String> {
        names.into_iter().filter(|n| !n.contains("probe")).collect()
    };
    assert_eq!(protocol(s.file_names()), protocol(before));
}
