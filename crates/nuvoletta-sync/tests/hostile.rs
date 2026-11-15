//! Hostile or broken remote files: authenticated but malformed entries, mismatched names and
//! scopes, garbage. Readers report, skip and continue; they never panic, apply or delete.

mod common;

use common::*;
use nuvoletta_sync::log::{Entry, KIND_CHECKPOINT, KIND_DELETE, KIND_PUT, seal_segment};
use nuvoletta_sync::{Hash32, Id16, StrayKind};

fn put(note: u8, rev: u8, parents: Vec<Id16>) -> Entry {
    let mut e = Entry::body(KIND_PUT, 0);
    e.note_id = Some(Id16([note; 16]));
    e.rev_id = Some(Id16([rev; 16]));
    e.ct_hash = Some(Hash32([rev; 32]));
    e.parents = Some(parents);
    e
}

fn write_segment(
    w: &World,
    a: &Device,
    device: Id16,
    name_seq: u64,
    first_seq: u64,
    prev: Hash32,
    bodies: Vec<Entry>,
) {
    let seg = seal_segment(a.vault.keys(), device, first_seq, prev, bodies).unwrap();
    let dir = w.remote.join(format!("d/{device}/l"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name_seq:016x}.nvlog")), seg.bytes).unwrap();
}

#[test]
fn semantically_invalid_entries_are_reported_and_skipped_without_stopping_the_stream() {
    let mut w = World::new();
    let a = w.first("a");
    let mut b = w.join("b", &a);
    let dev = Id16([0x77; 16]);
    let mut no_hash = put(1, 10, vec![]);
    no_hash.ct_hash = None;
    let mut delete_no_parents = Entry::body(KIND_DELETE, 0);
    delete_no_parents.note_id = Some(Id16([1; 16]));
    delete_no_parents.rev_id = Some(Id16([11; 16]));
    let mut unknown = Entry::body(9, 0);
    unknown.note_id = Some(Id16([1; 16]));
    write_segment(
        &w,
        &a,
        dev,
        1,
        1,
        Hash32::ZERO,
        vec![
            no_hash,
            delete_no_parents,
            unknown,
            put(2, 20, vec![]),
            // The same revision id claimed for another note, then redefined with other parents.
            put(3, 20, vec![]),
            put(2, 20, vec![Id16([99; 16])]),
            Entry::body(KIND_CHECKPOINT, 0),
        ],
    );
    let r = b.sync();
    let reasons: Vec<_> = r.invalid.iter().map(|i| (i.seq, i.reason)).collect();
    assert_eq!(reasons.len(), 5, "{reasons:?}");
    assert_eq!(
        reasons.iter().map(|r| r.0).collect::<Vec<_>>(),
        vec![1, 2, 3, 5, 6]
    );
    // The one valid entry (seq 4) was applied: its object is simply missing (dangling).
    assert_eq!(
        b.syncer.heads(&nuvoletta_core::NoteId::from_bytes([2; 16])),
        vec![Id16([20; 16])]
    );
    assert!(
        b.syncer
            .heads(&nuvoletta_core::NoteId::from_bytes([3; 16]))
            .is_empty()
    );
    assert_eq!(
        b.syncer.peers().iter().find(|p| p.0 == dev).unwrap().1,
        7,
        "the chain advanced past the invalid entries"
    );
}

#[test]
fn a_segment_whose_name_or_directory_lies_is_invalid_and_never_applied() {
    let mut w = World::new();
    let a = w.first("a");
    let mut b = w.join("b", &a);
    let dev = Id16([0x77; 16]);
    // The name says first_seq 5, the content says 1.
    write_segment(&w, &a, dev, 5, 1, Hash32::ZERO, vec![put(2, 20, vec![])]);
    // The directory says device 0x88, the authenticated scope says 0x99.
    write_segment(
        &w,
        &a,
        Id16([0x99; 16]),
        1,
        1,
        Hash32::ZERO,
        vec![put(4, 40, vec![])],
    );
    let moved = w.remote.join(format!("d/{}/l", Id16([0x88; 16])));
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::rename(
        w.remote
            .join(format!("d/{}/l/{:016x}.nvlog", Id16([0x99; 16]), 1)),
        moved.join(format!("{:016x}.nvlog", 1)),
    )
    .unwrap();
    let r = b.sync();
    assert_eq!(r.invalid.len(), 2, "{:?}", r.invalid);
    assert_eq!(r.entries_applied, 0);
    assert!(b.syncer.known_revisions().is_empty());
}

#[test]
fn garbage_and_foreign_vault_files_are_not_yet_arrived_not_errors() {
    let mut w = World::new();
    let a = w.first("a");
    let mut b = w.join("b", &a);
    let dev = Id16([0x77; 16]);
    let dir = w.remote.join(format!("d/{dev}/l"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{:016x}.nvlog", 1)),
        b"not an envelope at all",
    )
    .unwrap();
    // A well-formed segment of a different vault.
    let other = World::new().first("other");
    let seg = seal_segment(
        other.vault.keys(),
        dev,
        1,
        Hash32::ZERO,
        vec![put(2, 20, vec![])],
    )
    .unwrap();
    std::fs::write(dir.join(format!("{:016x}.nvlog", 2)), seg.bytes).unwrap();
    // Garbage objects and copies.
    let o = w.remote.join("o/ab");
    std::fs::create_dir_all(&o).unwrap();
    std::fs::write(o.join(format!("{} (1).nvobj", "ab".repeat(16))), b"junk").unwrap();
    for _ in 0..3 {
        let r = b.sync();
        assert_eq!(r.entries_applied, 0);
        assert!(r.invalid.is_empty());
    }
    let r = b.sync();
    assert!(r.strays.iter().all(|s| matches!(
        s.kind,
        StrayKind::NotYetArrived | StrayKind::CopyOnly | StrayKind::Ignored
    )));
}

#[test]
fn an_object_with_the_right_name_but_the_wrong_content_is_never_installed() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    let n1 = a.create("one", "p", "1");
    let n2 = a.create("two", "p", "2");
    a.sync();
    // Swap the two objects' contents on the remote: authentic envelopes, wrong names.
    let files = all_files(&w.remote);
    let objs: Vec<_> = files
        .keys()
        .filter(|k| k.starts_with("o/"))
        .cloned()
        .collect();
    assert_eq!(objs.len(), 2);
    std::fs::write(w.remote.join(&objs[0]), &files[&objs[1]]).unwrap();
    std::fs::write(w.remote.join(&objs[1]), &files[&objs[0]]).unwrap();
    let r = b.sync();
    assert_eq!(r.objects_fetched, 0);
    assert!(!b.exists(&n1) && !b.exists(&n2));
}
