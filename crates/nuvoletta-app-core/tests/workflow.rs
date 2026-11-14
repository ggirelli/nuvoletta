//! End-to-end workflows of the application core, driven by a manual clock (no sleeping).

use nuvoletta_app_core::{
    App, DEFAULT_AUTO_LOCK_MINUTES, ManualClock, Phase, Settings, TickOutcome, TreeRow,
};
use nuvoletta_core::NoteId;
use nuvoletta_crypto::KdfParams;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const PW: &str = "correct horse battery";
const SEC: Duration = Duration::from_secs(1);

fn app() -> (App, ManualClock) {
    let clock = ManualClock::new();
    let mut app = App::new(Arc::new(clock.clone()), Settings::default());
    app.set_kdf_params(KdfParams::MIN_ACCEPTED);
    (app, clock)
}

fn new_vault(dir: &Path) -> (App, ManualClock, String) {
    let (mut app, clock) = app();
    let key = app.create_vault(dir, PW).unwrap();
    (app, clock, key.to_string())
}

/// Create a note named `name`, with `body`, and leave it open and saved.
fn write(app: &mut App, name: &str, body: &str) -> NoteId {
    let id = app.create_note("").unwrap();
    app.rename_note(id, name).unwrap();
    app.edit(id, body).unwrap();
    app.save_now().unwrap();
    id
}

#[test]
fn create_edit_link_find_preview_lock_unlock_verify() {
    let d = tempfile::tempdir().unwrap();
    let vault = d.path().join("v");
    let (mut app, _clock, _key) = new_vault(&vault);
    assert_eq!(app.phase(), Phase::Unlocked);

    let ideas = write(
        &mut app,
        "Ideas",
        "# Ideas\n\nSee [[Plan]] and [[Missing]]. #x\n",
    );
    let plan = write(&mut app, "Plan", "The plan mentions pineapple.\n");
    app.set_tags(plan, vec!["work".into()]).unwrap();

    // Links, backlinks, unresolved.
    assert_eq!(app.backlinks(plan).len(), 1);
    assert_eq!(app.backlinks(plan)[0].id, ideas);
    let unresolved = app.unresolved();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].target, "Missing");
    assert_eq!(unresolved[0].source.id, ideas);

    // Search and tags.
    let hits = app.search("pineapple", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].note.id, plan);
    assert_eq!(app.tags().len(), 1);
    assert_eq!(app.notes_with_tag("work")[0].id, plan);

    // Preview resolves wikilinks through the index and escapes raw HTML.
    app.open_note(ideas).unwrap();
    app.edit(
        ideas,
        "[[Plan]] <script>alert(1)</script> ![x](https://evil.example/a.png)",
    )
    .unwrap();
    let html = app.preview_html();
    assert!(html.contains(&format!("nuvoletta://note/{plan}")), "{html}");
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    app.save_now().unwrap();

    // Lock drops everything; unlock restores it from disk.
    app.lock(false).unwrap();
    assert_eq!(app.phase(), Phase::Locked);
    assert!(app.buffer().is_none());
    assert!(app.open_info().is_none());
    assert!(app.search("pineapple", 10).is_empty());
    assert!(app.notes().is_empty());
    assert!(app.preview_html().is_empty());

    assert!(app.unlock_with_passphrase("wrong wrong wrong").is_err());
    assert_eq!(app.phase(), Phase::Locked);
    app.unlock_with_passphrase(PW).unwrap();
    assert_eq!(app.notes().len(), 2);
    assert_eq!(app.search("pineapple", 10).len(), 1);
    assert_eq!(app.backlinks(plan)[0].id, ideas);
    app.open_note(ideas).unwrap();
    assert!(app.buffer().unwrap().contains("<script>"));
    assert!(app.settings().last_vault.as_deref() == Some(vault.as_path()));
}

#[test]
fn a_fresh_app_can_open_and_unlock_an_existing_vault() {
    let d = tempfile::tempdir().unwrap();
    let (mut a, _c, _k) = new_vault(d.path());
    write(&mut a, "Hello", "world");
    drop(a);
    let (mut b, _c) = app();
    assert_eq!(b.phase(), Phase::NoVault);
    b.open_vault(d.path()).unwrap();
    assert_eq!(b.phase(), Phase::Locked);
    b.unlock_with_passphrase(PW).unwrap();
    assert_eq!(b.search("world", 5).len(), 1);
    let (mut c, _c) = app();
    assert!(c.open_vault(&d.path().join("nothing")).is_err());
    assert_eq!(c.phase(), Phase::NoVault);
}

#[test]
fn vault_directory_never_contains_plaintext() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    write(&mut app, "TitleNeedle", "BodyNeedle");
    app.set_tags(app.open_info().unwrap().id, vec!["TagNeedle".into()])
        .unwrap();
    app.lock(false).unwrap();
    fn walk(p: &Path, out: &mut Vec<Vec<u8>>) {
        for e in std::fs::read_dir(p).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(std::fs::read(&p).unwrap());
            }
        }
    }
    let mut files = Vec::new();
    walk(d.path(), &mut files);
    assert!(!files.is_empty());
    for f in files {
        for needle in ["TitleNeedle", "BodyNeedle", "TagNeedle"] {
            assert!(!f.windows(needle.len()).any(|w| w == needle.as_bytes()));
        }
    }
}

#[test]
fn autosave_waits_for_the_debounce_and_resets_on_each_edit() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, clock, _k) = new_vault(d.path());
    let id = app.create_note("").unwrap();
    assert!(!app.open_info().unwrap().dirty);

    app.edit(id, "a").unwrap();
    assert!(app.open_info().unwrap().dirty);
    clock.advance(Duration::from_millis(600));
    assert_eq!(app.tick(), TickOutcome::Idle);
    app.edit(id, "ab").unwrap(); // restarts the debounce
    clock.advance(Duration::from_millis(600));
    assert_eq!(app.tick(), TickOutcome::Idle);
    clock.advance(Duration::from_millis(500));
    assert_eq!(app.tick(), TickOutcome::Saved);
    assert!(!app.open_info().unwrap().dirty);
    assert_eq!(app.tick(), TickOutcome::Idle);

    // The saved text is what a reopened vault shows.
    app.lock(false).unwrap();
    app.unlock_with_passphrase(PW).unwrap();
    app.open_note(id).unwrap();
    assert_eq!(app.buffer(), Some("ab"));
}

#[test]
fn undoing_back_to_the_stored_text_is_not_dirty() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let id = write(&mut app, "N", "one");
    app.edit(id, "two").unwrap();
    assert!(app.open_info().unwrap().dirty);
    app.edit(id, "one").unwrap();
    assert!(!app.open_info().unwrap().dirty);
}

#[test]
fn stale_editor_messages_for_another_note_are_ignored() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let a = write(&mut app, "A", "alpha");
    let b = write(&mut app, "B", "beta");
    app.open_note(b).unwrap();
    assert!(app.edit(a, "late message").is_err());
    assert_eq!(app.buffer(), Some("beta"));
}

#[test]
fn navigating_saves_the_current_note_first() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let a = write(&mut app, "A", "alpha");
    let b = write(&mut app, "B", "beta");
    app.open_note(a).unwrap();
    app.edit(a, "alpha edited").unwrap();
    app.open_note(b).unwrap(); // flushes A
    app.open_note(a).unwrap();
    assert_eq!(app.buffer(), Some("alpha edited"));
    assert_eq!(app.search("edited", 5).len(), 1); // index follows the save
}

#[cfg(unix)]
mod failing_saves {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Make the notes directory read-only so that writes fail; undo on drop.
    struct ReadOnly(std::path::PathBuf);
    impl ReadOnly {
        fn new(vault: &Path) -> Self {
            let p = vault.join("notes");
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o555)).unwrap();
            Self(p)
        }
    }
    impl Drop for ReadOnly {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    fn writable_probe(vault: &Path) -> bool {
        // Root ignores permission bits; skip the assertions there.
        std::fs::write(vault.join("notes/probe"), b"x").is_ok()
    }

    #[test]
    fn failed_save_keeps_the_buffer_blocks_navigation_and_lock_until_forced() {
        let d = tempfile::tempdir().unwrap();
        let (mut app, clock, _k) = new_vault(d.path());
        let a = write(&mut app, "A", "alpha");
        let b = write(&mut app, "B", "beta");
        app.open_note(a).unwrap();
        app.edit(a, "precious edit").unwrap();

        let _ro = ReadOnly::new(d.path());
        if writable_probe(d.path()) {
            return;
        }

        // Autosave fails, surfaces the error, keeps the buffer and retries later.
        clock.advance(2 * SEC);
        let TickOutcome::SaveFailed(msg) = app.tick() else {
            panic!("expected a failed save");
        };
        assert!(!msg.is_empty());
        let info = app.open_info().unwrap();
        assert!(info.dirty && info.save_error.is_some());
        assert_eq!(app.buffer(), Some("precious edit"));
        assert_eq!(app.tick(), TickOutcome::Idle); // not retried before the debounce

        // Navigating away and locking are refused, nothing is discarded.
        assert!(app.open_note(b).is_err());
        assert_eq!(app.open_info().unwrap().id, a);
        assert!(app.lock(false).is_err());
        assert_eq!(app.phase(), Phase::Unlocked);
        assert_eq!(app.buffer(), Some("precious edit"));

        // The auto-lock timer does not lock either; it reports.
        clock.advance(Duration::from_secs(
            u64::from(DEFAULT_AUTO_LOCK_MINUTES) * 60,
        ));
        assert!(matches!(app.tick(), TickOutcome::LockBlocked(_)));
        assert_eq!(app.phase(), Phase::Unlocked);

        // An explicit, forced lock is possible.
        app.lock(true).unwrap();
        assert_eq!(app.phase(), Phase::Locked);
        assert!(app.buffer().is_none());
    }

    #[test]
    fn save_succeeds_again_once_the_problem_is_gone() {
        let d = tempfile::tempdir().unwrap();
        let (mut app, _c, _k) = new_vault(d.path());
        let a = write(&mut app, "A", "alpha");
        app.edit(a, "changed").unwrap();
        {
            let _ro = ReadOnly::new(d.path());
            if writable_probe(d.path()) {
                return;
            }
            assert!(app.save_now().is_err());
        }
        app.save_now().unwrap();
        let info = app.open_info().unwrap();
        assert!(!info.dirty && info.save_error.is_none());
        app.lock(false).unwrap();
    }
}

#[test]
fn auto_lock_after_the_configured_inactivity_and_activity_resets_it() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, clock, _k) = new_vault(d.path());
    assert_eq!(app.settings().auto_lock_minutes, 5);
    let id = write(&mut app, "N", "text");

    clock.advance(Duration::from_secs(4 * 60));
    assert_eq!(app.tick(), TickOutcome::Idle);
    app.touch();
    clock.advance(Duration::from_secs(4 * 60));
    assert_eq!(app.tick(), TickOutcome::Idle);
    assert_eq!(app.idle_remaining(), Some(Duration::from_secs(60)));

    // Unsaved edit is flushed by the auto-lock, then everything is dropped.
    app.edit(id, "text, edited").unwrap();
    clock.advance(Duration::from_secs(4 * 60 + 59));
    clock.advance(SEC);
    assert_eq!(app.tick(), TickOutcome::Locked);
    assert_eq!(app.phase(), Phase::Locked);
    assert!(app.buffer().is_none());
    assert_eq!(app.tick(), TickOutcome::Idle);
    app.unlock_with_passphrase(PW).unwrap();
    app.open_note(id).unwrap();
    assert_eq!(app.buffer(), Some("text, edited"));
}

#[test]
fn auto_lock_period_is_configurable_and_clamped() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, clock, _k) = new_vault(d.path());
    app.set_auto_lock_minutes(1);
    clock.advance(Duration::from_secs(59));
    assert_eq!(app.tick(), TickOutcome::Idle);
    clock.advance(SEC);
    assert_eq!(app.tick(), TickOutcome::Locked);
    app.unlock_with_passphrase(PW).unwrap();
    app.set_auto_lock_minutes(0);
    assert_eq!(app.settings().auto_lock_minutes, 1);
    app.set_auto_lock_minutes(10_000_000);
    assert_eq!(app.settings().auto_lock_minutes, 24 * 60);
}

#[test]
fn manual_lock_flushes_unsaved_edits() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let id = write(&mut app, "N", "one");
    app.edit(id, "two").unwrap();
    app.lock(false).unwrap();
    app.unlock_with_passphrase(PW).unwrap();
    app.open_note(id).unwrap();
    assert_eq!(app.buffer(), Some("two"));
}

#[test]
fn rename_move_and_delete_keep_the_index_in_step() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let src = write(&mut app, "Source", "to [[Target]]");
    let target = write(&mut app, "Target", "t");
    assert_eq!(app.backlinks(target).len(), 1);
    assert!(app.unresolved().is_empty());

    app.rename_note(target, "Goal").unwrap();
    assert!(app.backlinks(target).is_empty());
    assert_eq!(app.unresolved().len(), 1);
    app.rename_note(target, "Target").unwrap();
    assert_eq!(app.backlinks(target).len(), 1);

    app.move_note(target, "projects/2026").unwrap();
    assert_eq!(
        app.notes().iter().find(|n| n.id == target).unwrap().path,
        "projects/2026/Target"
    );
    assert_eq!(app.backlinks(target).len(), 1);
    app.move_note(target, "").unwrap();
    assert_eq!(
        app.notes().iter().find(|n| n.id == target).unwrap().path,
        "Target"
    );

    // Renaming the open note keeps unsaved edits.
    app.open_note(src).unwrap();
    app.edit(src, "to [[Target]] and more").unwrap();
    app.rename_note(src, "Origin").unwrap();
    assert_eq!(app.buffer(), Some("to [[Target]] and more"));
    assert!(!app.open_info().unwrap().dirty);

    assert!(app.rename_note(src, "").is_err());
    assert!(app.rename_note(src, "a/b").is_err());

    app.delete_note(target).unwrap();
    assert_eq!(app.unresolved().len(), 1);
    assert!(app.search("Target", 5).iter().all(|h| h.note.id != target));
    app.delete_note(src).unwrap(); // open note: buffer dropped
    assert!(app.open_info().is_none());
    assert!(app.notes().is_empty());
}

#[test]
fn new_notes_get_unique_untitled_names_in_their_folder() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let a = app.create_note("").unwrap();
    let b = app.create_note("").unwrap();
    let c = app.create_note("work/").unwrap();
    let paths: Vec<String> = app.notes().into_iter().map(|n| n.path).collect();
    assert_eq!(paths, ["Untitled", "Untitled 2", "work/Untitled"]);
    assert!(a != b && b != c);
}

#[test]
fn tree_groups_notes_by_folder() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, _k) = new_vault(d.path());
    let top = write(&mut app, "Top", "");
    let deep = write(&mut app, "Deep", "");
    app.move_note(deep, "a/b").unwrap();
    let mid = write(&mut app, "Mid", "");
    app.move_note(mid, "a").unwrap();
    let shapes: Vec<String> = app
        .tree()
        .iter()
        .map(|r| match r {
            TreeRow::Folder { depth, path, .. } => format!("{depth}:folder {path}"),
            TreeRow::Note { depth, label, .. } => format!("{depth}:note {label}"),
        })
        .collect();
    assert_eq!(
        shapes,
        [
            "0:note Top",
            "0:folder a",
            "1:note Mid",
            "1:folder a/b",
            "2:note Deep"
        ]
    );
    let _ = top;
}

#[test]
fn recovery_key_unlock_requires_a_new_passphrase_which_replaces_the_old() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c, key) = new_vault(d.path());
    write(&mut app, "N", "kept");
    app.lock(false).unwrap();

    assert!(app.unlock_with_recovery_key("not a key").is_err());
    assert_eq!(app.phase(), Phase::Locked);
    app.unlock_with_recovery_key(&key).unwrap();
    assert!(app.must_change_passphrase());
    assert!(app.change_passphrase("short").is_err());
    assert!(app.must_change_passphrase());
    app.change_passphrase("a brand new passphrase").unwrap();
    assert!(!app.must_change_passphrase());
    app.lock(false).unwrap();

    assert!(app.unlock_with_passphrase(PW).is_err());
    app.unlock_with_passphrase("a brand new passphrase")
        .unwrap();
    assert_eq!(app.search("kept", 3).len(), 1);

    // Rotating the recovery key retires the old one.
    let new_key = app.rotate_recovery_key("a brand new passphrase").unwrap();
    assert_ne!(*new_key, key);
    app.lock(false).unwrap();
    assert!(app.unlock_with_recovery_key(&key).is_err());
    app.unlock_with_recovery_key(&new_key).unwrap();
}

#[test]
fn export_and_import_round_trip_into_another_vault() {
    let d = tempfile::tempdir().unwrap();
    let (mut a, _c, _k) = new_vault(&d.path().join("a"));
    let n = write(&mut a, "Note", "hello [[Other]] export");
    a.set_tags(n, vec!["t1".into()]).unwrap();
    write(&mut a, "Other", "second");
    a.open_note(n).unwrap();
    a.edit(n, "hello [[Other]] export, unsaved").unwrap();

    let out = d.path().join("out");
    // Inside the vault is refused.
    assert!(a.export(&d.path().join("a/out")).is_err());
    let report = a.export(&out).unwrap(); // flushes the unsaved edit first
    assert_eq!(report.exported.len(), 2);
    assert!(report.failed.is_empty());

    let (mut b, _c, _k) = new_vault(&d.path().join("b"));
    let imp = b.import(&out).unwrap();
    assert_eq!(imp.imported.len(), 2);
    assert_eq!(b.notes().len(), 2);
    assert_eq!(b.search("unsaved", 5).len(), 1);
    assert_eq!(b.tags()[0].tag, "t1");
    assert!(b.unresolved().is_empty());
    // Importing the same export again is harmless.
    let again = b.import(&out).unwrap();
    assert!(again.imported.is_empty());
    assert_eq!(b.notes().len(), 2);
}

#[test]
fn operations_in_the_wrong_phase_are_errors_not_panics() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c) = app();
    assert!(app.create_note("").is_err());
    assert!(app.unlock_with_passphrase(PW).is_err());
    assert!(app.lock(false).is_err());
    app.create_vault(d.path(), PW).unwrap();
    assert!(app.create_vault(&d.path().join("x"), PW).is_err());
    assert!(app.open_vault(d.path()).is_err());
    app.lock(false).unwrap();
    assert!(app.create_note("").is_err());
    assert!(app.save_now().is_err());
    app.close_vault(false).unwrap();
    assert_eq!(app.phase(), Phase::NoVault);
    assert!(app.vault_root().is_none());
}

#[test]
fn short_passphrase_is_rejected_and_wrong_passphrase_message_is_friendly() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _c) = app();
    assert!(app.create_vault(&d.path().join("v"), "short").is_err());
    assert_eq!(app.phase(), Phase::NoVault);
    app.create_vault(&d.path().join("w"), PW).unwrap();
    app.lock(false).unwrap();
    let e = app
        .unlock_with_passphrase("nope nope nope nope")
        .unwrap_err();
    assert!(e.user_message().contains("Wrong passphrase"));
}
