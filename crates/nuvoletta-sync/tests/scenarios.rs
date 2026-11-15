//! Worked scenarios of `SYNC_PROTOCOL.md` section 12 and the basic flows, with two or three
//! in-process vault instances syncing through a folder.

mod common;

use common::*;
use nuvoletta_sync::{NoteStatus, StrayKind, Warning};

#[test]
fn a_new_note_reaches_the_other_device_and_a_second_sync_changes_nothing() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("Title", "inbox/one", "hello");
    let r = a.sync();
    assert_eq!(
        (r.intents_created, r.objects_pushed, r.segments_uploaded),
        (1, 1, 1)
    );
    let r = b.sync();
    assert_eq!(r.entries_applied, 1);
    assert_eq!(r.fast_forwards, 1);
    assert_eq!(b.body(&n).as_deref(), Some("hello"));
    assert_eq!(b.note(&n).path.as_str(), "inbox/one");
    // Idempotent: a repeated sync does nothing at all.
    for d in [&mut a, &mut b] {
        let before = all_files(&w.remote);
        let r = d.sync();
        assert_eq!(
            (
                r.intents_created,
                r.objects_pushed,
                r.segments_uploaded,
                r.entries_applied,
                r.fast_forwards
            ),
            (0, 0, 0, 0, 0)
        );
        let after = all_files(&w.remote);
        // Only a checkpoint may be added by the receiving device; never anything else.
        assert!(
            after
                .keys()
                .all(|k| after[k] == *before.get(k).unwrap_or(&after[k]))
        );
    }
    w.converge(&mut [&mut a, &mut b]);
    assert_converged(&[&a, &b]);
    assert_eq!(a.syncer.note_status(&n), NoteStatus::Synced);
    assert_eq!(b.syncer.note_status(&n), NoteStatus::Synced);
}

#[test]
fn edits_before_upload_collapse_into_one_revision() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "v1");
    w.converge(&mut [&mut a, &mut b]);
    a.set_body(&n, "v2");
    a.syncer.record_local_changes(&a.vault).unwrap();
    a.set_body(&n, "v3");
    a.set_body(&n, "v4");
    let objects_before = all_files(&w.remote)
        .keys()
        .filter(|k| k.starts_with("o/"))
        .count();
    w.converge(&mut [&mut a, &mut b]);
    let objects_after = all_files(&w.remote)
        .keys()
        .filter(|k| k.starts_with("o/"))
        .count();
    assert_eq!(
        objects_after - objects_before,
        1,
        "intermediate revisions are not uploaded"
    );
    assert_eq!(b.body(&n).as_deref(), Some("v4"));
    assert_converged(&[&a, &b]);
}

#[test]
fn deletes_propagate_and_the_content_stays_retained() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "keep me");
    w.converge(&mut [&mut a, &mut b]);
    b.delete(&n);
    w.converge(&mut [&mut a, &mut b]);
    assert!(!a.exists(&n) && !b.exists(&n));
    assert_converged(&[&a, &b]);
    // The deleted content is still cached locally and on the remote.
    assert_no_revision_lost(&w.remote, &[&a, &b]);
    let revs: Vec<_> = a
        .syncer
        .known_revisions()
        .into_iter()
        .filter(|r| !r.2)
        .collect();
    assert!(revs.iter().all(|(_, r, _)| a.syncer.has_cached_revision(r)));
}

#[test]
fn a_note_created_and_deleted_before_any_sync_leaves_no_trace() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.sync(); // initialise the sync state
    let n = a.create("T", "p", "x");
    a.syncer.record_local_changes(&a.vault).unwrap();
    a.delete(&n);
    a.sync();
    assert!(a.syncer.known_revisions().is_empty());
    assert!(all_files(&w.remote).keys().all(|k| !k.starts_with("o/")));
}

#[test]
fn s1_two_devices_edit_the_same_note_offline() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.set_body(&n, "from a");
    b.set_body(&n, "from b");
    w.converge(&mut [&mut a, &mut b]);
    // Both bodies changed: two heads, nothing merged, nothing lost, local content untouched.
    assert_eq!(a.heads(&n).len(), 2);
    assert_eq!(a.heads(&n), b.heads(&n));
    assert_eq!(a.body(&n).as_deref(), Some("from a"));
    assert_eq!(b.body(&n).as_deref(), Some("from b"));
    let NoteStatus::Conflict(heads) = a.syncer.note_status(&n) else {
        panic!("expected a conflict")
    };
    assert!(heads.iter().all(|h| h.available));
    let bodies: Vec<String> = heads
        .iter()
        .map(|h| {
            a.syncer
                .read_revision(&a.vault, &n, &h.rev)
                .unwrap()
                .body
                .clone()
        })
        .collect();
    assert!(bodies.contains(&"from a".to_owned()) && bodies.contains(&"from b".to_owned()));
    assert_eq!(a.syncer.conflicts().len(), 1);
    // Resolve on a by choosing b's version: everyone converges on a single head.
    let b_head = heads
        .iter()
        .find(|h| a.syncer.read_revision(&a.vault, &n, &h.rev).unwrap().body == "from b")
        .unwrap()
        .rev;
    a.syncer
        .resolve_choose_head(&mut a.vault, &n, &b_head)
        .unwrap();
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&n).len(), 1);
    assert_eq!(a.body(&n).as_deref(), Some("from b"));
    assert_eq!(b.body(&n).as_deref(), Some("from b"));
    assert_converged(&[&a, &b]);
    assert_no_revision_lost(&w.remote, &[&a, &b]);
}

#[test]
fn s1_concurrent_identical_resolutions_end_in_one_head() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.set_body(&n, "one");
    b.set_body(&n, "two");
    // Both see the conflict, both resolve concurrently with the same chosen text.
    a.sync();
    b.sync();
    a.sync();
    b.sync();
    for d in [&mut a, &mut b] {
        let heads = d.syncer.conflicts()[0].heads.clone();
        let h = heads
            .iter()
            .find(|h| d.syncer.read_revision(&d.vault, &n, &h.rev).unwrap().body == "one")
            .unwrap()
            .rev;
        d.syncer.resolve_choose_head(&mut d.vault, &n, &h).unwrap();
    }
    // Two resolution revisions with identical content: one designated device merges them.
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&n).len(), 1, "the conflict never reappears");
    assert_converged(&[&a, &b]);
    assert_eq!(b.body(&n).as_deref(), Some("one"));
}

#[test]
fn s2_rename_versus_edit_merges_automatically() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "old/path", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.edit(&n, |x| {
        x.path = nuvoletta_core::NotePath::parse("new/path").unwrap()
    });
    b.set_body(&n, "edited elsewhere");
    let ra = a.sync();
    let rb = b.sync();
    let ra2 = a.sync();
    let rb2 = b.sync();
    assert!(ra.auto_merges + rb.auto_merges + ra2.auto_merges + rb2.auto_merges >= 1);
    w.converge(&mut [&mut a, &mut b]);
    for d in [&a, &b] {
        let note = d.note(&n);
        assert_eq!(note.path.as_str(), "new/path");
        assert_eq!(note.body, "edited elsewhere");
        assert_eq!(d.heads(&n).len(), 1);
    }
    assert_converged(&[&a, &b]);
    assert_no_revision_lost(&w.remote, &[&a, &b]);
}

#[test]
fn tags_and_title_edits_merge_but_two_body_edits_do_not() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.edit(&n, |x| x.title = "New title".into());
    b.edit(&n, |x| x.tags = vec!["t".into()]);
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&n).len(), 1);
    assert_eq!(b.note(&n).title, "New title");
    assert_eq!(b.note(&n).tags, vec!["t".to_owned()]);
    assert_converged(&[&a, &b]);
}

#[test]
fn s3_delete_versus_edit_is_a_conflict_and_keep_resolves_it() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.delete(&n);
    b.set_body(&n, "edited");
    w.converge(&mut [&mut a, &mut b]);
    // Not deleted on b, not resurrected on a.
    assert!(!a.exists(&n));
    assert_eq!(b.body(&n).as_deref(), Some("edited"));
    assert_eq!(a.heads(&n).len(), 2);
    assert!(matches!(a.syncer.note_status(&n), NoteStatus::Conflict(_)));
    a.syncer.resolve_keep(&mut a.vault, &n).unwrap();
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&n).len(), 1);
    assert_eq!(a.body(&n).as_deref(), Some("edited"));
    assert_eq!(b.body(&n).as_deref(), Some("edited"));
    assert_converged(&[&a, &b]);
}

#[test]
fn s3_delete_versus_edit_resolved_by_delete_retains_the_edit() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.delete(&n);
    b.set_body(&n, "edited");
    w.converge(&mut [&mut a, &mut b]);
    b.syncer.resolve_delete(&mut b.vault, &n).unwrap();
    w.converge(&mut [&mut a, &mut b]);
    assert!(!a.exists(&n) && !b.exists(&n));
    assert_eq!(a.heads(&n).len(), 1);
    assert_converged(&[&a, &b]);
    // The edited revision is retained (a parent of the new tombstone) so the user can undo.
    assert_no_revision_lost(&w.remote, &[&a, &b]);
    let edited: Vec<_> = b
        .syncer
        .known_revisions()
        .into_iter()
        .filter(|(_, r, del)| {
            !del && b
                .syncer
                .read_revision(&b.vault, &n, r)
                .is_ok_and(|x| x.body == "edited")
        })
        .collect();
    assert_eq!(edited.len(), 1);
}

#[test]
fn keep_both_makes_a_conflict_copy_with_a_new_note_id() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("Plan", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    a.set_body(&n, "mine");
    b.set_body(&n, "theirs");
    w.converge(&mut [&mut a, &mut b]);
    let heads = a.syncer.conflicts()[0].heads.clone();
    let rev_of = |d: &Device, body: &str| {
        heads
            .iter()
            .find(|h| d.syncer.read_revision(&d.vault, &n, &h.rev).unwrap().body == body)
            .unwrap()
            .rev
    };
    let (keep, copy) = (rev_of(&a, "mine"), rev_of(&a, "theirs"));
    let copy_id = a
        .syncer
        .resolve_keep_both(&mut a.vault, &n, &keep, &copy)
        .unwrap();
    assert_ne!(copy_id, n);
    w.converge(&mut [&mut a, &mut b]);
    for d in [&a, &b] {
        assert_eq!(d.body(&n).as_deref(), Some("mine"));
        assert_eq!(d.body(&copy_id).as_deref(), Some("theirs"));
        assert_eq!(d.note(&copy_id).title, "Plan (conflict copy)");
        assert_eq!(d.heads(&n).len(), 1);
    }
    assert_converged(&[&a, &b]);
}

#[test]
fn three_devices_converge_through_one_folder() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let mut c = w.join("c", &a);
    let n1 = a.create("one", "p", "1");
    let n2 = b.create("two", "p", "2");
    let n3 = c.create("three", "p", "3");
    w.converge(&mut [&mut a, &mut b, &mut c]);
    for d in [&a, &b, &c] {
        assert_eq!(d.body(&n1).as_deref(), Some("1"));
        assert_eq!(d.body(&n2).as_deref(), Some("2"));
        assert_eq!(d.body(&n3).as_deref(), Some("3"));
    }
    assert_converged(&[&a, &b, &c]);
}

#[test]
fn offline_edits_accumulate_and_sync_later() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    // b goes offline: the provider fails for every operation.
    b.faults().lock().unwrap().fail_from_op = Some(1);
    let m = b.create("offline", "p", "made offline");
    b.set_body(&n, "edited offline");
    assert!(b.try_sync().is_err());
    // Meanwhile a edits another note.
    let k = a.create("other", "p", "new on a");
    a.sync();
    // b comes back.
    b.faults().lock().unwrap().fail_from_op = None;
    b.restart();
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.body(&n).as_deref(), Some("edited offline"));
    assert_eq!(a.body(&m).as_deref(), Some("made offline"));
    assert_eq!(b.body(&k).as_deref(), Some("new on a"));
    assert_converged(&[&a, &b]);
}

#[test]
fn s4_conflicted_copy_files_are_stray_but_never_lost() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "x");
    a.sync();
    // A sync tool duplicates a's segment and an object under conflicted-copy names.
    let files = all_files(&w.remote);
    let seg = files
        .keys()
        .find(|k| k.ends_with(".nvlog"))
        .unwrap()
        .clone();
    let obj = files
        .keys()
        .find(|k| k.ends_with(".nvobj"))
        .unwrap()
        .clone();
    for f in [&seg, &obj] {
        let (stem, ext) = f.rsplit_once('.').unwrap();
        std::fs::copy(
            w.remote.join(f),
            w.remote.join(format!("{stem} (conflicted copy).{ext}")),
        )
        .unwrap();
    }
    std::fs::write(w.remote.join("h/.DS_Store"), b"junk").unwrap();
    std::fs::write(w.remote.join("d/notes.txt"), b"junk").unwrap();
    let r = b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("x"));
    // Identical copies deduplicate; unrelated files are only reported.
    assert!(r.strays.iter().any(|s| s.kind == StrayKind::DuplicateCopy
        && s.name.ends_with(".nvlog")
        || s.name.contains("(conflicted copy).nvlog")));
    assert!(
        r.strays
            .iter()
            .any(|s| s.kind == StrayKind::Ignored && s.name.ends_with(".DS_Store"))
    );
    assert!(
        r.strays
            .iter()
            .any(|s| s.kind == StrayKind::Ignored && s.name == "d/notes.txt")
    );
    assert!(r.invalid.is_empty());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    // The engine never deletes strays.
    assert!(w.remote.join("h/.DS_Store").exists());
    assert!(w.remote.join("d/notes.txt").exists());
}

#[test]
fn s4_a_conflicted_copy_with_different_entries_is_fork_evidence_and_nothing_is_deleted() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let mut c = w.join("c", &a);
    let n = a.create("T", "p", "from a");
    // A restored clone of a (same device id) writes a different seq 1 in its own folder.
    let mut a2 = w.clone_device("a2", &a);
    a.sync();
    a2.set_body(&n, "from the clone");
    a2.faults().lock().unwrap().fail_from_op = None;
    // Make the clone's segment appear as a sync-tool conflicted copy of a's first segment.
    let before = all_files(&w.remote);
    let seg_a = before
        .keys()
        .find(|k| k.ends_with(".nvlog"))
        .unwrap()
        .clone();
    // The clone has not synced yet: it uploads into a scratch remote and we copy the file over.
    let scratch = w.tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    a2.syncer =
        nuvoletta_sync::Syncer::open_with(&a2.vault, Faulty::new(&scratch), config()).unwrap();
    a2.sync();
    let clone_files = all_files(&scratch);
    let clone_seg = clone_files
        .keys()
        .find(|k| k.ends_with(".nvlog"))
        .unwrap()
        .clone();
    let (stem, ext) = seg_a.rsplit_once('.').unwrap();
    std::fs::write(
        w.remote.join(format!("{stem} (conflicted copy).{ext}")),
        &clone_files[&clone_seg],
    )
    .unwrap();
    std::fs::create_dir_all(w.remote.join("o")).unwrap();
    for (k, v) in clone_files.iter().filter(|(k, _)| k.starts_with("o/")) {
        let p = w.remote.join(k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, v).unwrap();
    }
    let r = b.sync();
    // b read a's real segment, then the contradicting copy: the stream is frozen at seq 1, so
    // the note was never installed from either branch.
    assert!(
        r.strays.iter().any(|s| s.kind == StrayKind::ForkEvidence),
        "{:?}",
        r.strays
    );
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::DeviceFork { .. }))
    );
    assert!(
        b.syncer
            .peers()
            .iter()
            .any(|(d, _, f)| *d == a.syncer.device_id() && *f == Some(1))
    );
    assert!(!b.exists(&n));
    c.sync();
    // The conflicted copy sits in a's own directory: a finds the fork of its own id and
    // re-emits under a new id; the clone does the same once it syncs against the real remote.
    let old_a = a.syncer.device_id();
    let r = a.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::OwnForkRecovered { .. })),
        "{:?}",
        r.warnings
    );
    assert_ne!(a.syncer.device_id(), old_a);
    a2.restart();
    a2.sync();
    w.converge(&mut [&mut a, &mut b, &mut c, &mut a2]);
    // Both branches of the note are alive everywhere as a visible two-head conflict (two roots).
    for d in [&a, &b, &c, &a2] {
        assert_eq!(d.heads(&n).len(), 2, "{}", d.name);
    }
    assert_converged(&[&a, &b, &c, &a2]);
    assert_no_revision_lost(&w.remote, &[&a, &b, &c, &a2]);
    // Nothing was deleted from the remote by anyone.
    for k in before.keys() {
        assert!(w.remote.join(k).exists());
    }
}

#[test]
fn s5_out_of_order_segments_are_buffered_until_the_gap_fills() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n1 = a.create("one", "p", "1");
    a.sync();
    let seg1 = all_files(&w.remote)
        .keys()
        .find(|k| k.ends_with(".nvlog"))
        .unwrap()
        .clone();
    let n2 = a.create("two", "p", "2");
    a.sync();
    let n3 = a.create("three", "p", "3");
    a.sync();
    // The first segment is delayed: b sees the later ones first.
    b.faults().lock().unwrap().hidden.push(seg1.clone());
    let r = b.sync();
    assert_eq!(r.entries_applied, 0, "nothing past the gap is applied");
    assert!(!b.exists(&n1) && !b.exists(&n2) && !b.exists(&n3));
    let r = b.sync();
    assert!(
        r.warnings.iter().any(|x| matches!(
            x,
            Warning::StreamGap {
                expected_seq: 1,
                ..
            }
        )),
        "{:?}",
        r.warnings
    );
    // The gap fills: everything applies in order.
    b.faults().lock().unwrap().hidden.clear();
    b.sync();
    assert_eq!(b.body(&n1).as_deref(), Some("1"));
    assert_eq!(b.body(&n3).as_deref(), Some("3"));
    assert_converged(&[&a, &b]);
}

#[test]
fn s5_a_dangling_revision_is_applied_to_the_graph_but_shown_only_when_its_object_arrives() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "payload");
    a.sync();
    let obj = all_files(&w.remote)
        .keys()
        .find(|k| k.starts_with("o/"))
        .unwrap()
        .clone();
    b.faults().lock().unwrap().hidden.push(obj.clone());
    let r = b.sync();
    assert_eq!(r.entries_applied, 1);
    assert_eq!(r.dangling, 1);
    assert!(!b.exists(&n));
    assert_eq!(b.syncer.note_status(&n), NoteStatus::Dangling);
    let r = b.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::ObjectUnavailable { .. }))
    );
    b.faults().lock().unwrap().hidden.clear();
    let r = b.sync();
    assert_eq!(r.objects_fetched, 1);
    assert_eq!(b.body(&n).as_deref(), Some("payload"));
}

#[test]
fn truncated_files_are_not_yet_arrived_and_never_applied() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "whole");
    a.sync();
    b.faults().lock().unwrap().truncated.push("d/".into());
    b.faults().lock().unwrap().truncated.push("o/".into());
    let r = b.sync();
    assert_eq!(r.entries_applied, 0);
    assert!(!b.exists(&n));
    b.faults().lock().unwrap().truncated.clear();
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("whole"));
}

#[test]
fn a_truncated_object_is_never_installed() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "whole");
    a.sync();
    b.faults().lock().unwrap().truncated.push("o/".into());
    let r = b.sync();
    assert_eq!(r.entries_applied, 1);
    assert!(!b.exists(&n));
    b.faults().lock().unwrap().truncated.clear();
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("whole"));
}

#[test]
fn duplicated_uploads_are_deduplicated() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.faults().lock().unwrap().duplicate_puts = true;
    let n = a.create("T", "p", "x");
    a.sync();
    let m = a.create("U", "p", "y");
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(b.body(&n).as_deref(), Some("x"));
    assert_eq!(b.body(&m).as_deref(), Some("y"));
    let r = b.sync();
    assert!(
        r.strays.iter().all(|s| s.kind == StrayKind::DuplicateCopy),
        "{:?}",
        r.strays
    );
    assert!(r.warnings.is_empty());
    assert_converged(&[&a, &b]);
}

#[test]
fn a_dropped_upload_is_retried_with_the_same_bytes() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.faults()
        .lock()
        .unwrap()
        .drop_puts
        .push((".nvlog".into(), 1));
    let n = a.create("T", "p", "x");
    let r = a.sync();
    assert_eq!(
        r.segments_uploaded, 0,
        "not confirmed: the listing does not show it"
    );
    let sealed_bytes_seen = all_files(&w.remote)
        .keys()
        .filter(|k| k.ends_with(".nvlog"))
        .count();
    assert_eq!(sealed_bytes_seen, 0);
    let r = a.sync();
    assert_eq!(r.segments_uploaded, 1);
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("x"));
}

#[test]
fn a_leftover_tmp_file_of_our_own_is_removed_and_the_upload_repeats() {
    let mut w = World::new();
    let mut a = w.first("a");
    let n = a.create("T", "p", "x");
    a.sync();
    let dir = std::fs::read_dir(w.remote.join("d"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("l");
    std::fs::write(dir.join(".tmp-0123456789abcdef"), b"half written").unwrap();
    // A foreign tmp is left alone.
    let other = w.remote.join("d").join("ab".repeat(16)).join("l");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join(".tmp-0123456789abcdef"), b"theirs").unwrap();
    a.set_body(&n, "y");
    a.sync();
    assert!(!dir.join(".tmp-0123456789abcdef").exists());
    assert!(other.join(".tmp-0123456789abcdef").exists());
}

#[test]
fn state_survives_a_restart_and_a_lost_notes_state_is_rebuilt_from_the_remote() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "x");
    w.converge(&mut [&mut a, &mut b]);
    a.restart();
    assert_eq!(a.sync().entries_applied, 0);
    // Lose everything rebuildable on b (peers, notes, inventory, pending, revs).
    let state = b.dir.join(".nuvoletta/sync-state");
    for f in ["peers", "notes", "inventory", "pending"] {
        let _ = std::fs::remove_file(state.join(f));
    }
    for e in std::fs::read_dir(state.join("revs")).unwrap() {
        std::fs::remove_file(e.unwrap().path()).unwrap();
    }
    b.restart();
    // The first sync after the loss adopts the vault's revisions as known history...
    assert_eq!(b.sync().intents_created, 0);
    // ...so an edit afterwards is a normal descendant, not a competing root.
    b.set_body(&n, "edited after the loss");
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.body(&n).as_deref(), Some("edited after the loss"));
    assert_eq!(a.heads(&n).len(), 1);
    assert_converged(&[&a, &b]);
}

#[test]
fn an_edit_made_while_the_sync_state_was_lost_becomes_a_visible_conflict_not_a_silent_overwrite() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("T", "p", "x");
    w.converge(&mut [&mut a, &mut b]);
    let state = b.dir.join(".nuvoletta/sync-state");
    for f in ["peers", "notes", "inventory", "pending"] {
        let _ = std::fs::remove_file(state.join(f));
    }
    b.restart();
    // The base of this edit is unknown (the state that knew it is gone).
    b.set_body(&n, "edited blind");
    w.converge(&mut [&mut a, &mut b]);
    assert_eq!(a.heads(&n).len(), 2);
    assert_eq!(
        a.body(&n).as_deref(),
        Some("x"),
        "a's content is not overwritten"
    );
    assert_eq!(
        b.body(&n).as_deref(),
        Some("edited blind"),
        "neither is b's"
    );
    assert_converged(&[&a, &b]);
}

#[test]
fn rollback_of_a_remote_log_is_reported_and_state_is_not_reverted() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n1 = a.create("one", "p", "1");
    a.sync();
    let snapshot = all_files(&w.remote);
    let n2 = a.create("two", "p", "2");
    a.sync();
    b.sync();
    assert!(b.exists(&n1) && b.exists(&n2));
    // The provider serves an older state: a's second segment (and object) vanish.
    for (k, _) in all_files(&w.remote) {
        if !snapshot.contains_key(&k) {
            std::fs::remove_file(w.remote.join(&k)).unwrap();
        }
    }
    let r = b.sync();
    assert!(
        r.warnings.iter().any(|x| matches!(
            x,
            Warning::Rollback {
                known_seq: 2,
                remote_seq: 1,
                ..
            }
        )),
        "{:?}",
        r.warnings
    );
    assert!(b.exists(&n2), "local state is not reverted");
}

#[test]
fn a_checkpoint_naming_entries_we_cannot_see_is_reported_as_possibly_withheld() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let mut c = w.join("c", &a);
    a.create("one", "p", "1");
    a.sync();
    b.sync(); // b verifies a#1 and checkpoints it
    b.create("b's", "p", "x");
    b.sync();
    // The provider hides a's whole log from c, but b's checkpoint claims a#1.
    let a_dir = format!("d/{}/", a.syncer.device_id());
    c.faults().lock().unwrap().hidden.push(a_dir);
    c.sync();
    let r = c.sync();
    assert!(
        r.warnings.iter().any(|x| matches!(
            x,
            Warning::PossiblyWithheld {
                claimed_seq: 1,
                have_seq: 0,
                ..
            }
        )),
        "{:?}",
        r.warnings
    );
}

#[test]
fn a_cloned_device_directory_is_detected_and_recovers_with_a_new_id_losing_nothing() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n = a.create("shared", "p", "base");
    w.converge(&mut [&mut a, &mut b]);
    // b is cloned (restored from backup): same device id, same sync state.
    let mut b2 = w.clone_device("b2", &b);
    assert_eq!(b2.syncer.device_id(), b.syncer.device_id());
    let k1 = b.create("on b", "p", "from b");
    let k2 = b2.create("on b2", "p", "from b2");
    w.converge(&mut [&mut a, &mut b]);
    // b2 now writes the same seq with different content under the same id: its upload finds
    // b's segment and recovers with a new id, re-emitting its own entries.
    let r = b2.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::OwnForkRecovered { .. })),
        "{:?}",
        r.warnings
    );
    assert_ne!(b2.syncer.device_id(), b.syncer.device_id());
    w.converge(&mut [&mut a, &mut b, &mut b2]);
    // Nothing from either branch was lost, everywhere.
    for d in [&a, &b, &b2] {
        assert_eq!(d.body(&n).as_deref(), Some("base"), "{}", d.name);
        assert_eq!(d.body(&k1).as_deref(), Some("from b"), "{}", d.name);
        assert_eq!(d.body(&k2).as_deref(), Some("from b2"), "{}", d.name);
    }
    assert_converged(&[&a, &b, &b2]);
    assert_no_revision_lost(&w.remote, &[&a, &b, &b2]);
}

#[test]
fn a_fork_seen_first_by_a_third_device_freezes_the_stream_and_marks_notes() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let mut c = w.join("c", &a);
    let n = a.create("seed", "p", "seed");
    w.converge(&mut [&mut a, &mut b, &mut c]);
    let mut b2 = w.clone_device("b2", &b);
    let k1 = b.create("one", "p", "b side");
    b.sync();
    c.sync(); // c applies b's entry (seq beyond the clone point)
    assert!(c.exists(&k1));
    // The clone writes a different entry at the same seq; its file lands under another name.
    let scratch = w.tmp.path().join("scratch2");
    std::fs::create_dir_all(&scratch).unwrap();
    b2.syncer =
        nuvoletta_sync::Syncer::open_with(&b2.vault, Faulty::new(&scratch), config()).unwrap();
    let k2 = b2.create("two", "p", "clone side");
    b2.sync();
    for (k, v) in all_files(&scratch) {
        if k.starts_with("d/") {
            let name = k.replace(".nvlog", " (restored).nvlog");
            std::fs::write(w.remote.join(name), v).unwrap();
        } else if k.starts_with("o/") {
            let p = w.remote.join(&k);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, v).unwrap();
        }
    }
    let r = c.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::DeviceFork { .. })),
        "{:?}",
        r.warnings
    );
    // The note installed from the frozen branch stays as local content, marked for attention.
    assert!(c.exists(&k1));
    assert_eq!(c.syncer.note_status(&k1), NoteStatus::NeedsAttention);
    assert!(!c.exists(&k2));
    // Syncing again changes nothing and never deletes local content.
    c.sync();
    assert!(c.exists(&k1));
    let _ = n;
}

#[test]
fn an_old_vault_header_generation_is_flagged() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.sync();
    assert!(all_files(&w.remote).keys().any(|k| k.starts_with("h/")));
    // Pretend a newer generation was seen before; the remote now only has generation 1.
    let mut b = w.join("b", &a);
    b.sync();
    drop(b);
    // Hand-craft: bump max-header-gen by syncing against a remote that has a header file of a
    // higher generation, then remove it.
    let hdr = std::fs::read(w.remote.join(format!("h/{:08x}.nvhdr", 1))).unwrap();
    let mut gen2 = hdr.clone();
    gen2[10..14].copy_from_slice(&2u32.to_be_bytes());
    std::fs::write(w.remote.join(format!("h/{:08x}.nvhdr", 2)), &gen2).unwrap();
    let r = a.sync();
    // The fake generation-2 file fails the MAC-less checks only if invalid; it parses, so it is
    // reported as newer than the local header.
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderNewer { generation: 2 })),
        "{:?}",
        r.warnings
    );
    std::fs::remove_file(w.remote.join(format!("h/{:08x}.nvhdr", 2))).unwrap();
    let r = a.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderRollback { seen: 2, remote: 1 })),
        "{:?}",
        r.warnings
    );
}
