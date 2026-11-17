//! Regression tests for the internal security review (docs/SECURITY_REVIEW.md): integrity
//! detection must not be defeated by unauthenticated files, and malformed authenticated data
//! must never panic the engine.

mod common;

use common::*;
use nuvoletta_crypto::KdfParams;
use nuvoletta_sync::log::{Entry, KIND_CHECKPOINT, SeenPos, seal_segment};
use nuvoletta_sync::{Hash32, Id16, Warning};
use std::collections::BTreeMap;

/// M1: a junk file under a protocol segment name must not hide a rollback.
#[test]
fn junk_segment_does_not_suppress_the_rollback_warning() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.create("one", "p", "1");
    a.sync();
    let snapshot = all_files(&w.remote);
    a.create("two", "p", "2");
    a.sync();
    b.sync();
    for (k, _) in all_files(&w.remote) {
        if !snapshot.contains_key(&k) {
            std::fs::remove_file(w.remote.join(&k)).unwrap();
        }
    }
    let dev = a.syncer.device_id();
    let junk = w.remote.join(format!("d/{dev}/l/00000000000000ff.nvlog"));
    std::fs::write(&junk, b"junk").unwrap();
    let mut all = Vec::new();
    for _ in 0..4 {
        all.extend(b.sync().warnings);
    }
    assert!(
        all.iter().any(|x| matches!(x, Warning::Rollback { device, known_seq: 2, remote_seq: 1 } if *device == dev)),
        "{all:?}"
    );
    // L1: the persistent junk file itself is reported, once it stays unreadable.
    assert!(
        all.iter().any(
            |x| matches!(x, Warning::SegmentUnreadable { device, first_seq: 255 } if *device == dev)
        ),
        "{all:?}"
    );
}

/// L1: a newest segment that never authenticates is reported after a few cycles, not at once.
#[test]
fn a_persistently_unreadable_newest_segment_is_reported() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.create("one", "p", "1");
    a.sync();
    b.sync();
    let dev = a.syncer.device_id();
    std::fs::write(
        w.remote.join(format!("d/{dev}/l/0000000000000009.nvlog")),
        b"truncated",
    )
    .unwrap();
    let first = b.sync();
    assert!(
        !first
            .warnings
            .iter()
            .any(|x| matches!(x, Warning::SegmentUnreadable { .. }))
    );
    let mut seen = false;
    for _ in 0..3 {
        seen |= b
            .sync()
            .warnings
            .iter()
            .any(|x| matches!(x, Warning::SegmentUnreadable { first_seq: 9, .. }));
    }
    assert!(seen);
    // Once the file is gone the warning goes away.
    std::fs::remove_file(w.remote.join(format!("d/{dev}/l/0000000000000009.nvlog"))).unwrap();
    assert!(
        !b.sync()
            .warnings
            .iter()
            .any(|x| matches!(x, Warning::SegmentUnreadable { .. }))
    );
}

fn checkpoint_with(seen: BTreeMap<Id16, SeenPos>) -> Entry {
    let mut c = Entry::body(KIND_CHECKPOINT, 0);
    c.seen = Some(seen);
    c
}

fn plant(w: &World, a: &Device, dev: Id16, entries: Vec<Entry>) {
    let seg = seal_segment(a.vault.keys(), dev, 1, Hash32::ZERO, entries).unwrap();
    let dir = w.remote.join(format!("d/{dev}/l"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("0000000000000001.nvlog"), seg.bytes).unwrap();
}

/// M4: a checkpoint claiming sequence number 0 is invalid and never panics any reader, on every
/// cycle and after a restart (the claim must not be stored).
#[test]
fn checkpoint_seen_seq_zero_is_rejected_not_a_panic() {
    let mut w = World::new();
    let a = w.first("a");
    let mut b = w.join("b", &a);
    let dev = Id16([0x55; 16]);
    let mut seen = BTreeMap::new();
    for x in [0x66u8, 0x77] {
        seen.insert(
            Id16([x; 16]),
            SeenPos {
                seq: 0,
                hash: Hash32::ZERO,
            },
        );
    }
    plant(&w, &a, dev, vec![checkpoint_with(seen)]);
    for round in 0..3 {
        if round == 2 {
            b.restart();
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.try_sync()))
            .expect("sync must not panic")
            .expect("sync must not fail");
        if round == 0 {
            assert!(
                r.invalid.iter().any(|i| i.device == dev && i.seq == 1),
                "{:?}",
                r.invalid
            );
        }
    }
}

/// M4: a zero claim about a device this reader knows must not underflow either.
#[test]
fn checkpoint_seen_seq_zero_about_a_known_device_is_harmless() {
    let mut w = World::new();
    let mut a = w.first("a");
    let mut b = w.join("b", &a);
    a.create("one", "p", "1");
    a.sync();
    b.sync();
    let mut seen = BTreeMap::new();
    seen.insert(
        a.syncer.device_id(),
        SeenPos {
            seq: 0,
            hash: Hash32::ZERO,
        },
    );
    plant(&w, &a, Id16([0x55; 16]), vec![checkpoint_with(seen)]);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.try_sync()))
        .expect("sync must not panic")
        .expect("sync must not fail");
    assert!(
        !r.warnings
            .iter()
            .any(|x| matches!(x, Warning::DivergentCheckpoint { .. }))
    );
}

/// M2: a junk header file under a protocol name is invalid and neither raises "newer" nor
/// poisons the recorded generation.
#[test]
fn junk_header_file_is_invalid_not_newer_and_does_not_poison_state() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.sync();
    std::fs::write(w.remote.join("h/ffffffff.nvhdr"), b"junk").unwrap();
    let r = a.sync();
    assert!(
        r.warnings.iter().any(|x| matches!(
            x,
            Warning::HeaderInvalid {
                generation: 0xffff_ffff
            }
        )),
        "{:?}",
        r.warnings
    );
    assert!(
        !r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderNewer { .. }))
    );
    std::fs::remove_file(w.remote.join("h/ffffffff.nvhdr")).unwrap();
    a.restart();
    let r = a.sync();
    assert!(
        !r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderRollback { .. })),
        "{:?}",
        r.warnings
    );
}

/// M2: a structurally valid header of the same vault id and a higher generation but with a bad
/// MAC (forged by someone without the DEK) is invalid, not newer.
#[test]
fn forged_header_with_a_bad_mac_is_not_newer() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.sync();
    let mut forged = std::fs::read(w.remote.join("h/00000001.nvhdr")).unwrap();
    forged[10..14].copy_from_slice(&7u32.to_be_bytes());
    std::fs::write(w.remote.join("h/00000007.nvhdr"), &forged).unwrap();
    let r = a.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderInvalid { generation: 7 }))
    );
    assert!(
        !r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderNewer { .. }))
    );
}

/// M2: junk squatting on the name of the next generation must not stop the genuine new header
/// from being recognised as newer by other devices once it is published under its own name.
/// A junk file on the device's own generation name does not count as "published".
#[test]
fn junk_on_the_local_generation_name_is_not_taken_as_the_published_header() {
    let mut w = World::new();
    let mut a = w.first("a");
    std::fs::create_dir_all(w.remote.join("h")).unwrap();
    std::fs::write(w.remote.join("h/00000001.nvhdr"), b"junk").unwrap();
    let r = a.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderInvalid { generation: 1 })),
        "{:?}",
        r.warnings
    );
}

/// ADR 0008, O6: two devices change the passphrase at once. The second to publish finds the
/// name of its generation taken by a header that authenticates under the same keys but has
/// different bytes. That is reported as a divergence (not as an invalid file), it is not counted
/// as a newer generation, and nothing is overwritten.
#[test]
fn a_lost_race_on_the_header_name_is_reported_as_diverged_not_invalid() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.sync();
    let mut b = w.join("b", &a);
    a.vault
        .change_passphrase("passphrase set on device a", KdfParams::MIN_ACCEPTED)
        .unwrap();
    b.vault
        .change_passphrase("passphrase set on device b", KdfParams::MIN_ACCEPTED)
        .unwrap();
    assert_eq!(a.vault.header().generation(), b.vault.header().generation());
    a.sync();
    let winner = std::fs::read(w.remote.join("h/00000002.nvhdr")).unwrap();
    let r = b.sync();
    assert!(
        r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderDiverged { generation: 2 })),
        "{:?}",
        r.warnings
    );
    assert!(
        !r.warnings.iter().any(|x| matches!(
            x,
            Warning::HeaderInvalid { .. } | Warning::HeaderNewer { .. }
        )),
        "{:?}",
        r.warnings
    );
    // The winner's file was not touched.
    assert_eq!(
        std::fs::read(w.remote.join("h/00000002.nvhdr")).unwrap(),
        winner
    );
}

/// M2: state written before the fix (`max-header-gen`, possibly poisoned) is dropped on load.
#[test]
fn a_legacy_max_header_gen_file_is_discarded() {
    let mut w = World::new();
    let mut a = w.first("a");
    a.sync();
    let state = std::fs::read_dir(&a.dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.join("device-id").exists())
        .or_else(|| find_state_dir(&a.dir))
        .expect("state dir");
    std::fs::write(state.join("max-header-gen"), b"poisoned").unwrap();
    a.restart();
    assert!(!state.join("max-header-gen").exists());
    let r = a.sync();
    assert!(
        !r.warnings
            .iter()
            .any(|x| matches!(x, Warning::HeaderRollback { .. }))
    );
}

fn find_state_dir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.join("device-id").exists() {
                return Some(p);
            }
            if let Some(f) = find_state_dir(&p) {
                return Some(f);
            }
        }
    }
    None
}
