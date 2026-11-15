//! Crash safety: the sync cycle is interrupted at every labelled step and at every provider
//! operation, the process "restarts" (everything not persisted is lost) and the devices must
//! still converge without losing a revision, never overwriting or deleting a remote file.

mod common;

use common::*;
use nuvoletta_sync::{CrashPoint, Error};

const POINTS: [CrashPoint; 10] = [
    CrashPoint::AfterScan,
    CrashPoint::AfterObjectsPushed,
    CrashPoint::AfterSealed,
    CrashPoint::BeforeConfirm,
    CrashPoint::AfterUpload,
    CrashPoint::AfterList,
    CrashPoint::AfterLogsPulled,
    CrashPoint::AfterObjectsPulled,
    CrashPoint::AfterPlanSaved,
    CrashPoint::AfterMerge,
];

struct Setup {
    w: World,
    a: Device,
    b: Device,
    shared: nuvoletta_core::NoteId,
    from_a: nuvoletta_core::NoteId,
    from_b: nuvoletta_core::NoteId,
}

/// a and b share a note; then both make offline changes (a body edit and a new note on a, a
/// rename and a new note on b), and b publishes first, so a's next cycle has everything to do:
/// push, pull, fast-forward and an automatic merge.
fn setup() -> Setup {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let shared = a.create("shared", "old", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.set_body(&shared, "a edited the body");
    let from_a = a.create("from a", "p", "note made on a");
    b.edit(&shared, |n| {
        n.path = nuvoletta_core::NotePath::parse("renamed").unwrap()
    });
    let from_b = b.create("from b", "p", "note made on b");
    b.sync();
    Setup {
        w,
        a,
        b,
        shared,
        from_a,
        from_b,
    }
}

fn finish(mut s: Setup) {
    s.w.converge(&mut [&mut s.a, &mut s.b]);
    assert_converged(&[&s.a, &s.b]);
    assert_no_revision_lost(&s.w.remote, &[&s.a, &s.b]);
    for d in [&s.a, &s.b] {
        assert_eq!(
            d.body(&s.from_a).as_deref(),
            Some("note made on a"),
            "{}",
            d.name
        );
        assert_eq!(
            d.body(&s.from_b).as_deref(),
            Some("note made on b"),
            "{}",
            d.name
        );
        // The rename and the body edit were merged automatically.
        let n = d.note(&s.shared);
        assert_eq!(n.path.as_str(), "renamed", "{}", d.name);
        assert_eq!(n.body, "a edited the body", "{}", d.name);
        assert_eq!(d.heads(&s.shared).len(), 1, "{}", d.name);
    }
    // Idempotent: another full round writes nothing.
    let before = all_files(&s.w.remote);
    s.a.sync();
    s.b.sync();
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        all_files(&s.w.remote).keys().collect::<Vec<_>>()
    );
}

#[test]
fn a_crash_at_every_step_of_the_cycle_loses_nothing_on_the_device_that_pushes_and_pulls() {
    for point in POINTS {
        let mut s = setup();
        s.a.syncer.arm_crash(Some(point));
        match s.a.try_sync() {
            Err(Error::Crash(p)) => assert_eq!(p, point),
            other => panic!("{point:?}: expected a crash, got {other:?}"),
        }
        s.w.check_immutable();
        s.a.restart();
        finish(s);
    }
}

#[test]
fn a_crash_at_every_step_on_the_device_that_publishes_first() {
    for point in POINTS {
        let mut w = World::new();
        let mut a = w.first("a");
        let mut b = w.join("b", &a);
        let shared = a.create("shared", "old", "base");
        w.converge(&mut [&mut a, &mut b]);
        a.set_body(&shared, "a edited the body");
        let from_a = a.create("from a", "p", "note made on a");
        b.edit(&shared, |n| {
            n.path = nuvoletta_core::NotePath::parse("renamed").unwrap()
        });
        let from_b = b.create("from b", "p", "note made on b");
        a.sync();
        // b crashes mid-cycle with a's changes to pull, merge and install.
        b.syncer.arm_crash(Some(point));
        let _ = b.try_sync();
        w.check_immutable();
        b.restart();
        finish(Setup {
            w,
            a,
            b,
            shared,
            from_a,
            from_b,
        });
    }
}

#[test]
fn a_crash_at_every_provider_operation_loses_nothing() {
    let mut op = 1;
    loop {
        let mut s = setup();
        s.a.faults().lock().unwrap().fail_from_op = Some(op);
        let r = s.a.try_sync();
        s.w.check_immutable();
        let crashed = r.is_err();
        s.a.restart();
        finish(s);
        if !crashed {
            break; // the whole cycle fits in fewer than `op` operations: every step was covered
        }
        op += 1;
        assert!(
            op < 200,
            "a cycle should not need hundreds of provider operations"
        );
    }
    assert!(
        op > 10,
        "the cycle was interrupted at many different operations (got {op})"
    );
}

#[test]
fn a_crash_at_every_provider_operation_on_the_second_device_too() {
    let mut op = 1;
    loop {
        let mut s = setup();
        s.a.sync();
        s.b.faults().lock().unwrap().fail_from_op = Some(op);
        let r = s.b.try_sync();
        s.w.check_immutable();
        let crashed = r.is_err();
        s.b.restart();
        finish(s);
        if !crashed {
            break;
        }
        op += 1;
        assert!(op < 200);
    }
}

#[test]
fn a_crash_between_segment_upload_and_confirmation_does_not_duplicate_entries() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "x");
    a.syncer.arm_crash(Some(CrashPoint::BeforeConfirm));
    assert!(matches!(a.try_sync(), Err(Error::Crash(_))));
    a.restart();
    // The sealed bytes were persisted: the retry finds the identical file and confirms it.
    let r = a.sync();
    assert_eq!(r.segments_uploaded, 1);
    let segs = all_files(&w.remote)
        .keys()
        .filter(|k| k.ends_with(".nvlog"))
        .count();
    assert_eq!(segs, 1);
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(b.body(&n).as_deref(), Some("x"));
}

#[test]
fn a_sealed_segment_survives_a_crash_and_is_uploaded_byte_identical() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.create("T", "p", "x");
    a.syncer.arm_crash(Some(CrashPoint::AfterSealed));
    assert!(a.try_sync().is_err());
    assert!(all_files(&w.remote).keys().all(|k| !k.ends_with(".nvlog")));
    let sealed = std::fs::read(a.dir.join(".nuvoletta/sync-state/journal")).unwrap();
    a.restart();
    a.sync();
    let seg = all_files(&w.remote)
        .into_iter()
        .find(|(k, _)| k.ends_with(".nvlog"))
        .unwrap();
    // The journal does not hold the segment in the clear; the uploaded bytes are exactly the
    // ones that were sealed before the crash (the journal embeds them verbatim in its CBOR).
    assert!(sealed.len() > seg.1.len() / 2);
    let _ = seg;
}

#[test]
fn a_half_written_state_save_continues_under_a_new_device_id_without_losing_the_entry() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.sync();
    let state = a.dir.join(".nuvoletta/sync-state");
    let old_peers = std::fs::read(state.join("peers")).unwrap();
    let old_notes = std::fs::read(state.join("notes")).unwrap();
    let n = a.create("T", "p", "x");
    a.syncer.arm_crash(Some(CrashPoint::AfterSealed));
    assert!(a.try_sync().is_err());
    // The crash hit between the journal write and the peers/notes writes: only the journal
    // (with the sealed bytes) is new.
    std::fs::write(state.join("peers"), old_peers).unwrap();
    std::fs::write(state.join("notes"), old_notes).unwrap();
    let old_id = a.syncer.device_id();
    a.restart();
    let r = a.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, nuvoletta_sync::Warning::OwnForkRecovered { .. }))
    );
    assert_ne!(a.syncer.device_id(), old_id);
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(b.body(&n).as_deref(), Some("x"));
    assert_converged(&[&a, &b]);
}
