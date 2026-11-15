//! Sync integration of the application core: settings, status transitions, locking during a
//! cycle, scheduling, conflicts, notices and diagnostics, with two devices over a temp folder.

use nuvoletta_app_core::{
    App, ConflictAction, ConflictKind, Error, ManualClock, Phase, Settings, SyncEvent, SyncState,
    SyncTiming, TickOutcome,
};
use nuvoletta_core::NoteId;
use nuvoletta_crypto::KdfParams;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const PW: &str = "correct horse battery";
const SEC: Duration = Duration::from_secs(1);

struct Dev {
    app: App,
    clock: ManualClock,
    dir: PathBuf,
}

struct World {
    tmp: tempfile::TempDir,
    shared: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        fs::create_dir_all(&shared).unwrap();
        Self { tmp, shared }
    }

    fn app() -> (App, ManualClock) {
        let clock = ManualClock::new();
        let mut app = App::new(Arc::new(clock.clone()), Settings::default());
        app.set_kdf_params(KdfParams::MIN_ACCEPTED);
        (app, clock)
    }

    /// A device that created the vault, with sync on.
    fn first(&self, name: &str) -> Dev {
        let dir = self.tmp.path().join(name);
        let (mut app, clock) = Self::app();
        app.create_vault(&dir, PW).unwrap();
        app.configure_sync(&self.shared).unwrap();
        Dev { app, clock, dir }
    }

    /// Another device of the same vault (same header, no notes), with sync on.
    fn join(&self, name: &str, of: &Dev) -> Dev {
        let dir = self.tmp.path().join(name);
        fs::create_dir_all(dir.join(".nuvoletta")).unwrap();
        fs::copy(
            of.dir.join(".nuvoletta/vault-header"),
            dir.join(".nuvoletta/vault-header"),
        )
        .unwrap();
        let (mut app, clock) = Self::app();
        app.open_vault(&dir).unwrap();
        app.unlock_with_passphrase(PW).unwrap();
        app.configure_sync(&self.shared).unwrap();
        Dev { app, clock, dir }
    }
}

fn finished_ok(ev: Option<SyncEvent>) -> bool {
    matches!(ev, Some(SyncEvent::Finished(f)) if f.ok)
}

impl Dev {
    fn sync(&mut self) -> Option<SyncEvent> {
        self.app.sync_now().unwrap();
        self.app.wait_sync()
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

    fn body(&mut self, id: NoteId) -> String {
        self.app.open_note(id).unwrap();
        self.app.buffer().unwrap().to_owned()
    }
}

fn sync_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir.join(".nuvoletta/device-state"))
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn settings_are_stored_per_vault_on_this_device_and_can_be_disabled() {
    let w = World::new();
    let mut a = w.first("a");
    let s = a.app.sync_settings().clone();
    assert!(s.enabled && s.folder.is_some());
    assert!(sync_files(&a.dir).contains(&"sync.json".to_owned()));

    // Reopening the vault restores the setting.
    a.app.lock(false).unwrap();
    let (mut again, _c) = World::app();
    again.open_vault(&a.dir).unwrap();
    again.unlock_with_passphrase(PW).unwrap();
    assert_eq!(again.sync_settings(), &s);

    // Disabling keeps the folder, stops syncing and is persisted.
    again.set_sync_enabled(false).unwrap();
    assert_eq!(again.sync_status().state, SyncState::Disabled);
    assert!(matches!(again.sync_now(), Err(Error::SyncNotConfigured)));
    again.lock(false).unwrap();
    again.unlock_with_passphrase(PW).unwrap();
    assert!(!again.sync_settings().enabled);
    assert_eq!(again.sync_settings().folder, s.folder);
    again.set_sync_enabled(true).unwrap();
    assert!(again.sync_settings().active());

    // The setting never travels with synced data: only protocol files are in the folder.
    for e in fs::read_dir(&w.shared).unwrap() {
        let n = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!n.contains("sync.json"), "{n}");
    }
}

#[test]
fn an_unusable_folder_is_refused_and_enabling_needs_a_folder() {
    let w = World::new();
    let (mut app, _c) = World::app();
    let dir = w.tmp.path().join("v");
    app.create_vault(&dir, PW).unwrap();
    assert!(matches!(
        app.set_sync_enabled(true),
        Err(Error::SyncNotConfigured)
    ));
    assert!(app.configure_sync(&w.tmp.path().join("missing")).is_err());
    assert!(app.configure_sync(&dir).is_err(), "the vault itself");
    assert!(
        app.configure_sync(&dir.join("notes")).is_err(),
        "inside the vault"
    );
    assert!(!app.sync_settings().active());
}

#[test]
fn status_goes_idle_syncing_idle_and_notes_travel_between_devices() {
    let w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    assert_eq!(a.app.sync_status().state, SyncState::Idle);
    assert_eq!(a.app.sync_status().last_synced_ms, None);

    let id = a.write("Plan", "pineapple");
    a.app.sync_now().unwrap();
    // Still "syncing" until the result is taken, however fast the worker is.
    assert_eq!(a.app.sync_status().state, SyncState::Syncing);
    assert!(a.app.sync_in_flight());
    // The note list keeps working while the vault is away.
    assert_eq!(a.app.notes().len(), 1);
    assert!(finished_ok(a.app.wait_sync()));
    let st = a.app.sync_status();
    assert_eq!(st.state, SyncState::Idle);
    assert!(st.last_synced_ms.is_some());
    assert_eq!(st.pending_objects, 0);
    assert!(st.problem.is_none());

    assert!(finished_ok(b.sync()));
    assert_eq!(b.body(id), "pineapple");
    assert_eq!(b.app.search("pineapple", 5).len(), 1);
}

#[test]
fn a_second_start_while_running_is_a_no_op_and_writes_wait() {
    let w = World::new();
    let mut a = w.first("a");
    let id = a.write("N", "x");
    a.app.sync_now().unwrap();
    a.app.sync_now().unwrap();
    assert!(matches!(a.app.save_now(), Ok(()) | Err(Error::SyncBusy)));
    assert!(matches!(a.app.create_note(""), Err(Error::SyncBusy)));
    assert!(matches!(a.app.delete_note(id), Err(Error::SyncBusy)));
    assert!(a.app.wait_sync().is_some());
    assert!(a.app.wait_sync().is_none());
    a.app.create_note("").unwrap();
}

#[test]
fn nothing_syncs_while_locked() {
    let w = World::new();
    let mut a = w.first("a");
    a.write("N", "x");
    a.app.lock(false).unwrap();
    assert!(matches!(a.app.sync_now(), Err(Error::Locked)));
    a.clock.advance(Duration::from_secs(3600));
    assert_eq!(a.app.poll_sync(), None);
    assert!(a.app.wait_sync().is_none());
    assert_eq!(a.app.sync_status().state, SyncState::Disabled);
    // Nothing was uploaded.
    assert_eq!(fs::read_dir(&w.shared).unwrap().count(), 0);
}

#[test]
fn locking_during_a_cycle_waits_and_loses_no_edits() {
    let w = World::new();
    let mut a = w.first("a");
    let id = a.write("N", "saved");
    a.app.sync_now().unwrap();
    a.app.edit(id, "typed during the cycle").unwrap();
    a.app.lock(false).unwrap();
    assert_eq!(a.app.phase(), Phase::Unlocked, "locks when the cycle ends");
    assert!(a.app.lock_pending());
    assert!(a.app.sync_status().lock_pending);
    // The idle timer does not pile up more requests.
    assert_eq!(a.app.tick(), TickOutcome::Idle);
    assert_eq!(a.app.wait_sync(), Some(SyncEvent::Locked));
    assert_eq!(a.app.phase(), Phase::Locked);
    a.app.unlock_with_passphrase(PW).unwrap();
    assert_eq!(a.body(id), "typed during the cycle");
}

#[test]
fn inactivity_during_a_cycle_reports_a_pending_lock() {
    let w = World::new();
    let mut a = w.first("a");
    a.write("N", "x");
    a.app.sync_now().unwrap();
    a.clock.advance(Duration::from_secs(6 * 60));
    assert_eq!(a.app.tick(), TickOutcome::LockPending);
    assert_eq!(a.app.tick(), TickOutcome::Idle);
    assert_eq!(a.app.wait_sync(), Some(SyncEvent::Locked));
    assert_eq!(a.app.phase(), Phase::Locked);
}

#[test]
fn closing_the_vault_waits_for_the_cycle_and_saves_the_buffer() {
    let w = World::new();
    let mut a = w.first("a");
    let id = a.write("N", "one");
    a.app.sync_now().unwrap();
    a.app.edit(id, "two").unwrap();
    a.app.close_vault(false).unwrap();
    assert_eq!(a.app.phase(), Phase::NoVault);
    let (mut again, _c) = World::app();
    again.open_vault(&a.dir).unwrap();
    again.unlock_with_passphrase(PW).unwrap();
    again.open_note(id).unwrap();
    assert_eq!(again.buffer(), Some("two"));
}

#[test]
fn automatic_sync_follows_saves_after_a_debounce_and_runs_periodically() {
    let w = World::new();
    let mut a = w.first("a");
    a.app.set_sync_timing(SyncTiming {
        debounce: 5 * SEC,
        interval: 60 * SEC,
    });
    // First tick after unlock syncs once.
    assert_eq!(a.app.tick(), TickOutcome::Sync(SyncEvent::Started));
    assert!(finished_ok(a.app.wait_sync()));
    a.clock.advance(SEC);
    assert_eq!(a.app.tick(), TickOutcome::Idle);

    // An edit is autosaved, then synced 5 s after the save.
    let id = a.app.create_note("").unwrap();
    a.app.edit(id, "hello").unwrap();
    a.clock.advance(2 * SEC);
    assert_eq!(a.app.tick(), TickOutcome::Saved);
    a.clock.advance(4 * SEC);
    assert_eq!(a.app.tick(), TickOutcome::Idle);
    a.clock.advance(SEC);
    assert_eq!(a.app.tick(), TickOutcome::Sync(SyncEvent::Started));
    assert!(finished_ok(a.app.wait_sync()));
    assert_eq!(a.app.sync_status().pending_objects, 0);

    // Nothing to do until the interval has passed.
    a.clock.advance(30 * SEC);
    assert_eq!(a.app.tick(), TickOutcome::Idle);
    a.clock.advance(31 * SEC);
    a.app.touch();
    assert_eq!(a.app.tick(), TickOutcome::Sync(SyncEvent::Started));
    a.app.wait_sync();
}

#[test]
fn automatic_sync_waits_while_the_note_is_being_typed() {
    let w = World::new();
    let mut a = w.first("a");
    a.app.set_sync_timing(SyncTiming {
        debounce: SEC,
        interval: 10 * SEC,
    });
    assert_eq!(a.app.tick(), TickOutcome::Sync(SyncEvent::Started));
    a.app.wait_sync();
    let id = a.app.create_note("").unwrap();
    a.clock.advance(20 * SEC);
    a.app.touch();
    a.app.edit(id, "typing").unwrap(); // dirty: autosave not yet due
    assert_eq!(
        a.app.tick(),
        TickOutcome::Idle,
        "no cycle over unsaved typing"
    );
}

#[test]
fn a_missing_folder_is_reported_not_created() {
    let w = World::new();
    let mut a = w.first("a");
    fs::remove_dir_all(&w.shared).unwrap();
    let e = a.app.sync_now().unwrap_err();
    assert!(e.user_message().contains("missing"), "{}", e.user_message());
    assert!(
        !w.shared.exists(),
        "an unmounted drive must not be recreated"
    );
    let p = a.app.sync_status().problem.unwrap();
    assert!(p.message.contains("missing") && !p.hint.is_empty());
    assert_eq!(a.app.sync_status().state, SyncState::Idle);
    // Reconnecting clears the problem on the next successful cycle.
    fs::create_dir_all(&w.shared).unwrap();
    assert!(finished_ok(a.sync()));
    assert!(a.app.sync_status().problem.is_none());
}

fn make_conflict(w: &World) -> (Dev, Dev, NoteId) {
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.write("Shared", "line one\nline two\nline three");
    a.sync();
    b.sync();
    a.set_body(id, "line one\nA changed this\nline three");
    b.set_body(id, "line one\nB changed this\nline three");
    a.sync();
    b.sync();
    (a, b, id)
}

#[test]
fn conflicting_edits_are_listed_with_heads_and_a_diff_and_nothing_is_lost() {
    let w = World::new();
    let (_a, b, id) = make_conflict(&w);
    assert_eq!(b.app.sync_status().conflicts, 1);
    let c = b.app.sync_conflicts();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].note, id);
    assert_eq!(c[0].kind, ConflictKind::Edits);
    assert_eq!(c[0].title, "Shared");
    assert_eq!(c[0].heads.len(), 2);
    assert_eq!(c[0].heads.iter().filter(|h| h.is_local).count(), 1);
    // Deterministic order: the same on every call.
    assert_eq!(c, b.app.sync_conflicts());

    // Each head can be viewed and diffed against the local version.
    for h in &c[0].heads {
        let d = b.app.conflict_head(id, &h.rev).unwrap();
        let body = d.body.unwrap();
        assert!(body.contains("changed this"));
        if h.is_local {
            assert!(d.diff.is_empty());
        } else {
            assert!(!d.diff.is_empty());
            assert!(body.contains("A changed"));
        }
    }
    // Unknown versions are refused.
    assert!(b.app.conflict_head(id, &"0".repeat(32)).is_err());
}

#[test]
fn choosing_a_head_resolves_the_conflict_on_every_device() {
    let w = World::new();
    let (mut a, mut b, id) = make_conflict(&w);
    let remote_head = b
        .app
        .sync_conflicts()
        .remove(0)
        .heads
        .into_iter()
        .find(|h| !h.is_local)
        .unwrap();
    b.app.open_note(id).unwrap();
    b.app
        .resolve_conflict(id, &ConflictAction::ChooseHead(remote_head.rev))
        .unwrap();
    assert!(b.app.sync_conflicts().is_empty());
    assert_eq!(b.body(id), "line one\nA changed this\nline three");
    b.sync();
    a.sync();
    assert!(a.app.sync_conflicts().is_empty());
    assert_eq!(a.body(id), "line one\nA changed this\nline three");
}

#[test]
fn keep_both_creates_a_conflict_copy_and_loses_no_text() {
    let w = World::new();
    let (mut a, mut b, id) = make_conflict(&w);
    let heads = b.app.sync_conflicts().remove(0).heads;
    let keep = heads.iter().find(|h| h.is_local).unwrap().rev.clone();
    let copy = heads.iter().find(|h| !h.is_local).unwrap().rev.clone();
    let out = b
        .app
        .resolve_conflict(id, &ConflictAction::KeepBoth { keep, copy })
        .unwrap();
    let copy_id = out.new_note.unwrap();
    assert!(b.body(id).contains("B changed"));
    assert!(b.body(copy_id).contains("A changed"));
    assert!(
        b.app
            .notes()
            .iter()
            .any(|n| n.title.ends_with("(conflict copy)"))
    );
    b.sync();
    a.sync();
    assert!(a.body(copy_id).contains("A changed"));
    assert!(a.app.sync_conflicts().is_empty());
}

#[test]
fn delete_versus_edit_offers_keep_and_delete() {
    for keep in [true, false] {
        let w = World::new();
        let mut a = w.first("a");
        let mut b = w.join("b", &a);
        let id = a.write("Doomed", "body");
        a.sync();
        b.sync();
        a.app.delete_note(id).unwrap();
        b.set_body(id, "edited");
        a.sync();
        b.sync();
        let c = b.app.sync_conflicts();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, ConflictKind::DeleteVsEdit);
        let del = c[0].heads.iter().find(|h| h.is_delete).unwrap();
        let d = b.app.conflict_head(id, &del.rev).unwrap();
        assert!(d.body.is_none() && d.title.is_none());
        let act = if keep {
            ConflictAction::Keep
        } else {
            ConflictAction::Delete
        };
        b.app.resolve_conflict(id, &act).unwrap();
        assert!(b.app.sync_conflicts().is_empty());
        b.sync();
        a.sync();
        assert_eq!(a.app.notes().iter().any(|n| n.id == id), keep);
        assert_eq!(b.app.notes().iter().any(|n| n.id == id), keep);
    }
}

#[test]
fn a_stale_or_invalid_action_is_an_explained_error_and_changes_nothing() {
    let w = World::new();
    let (_a, mut b, id) = make_conflict(&w);
    let err = b
        .app
        .resolve_conflict(id, &ConflictAction::ChooseHead("zz".into()))
        .unwrap_err();
    assert!(!err.user_message().is_empty());
    let err = b
        .app
        .resolve_conflict(id, &ConflictAction::ChooseHead("0".repeat(32)))
        .unwrap_err();
    assert!(
        err.user_message().contains("changed"),
        "{}",
        err.user_message()
    );
    assert_eq!(b.app.sync_conflicts().len(), 1);
    // "Keep" needs a deletion among the versions.
    assert!(b.app.resolve_conflict(id, &ConflictAction::Keep).is_err());
    assert_eq!(b.app.sync_conflicts().len(), 1);
}

#[test]
fn sync_that_updates_the_open_note_reloads_it_when_clean() {
    let w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.write("N", "v1");
    a.sync();
    b.sync();
    b.app.open_note(id).unwrap();
    let gen0 = b.app.open_info().unwrap().generation;
    a.set_body(id, "v2");
    a.sync();
    let Some(SyncEvent::Finished(f)) = b.sync() else {
        panic!("no result");
    };
    assert!(f.ok && f.open_note_changed);
    assert_eq!(b.app.buffer(), Some("v2"));
    assert!(b.app.open_info().unwrap().generation > gen0);
}

#[test]
fn typing_during_a_cycle_that_updates_the_open_note_is_kept_and_flagged() {
    let w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.write("N", "v1");
    a.sync();
    b.sync();
    b.app.open_note(id).unwrap();
    a.set_body(id, "v2");
    a.sync();
    b.app.sync_now().unwrap();
    b.app.edit(id, "typed meanwhile").unwrap();
    let Some(SyncEvent::Finished(f)) = b.app.wait_sync() else {
        panic!("no result");
    };
    assert!(!f.open_note_changed);
    assert!(f.message.unwrap().contains("updated by sync"));
    assert_eq!(b.app.buffer(), Some("typed meanwhile"));
    let info = b.app.open_info().unwrap();
    assert!(info.changed_by_sync && info.dirty);
    // Saving is an explicit act and clears the flag; the other text stays in sync history.
    b.app.save_now().unwrap();
    assert!(!b.app.open_info().unwrap().changed_by_sync);
}

#[test]
fn a_note_deleted_remotely_while_open_and_dirty_is_kept() {
    let w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.write("N", "v1");
    a.sync();
    b.sync();
    b.app.open_note(id).unwrap();
    a.app.delete_note(id).unwrap();
    a.sync();
    b.app.sync_now().unwrap();
    b.app.edit(id, "my unsaved text").unwrap();
    let Some(SyncEvent::Finished(f)) = b.app.wait_sync() else {
        panic!("no result");
    };
    assert!(f.message.unwrap().contains("deleted on another device"));
    assert_eq!(b.app.buffer(), Some("my unsaved text"));
    assert!(
        b.app.notes().iter().any(|n| n.id == id),
        "restored, not lost"
    );
}

#[test]
fn integrity_notices_stay_until_acknowledged_and_survive_a_restart() {
    let w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.write("N", "x");
    a.sync();
    b.sync();
    // The provider serves a shorter history than b verified: wipe the logs.
    for e in fs::read_dir(w.shared.join("d")).unwrap() {
        fs::remove_dir_all(e.unwrap().path()).unwrap();
    }
    b.sync();
    let notices = b.app.sync_notices();
    assert!(!notices.is_empty(), "rollback must be surfaced");
    assert!(
        notices
            .iter()
            .all(|n| !n.title.is_empty() && !n.detail.is_empty())
    );
    assert_eq!(b.app.sync_status().notices, notices.len());
    // More cycles never dismiss them.
    b.sync();
    assert!(b.app.sync_notices().len() >= notices.len());
    // A restart still shows them.
    b.app.lock(false).unwrap();
    b.app.unlock_with_passphrase(PW).unwrap();
    assert!(b.app.sync_notices().len() >= notices.len());
    // Only an explicit acknowledgement removes one.
    let n = b.app.sync_notices().len();
    b.app.acknowledge_sync_notice(0).unwrap();
    assert_eq!(b.app.sync_notices().len(), n - 1);
    assert!(b.app.acknowledge_sync_notice(99).is_err());
}

#[test]
fn stray_files_are_listed_and_never_touched() {
    let w = World::new();
    let mut a = w.first("a");
    a.write("N", "x");
    let stray = w.shared.join("notes (conflicted copy).txt");
    fs::write(&stray, b"hello").unwrap();
    assert!(finished_ok(a.sync()));
    let strays = a.app.sync_strays();
    assert!(
        strays
            .iter()
            .any(|s| s.name.contains("conflicted copy") && !s.treatment.is_empty()),
        "{strays:?}"
    );
    assert_eq!(fs::read(&stray).unwrap(), b"hello");
}

#[test]
fn cleanup_previews_without_deleting_and_is_unavailable_while_busy_or_locked() {
    let w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.write("N", "v0");
    for i in 1..=8 {
        a.sync();
        b.sync();
        a.set_body(id, &format!("v{i}"));
    }
    a.sync();
    b.sync();
    a.sync();
    let count = |p: &Path| {
        fn walk(p: &Path) -> usize {
            fs::read_dir(p)
                .unwrap()
                .map(|e| {
                    let p = e.unwrap().path();
                    if p.is_dir() { walk(&p) } else { 1 }
                })
                .sum()
        }
        walk(p)
    };
    let before = count(&w.shared);
    let preview = a.app.cleanup_preview().unwrap();
    assert!(preview.dry_run && preview.unavailable.is_none());
    assert_eq!(count(&w.shared), before);
    // Everything is far too recent for the real run (30-day grace): nothing is deleted.
    let done = a.app.cleanup_run().unwrap();
    assert!(!done.dry_run);
    assert_eq!(done.old_versions, 0);
    assert!(done.kept > 0);
    assert_eq!(count(&w.shared), before);

    a.app.sync_now().unwrap();
    assert!(matches!(a.app.cleanup_preview(), Err(Error::SyncBusy)));
    assert!(a.app.wait_sync().is_some());
    a.app.lock(false).unwrap();
    assert!(matches!(a.app.cleanup_preview(), Err(Error::Locked)));
}
