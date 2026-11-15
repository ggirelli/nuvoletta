//! Mobile lifecycle: leaving the foreground saves and locks, and never discards edits.

use nuvoletta_app_core::{App, BackgroundOutcome, ManualClock, Phase, Settings};
use nuvoletta_crypto::KdfParams;
use std::sync::Arc;

const PW: &str = "correct horse battery";

fn app(dir: &std::path::Path) -> App {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    app.set_kdf_params(KdfParams::MIN_ACCEPTED);
    app.create_vault(dir, PW).unwrap();
    app
}

#[test]
fn background_saves_unsaved_edits_then_locks() {
    let d = tempfile::tempdir().unwrap();
    let mut app = app(d.path());
    let id = app.create_note("").unwrap();
    app.edit(id, "typed just before switching apps").unwrap();
    assert!(app.open_info().unwrap().dirty);

    assert_eq!(app.on_background(), BackgroundOutcome::Locked);
    assert_eq!(app.phase(), Phase::Locked);
    assert!(app.buffer().is_none());

    // The edit reached the disk: it is there after unlocking again.
    app.unlock_with_passphrase(PW).unwrap();
    app.open_note(id).unwrap();
    assert_eq!(app.buffer(), Some("typed just before switching apps"));
}

#[test]
fn background_with_nothing_unlocked_does_nothing() {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    assert_eq!(app.on_background(), BackgroundOutcome::NotUnlocked);
    let d = tempfile::tempdir().unwrap();
    let mut app = self::app(d.path());
    app.lock(false).unwrap();
    assert_eq!(app.on_background(), BackgroundOutcome::NotUnlocked);
    assert_eq!(app.phase(), Phase::Locked);
}

#[cfg(unix)]
#[test]
fn background_that_cannot_save_keeps_the_vault_unlocked_and_the_edit() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let mut app = app(d.path());
    let id = app.create_note("").unwrap();
    app.edit(id, "precious").unwrap();
    let notes = d.path().join("notes");
    std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(0o555)).unwrap();
    let restore = || std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(0o755));
    if std::fs::write(notes.join("probe"), b"x").is_ok() {
        restore().unwrap();
        return; // running as root: permission bits are ignored
    }
    let outcome = app.on_background();
    restore().unwrap();
    assert!(matches!(outcome, BackgroundOutcome::LockBlocked(_)));
    assert_eq!(app.phase(), Phase::Unlocked);
    assert_eq!(app.buffer(), Some("precious"));
}
