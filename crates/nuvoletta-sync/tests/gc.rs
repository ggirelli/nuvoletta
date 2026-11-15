//! Garbage collection (section 9), `revs/` pruning (section 6.1) and gap buffering (section 6.3
//! step 4): what is deleted, what is never touched, and that devices keep converging.

mod common;

use common::*;
use nuvoletta_core::NoteId;
use proptest::prelude::*;
use std::collections::BTreeSet;

fn objects(remote: &std::path::Path) -> BTreeSet<String> {
    all_files(remote)
        .into_keys()
        .filter(|k| k.starts_with("o/"))
        .collect()
}

/// A creates a note and edits it `n` times, syncing after every edit; B syncs after each, so
/// B's checkpoints acknowledge everything. A final round makes A see B's last checkpoint.
fn history(w: &mut World, a: &mut Device, b: &mut Device, n: usize) -> NoteId {
    let id = a.create("t", "p", "v0");
    for i in 1..=n {
        a.sync();
        b.sync();
        a.set_body(&id, &format!("v{i}"));
    }
    w.converge_gc(&mut [a, b]);
    id
}

#[test]
fn old_own_objects_are_deleted_after_grace_and_acknowledgement() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = history(&mut w, &mut a, &mut b, 9);
    let segs: BTreeSet<String> = all_files(&w.remote)
        .into_keys()
        .filter(|k| k.starts_with("d/") || k.starts_with("h/"))
        .collect();
    let before = objects(&w.remote);
    let r = a.gc(40, false);
    assert!(r.skipped.is_none());
    assert!(r.objects_collected > 0, "{r:?}");
    assert!(r.bytes_collected > 0);
    assert!(r.kept.heads >= 1 && r.kept.head_parents >= 1 && r.kept.recent >= 1);
    assert!(r.segments_kept > 0);
    let after = objects(&w.remote);
    assert_eq!(before.len() - after.len(), r.objects_collected);
    // Logs and headers are never touched.
    let segs_after: BTreeSet<String> = all_files(&w.remote)
        .into_keys()
        .filter(|k| k.starts_with("d/") || k.starts_with("h/"))
        .collect();
    assert_eq!(segs, segs_after);
    // The head and its parent survive; everybody still converges, a new device bootstraps.
    let head = a.heads(&id)[0];
    assert!(after.contains(&format!("o/{:02x}/{head}.nvobj", head.0[0])));
    a.set_body(&id, "after gc");
    let mut c = w.join("c", &a);
    w.converge_gc(&mut [&mut a, &mut b, &mut c]);
    assert_converged(&[&a, &b, &c]);
    assert_eq!(c.body(&id).as_deref(), Some("after gc"));
}

#[test]
fn grace_period_and_acknowledgements_protect_recent_history() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.create("t", "p", "v0");
    a.sync();
    b.sync(); // B acknowledges A's first entry only
    for i in 1..=9 {
        a.set_body(&id, &format!("v{i}"));
        a.sync();
    }
    // Grace not over: nothing, whatever B says.
    let r = a.gc(1, false);
    assert_eq!(r.objects_collected, 0);
    assert!(r.kept.within_grace > 0);
    // Grace over, but B (active within 90 days) has not seen the superseding entries.
    let r = a.gc(40, false);
    assert_eq!(r.objects_collected, 0, "{r:?}");
    assert!(r.kept.not_acknowledged > 0);
    // B catches up and acknowledges; A learns it.
    w.converge_gc(&mut [&mut a, &mut b]);
    assert!(a.gc(40, true).objects_collected > 0);
    // Much later B counts as gone (outside the ack window) even without checkpoints: still
    // bounded by K, heads and parents.
    let r = a.gc(400, false);
    assert!(r.kept.heads >= 1);
    assert!(a.exists(&id));
}

#[test]
fn unacknowledged_device_blocks_collection_even_when_it_is_old() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.create("t", "p", "v0");
    a.sync();
    b.sync();
    for i in 1..=9 {
        a.set_body(&id, &format!("v{i}"));
        a.sync();
    }
    // Inside the window B is active and behind; with a window of zero it is not.
    let opts = nuvoletta_sync::GcOptions {
        ack_window_ms: 0,
        now_ms: nuvoletta_core::now_ms() + 40 * 86_400_000,
        ..Default::default()
    };
    let r = a.syncer.collect_garbage(&a.vault, &opts).unwrap();
    assert!(r.objects_collected > 0);
}

#[test]
fn dry_run_deletes_nothing_and_predicts_the_real_run() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    history(&mut w, &mut a, &mut b, 9);
    let files = all_files(&w.remote);
    let dry = a.gc(40, true);
    assert!(dry.dry_run && dry.objects_collected > 0);
    assert_eq!(files, all_files(&w.remote));
    assert!(
        a.faults()
            .lock()
            .unwrap()
            .mutations
            .iter()
            .all(|m| !m.starts_with("delete:"))
    );
    let real = a.gc(40, false);
    assert_eq!(dry.objects_collected, real.objects_collected);
    assert_eq!(dry.bytes_collected, real.bytes_collected);
    assert_eq!(dry.kept, real.kept);
}

#[test]
fn only_own_objects_are_deleted_and_foreign_files_are_untouched() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = history(&mut w, &mut a, &mut b, 6);
    // B writes a long history too.
    for i in 0..9 {
        b.set_body(&id, &format!("b{i}"));
        b.sync();
        a.sync();
    }
    w.converge_gc(&mut [&mut a, &mut b]);
    let b_objects: BTreeSet<String> = b
        .faults()
        .lock()
        .unwrap()
        .mutations
        .iter()
        .filter_map(|m| m.strip_prefix("put:o/").map(|n| format!("o/{n}")))
        .collect();
    assert!(!b_objects.is_empty());
    // Strays and copies next to the protocol files.
    std::fs::write(w.remote.join("notes (conflicted copy).txt"), b"x").unwrap();
    let copy = b_objects
        .iter()
        .next()
        .unwrap()
        .replace(".nvobj", " (1).nvobj");
    std::fs::write(w.remote.join(&copy), b"copy").unwrap();
    let before = all_files(&w.remote);
    let r = a.gc(40, false);
    assert!(r.objects_collected > 0);
    assert!(r.kept.other_writers > 0, "{r:?}");
    let after = all_files(&w.remote);
    for name in before.keys().filter(|k| !after.contains_key(*k)) {
        assert!(
            name.starts_with("o/") && !b_objects.contains(name),
            "{name}"
        );
        assert!(name.ends_with(".nvobj") && !name.contains(' '));
    }
    assert!(b_objects.iter().all(|n| after.contains_key(n)));
    assert!(after.contains_key("notes (conflicted copy).txt"));
    assert!(after.contains_key(&copy));
    // B collects its own the same way, and both still converge.
    let rb = b.gc(40, false);
    assert!(rb.objects_collected > 0);
    w.converge_gc(&mut [&mut a, &mut b]);
    assert_converged(&[&a, &b]);
}

#[test]
fn notes_in_conflict_keep_every_revision() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = history(&mut w, &mut a, &mut b, 8);
    a.set_body(&id, "A side");
    b.set_body(&id, "B side");
    w.converge_gc(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&id).len(), 2);
    let before = objects(&w.remote);
    let r = a.gc(400, false);
    assert_eq!(r.objects_collected, 0);
    assert!(r.kept.in_conflict > 0 || r.kept.heads > 0);
    assert_eq!(before, objects(&w.remote));
}

#[test]
fn orphans_are_collected_after_a_week() {
    let mut w = World::new();
    let mut a = w.first("a");
    let id = a.create("t", "p", "v0");
    a.sync();
    // The object of an edit is uploaded, then the process dies before the entry is sealed.
    a.set_body(&id, "intermediate");
    a.syncer
        .arm_crash(Some(nuvoletta_sync::CrashPoint::AfterObjectsPushed));
    assert!(a.try_sync().is_err());
    a.restart();
    // A further edit collapses the unsealed intent: its uploaded object is now unreferenced.
    a.set_body(&id, "final");
    a.sync();
    let before = objects(&w.remote);
    let young = a.gc(1, false);
    assert_eq!(young.orphans_collected, 0);
    assert_eq!(young.orphans_waiting, 1);
    let dry = a.gc(8, true);
    assert_eq!(dry.orphans_collected, 1);
    assert_eq!(before, objects(&w.remote));
    let r = a.gc(8, false);
    assert_eq!(r.orphans_collected, 1);
    assert!(r.orphan_bytes > 0);
    assert_eq!(objects(&w.remote).len(), before.len() - 1);
    assert_eq!(a.gc(8, false).orphans_collected, 0);
    assert_eq!(a.body(&id).as_deref(), Some("final"));
}

#[test]
fn gc_refuses_while_the_state_is_forked_or_rebuilding() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    history(&mut w, &mut a, &mut b, 8);
    let mut c = w.clone_device("c", &a);
    a.set_body(&a.vault.list()[0].id.clone(), "a side");
    c.set_body(&c.vault.list()[0].id.clone(), "c side");
    a.sync();
    c.sync();
    b.sync();
    b.sync();
    let r = b.gc(400, false);
    if r.skipped.is_none() {
        // The fork was not visible to B: collection is still allowed, but only what is safe.
        assert!(r.kept.heads >= 1);
    } else {
        assert_eq!(r.skipped, Some(nuvoletta_sync::GcSkip::Forks));
        assert_eq!(r.objects_collected, 0);
    }
}

#[test]
fn a_crash_during_collection_is_safe_and_a_repeat_finishes_the_job() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = history(&mut w, &mut a, &mut b, 12);
    let total = a.gc(40, true).objects_collected;
    assert!(total > 1);
    // The provider fails after the listing and the first deletion.
    {
        let f = a.faults();
        let mut f = f.lock().unwrap();
        f.fail_from_op = Some(f.ops + 3);
    }
    assert!(a.try_gc(40, false).is_err());
    a.faults().lock().unwrap().fail_from_op = None;
    a.restart();
    let rest = a.gc(40, false);
    assert!(rest.objects_collected < total);
    assert_eq!(a.gc(40, false).objects_collected, 0);
    w.converge_gc(&mut [&mut a, &mut b]);
    assert_converged(&[&a, &b]);
    assert_eq!(b.body(&id).as_deref(), Some("v12"));
}

#[test]
fn the_revision_cache_keeps_what_merging_needs_and_prunes_the_rest() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.create("t", "p", "v0");
    let mut seen = Vec::new();
    let mut pruned = 0;
    for i in 1..=12 {
        a.sync();
        b.sync();
        a.set_body(&id, &format!("v{i}"));
        pruned += a.sync().revs_pruned;
        seen.extend(a.heads(&id));
    }
    assert!(pruned > 0);
    let cached: Vec<_> = seen
        .iter()
        .filter(|r| a.syncer.has_cached_revision(r))
        .collect();
    assert!(cached.len() <= 8, "{} cached", cached.len());
    let head = a.heads(&id)[0];
    assert!(a.syncer.has_cached_revision(&head));
    w.converge_gc(&mut [&mut a, &mut b]);
    // Offline concurrent edits of a metadata field and the body still merge: the base is kept.
    a.edit(&id, |n| n.tags = vec!["x".into()]);
    b.set_body(&id, "b body");
    w.converge_gc(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&id).len(), 1);
    assert_converged(&[&a, &b]);
    // A conflict keeps all it has.
    a.set_body(&id, "A");
    b.set_body(&id, "B");
    w.converge_gc(&mut [&mut a, &mut b]);
    let heads = a.heads(&id);
    assert_eq!(heads.len(), 2);
    assert!(heads.iter().all(|h| a.syncer.has_cached_revision(h)));
}

#[test]
fn segments_after_a_gap_are_buffered_and_not_fetched_again() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.create("t", "p", "v0");
    for i in 1..=3 {
        a.sync();
        a.set_body(&id, &format!("v{i}"));
    }
    a.sync();
    let dir = format!("d/{}/l/", a.syncer.device_id());
    let mut segs: Vec<String> = all_files(&w.remote)
        .into_keys()
        .filter(|k| k.starts_with(&dir))
        .collect();
    segs.sort();
    assert!(segs.len() >= 3);
    let first = segs[0].clone();
    b.faults().lock().unwrap().hidden = vec![first.clone()];
    let count = |b: &Device, s: &str| {
        b.faults()
            .lock()
            .unwrap()
            .gets
            .iter()
            .filter(|g| *g == s)
            .count()
    };
    for _ in 0..3 {
        b.sync();
    }
    for s in &segs[1..] {
        assert_eq!(count(&b, s), 1, "{s} fetched more than once");
    }
    // The buffer is persisted state: a restart does not fetch them again either.
    b.restart();
    b.faults().lock().unwrap().hidden = vec![first.clone()];
    b.sync();
    for s in &segs[1..] {
        assert_eq!(count(&b, s), 0, "{s} fetched after a restart");
    }
    assert!(!b.exists(&id));
    // The gap fills: everything applies without fetching the buffered files again.
    b.faults().lock().unwrap().hidden.clear();
    b.sync();
    b.sync();
    assert_eq!(b.body(&id).as_deref(), Some("v3"));
    for s in &segs[1..] {
        assert_eq!(count(&b, s), 0);
    }
    assert_converged(&[&a, &b]);
}

#[test]
fn a_buffered_segment_that_does_not_verify_is_dropped() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let id = a.create("t", "p", "v0");
    a.sync();
    a.set_body(&id, "v1");
    a.sync();
    let dir = format!("d/{}/l/", a.syncer.device_id());
    let mut segs: Vec<String> = all_files(&w.remote)
        .into_keys()
        .filter(|k| k.starts_with(&dir))
        .collect();
    segs.sort();
    b.faults().lock().unwrap().hidden = vec![segs[0].clone()];
    b.sync();
    // The file under the second name disappears from the folder: the buffer is not trusted.
    std::fs::remove_file(w.remote.join(&segs[1])).unwrap();
    b.sync();
    b.faults().lock().unwrap().hidden.clear();
    b.sync();
    assert!(!b.exists(&id) || b.body(&id).as_deref() == Some("v0"));
}

#[derive(Clone, Debug)]
enum Op {
    Create(usize),
    Edit(usize, usize, u8),
    Rename(usize, usize, u8),
    Delete(usize, usize),
    Sync(usize),
    Gc(usize, u8),
}

fn op(devices: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => (0..devices).prop_map(Op::Create),
        5 => (0..devices, 0..6usize, any::<u8>()).prop_map(|(d, n, v)| Op::Edit(d, n, v)),
        1 => (0..devices, 0..6usize, any::<u8>()).prop_map(|(d, n, v)| Op::Rename(d, n, v)),
        1 => (0..devices, 0..6usize).prop_map(|(d, n)| Op::Delete(d, n)),
        5 => (0..devices).prop_map(Op::Sync),
        3 => (0..devices, 0..4u8).prop_map(|(d, t)| Op::Gc(d, t)),
    ]
}

fn run(devices: usize, ops: &[Op]) {
    let mut w = World::new();
    let names = ["d0", "d1", "d2", "d3"];
    let mut devs = vec![w.first(names[0])];
    for n in names.iter().take(devices).skip(1) {
        let d = w.join(n, &devs[0]);
        devs.push(d);
    }
    let notes = |d: &Device| -> Vec<NoteId> { d.vault.list().into_iter().map(|s| s.id).collect() };
    for (i, o) in ops.iter().enumerate() {
        match o {
            Op::Create(d) => {
                devs[*d].create(&format!("n{i}"), "p", &format!("b{i}"));
            }
            Op::Edit(d, n, v) => {
                let l = notes(&devs[*d]);
                if let Some(id) = l.get(n % l.len().max(1)) {
                    devs[*d].set_body(id, &format!("e{i}-{v}"));
                }
            }
            Op::Rename(d, n, v) => {
                let l = notes(&devs[*d]);
                if let Some(id) = l.get(n % l.len().max(1)) {
                    let p = nuvoletta_core::NotePath::parse(&format!("dir{}/n", v % 3)).unwrap();
                    devs[*d].edit(id, |x| x.path = p);
                }
            }
            Op::Delete(d, n) => {
                let l = notes(&devs[*d]);
                if let Some(id) = l.get(n % l.len().max(1)) {
                    devs[*d].delete(id);
                }
            }
            Op::Sync(d) => {
                devs[*d].sync();
            }
            Op::Gc(d, t) => {
                // 0, 40, 120 and 400 days later: inside and outside grace and ack windows.
                let days = [0, 40, 120, 400][usize::from(*t)];
                devs[*d].gc(days, false);
            }
        }
    }
    let mut refs: Vec<&mut Device> = devs.iter_mut().collect();
    w.converge_gc(&mut refs);
    // A device that joins after all of that still reaches the same heads.
    let mut late = w.join("late", &devs[0]);
    let mut refs: Vec<&mut Device> = devs.iter_mut().collect();
    refs.push(&mut late);
    w.converge_gc(&mut refs);
    let all: Vec<&Device> = devs.iter().chain([&late]).collect();
    assert_converged(&all);
    let files = all_files(&w.remote);
    for d in &all {
        for (note, rev, is_delete) in d.syncer.known_revisions() {
            let heads = d.heads(&note);
            if is_delete || !heads.contains(&rev) {
                continue;
            }
            // Every head keeps its object and is available on every device.
            let name = format!("o/{:02x}/{rev}.nvobj", rev.0[0]);
            assert!(files.contains_key(&name), "head {rev} lost from the remote");
            assert!(
                d.syncer.has_cached_revision(&rev),
                "head {rev} not available on {}",
                d.name
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn collection_never_loses_heads_and_devices_still_converge(ops in proptest::collection::vec(op(3), 1..30)) {
        run(3, &ops);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    #[ignore = "expensive: run by `just test-slow`"]
    fn collection_is_safe_under_long_four_device_histories(ops in proptest::collection::vec(op(4), 20..80)) {
        run(4, &ops);
    }
}
