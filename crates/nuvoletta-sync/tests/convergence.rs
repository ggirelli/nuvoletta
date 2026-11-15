//! Property tests: whatever the interleaving of local edits, syncs and delayed propagation,
//! once everything has been delivered all devices have identical heads for every note, no
//! revision is lost and no remote file is ever modified or deleted.

mod common;

use common::*;
use nuvoletta_core::NoteId;
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Create(usize),
    EditBody(usize, usize, u8),
    Rename(usize, usize, u8),
    Retag(usize, usize, u8),
    Delete(usize, usize),
    /// Sync a device while a pseudo-random third of the remote files is not visible to it.
    Sync(usize, u8),
}

fn op(devices: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => (0..devices).prop_map(Op::Create),
        3 => (0..devices, 0..8usize, any::<u8>()).prop_map(|(d, n, v)| Op::EditBody(d, n, v)),
        1 => (0..devices, 0..8usize, any::<u8>()).prop_map(|(d, n, v)| Op::Rename(d, n, v)),
        1 => (0..devices, 0..8usize, any::<u8>()).prop_map(|(d, n, v)| Op::Retag(d, n, v)),
        1 => (0..devices, 0..8usize).prop_map(|(d, n)| Op::Delete(d, n)),
        4 => (0..devices, any::<u8>()).prop_map(|(d, s)| Op::Sync(d, s)),
    ]
}

fn local_notes(d: &Device) -> Vec<NoteId> {
    d.vault.list().into_iter().map(|s| s.id).collect()
}

fn path(v: u8) -> nuvoletta_core::NotePath {
    nuvoletta_core::NotePath::parse(&format!("dir{}/n", v % 4)).unwrap()
}

fn hide(d: &Device, remote: &std::path::Path, seed: u8) {
    let names: Vec<String> = all_files(remote)
        .into_keys()
        .filter(|k| {
            let h = k.bytes().fold(u32::from(seed), |a, b| {
                a.wrapping_mul(31).wrapping_add(u32::from(b))
            });
            h % 3 == 0
        })
        .collect();
    d.faults().lock().unwrap().hidden = names;
}

fn run(devices: usize, ops: &[Op]) {
    let mut w = World::new();
    let names = ["d0", "d1", "d2", "d3"];
    let mut devs: Vec<Device> = Vec::new();
    devs.push(w.first(names[0]));
    for name in names.iter().take(devices).skip(1) {
        let d = w.join(name, &devs[0]);
        devs.push(d);
    }
    let remote = w.remote.clone();
    for (i, o) in ops.iter().enumerate() {
        match o {
            Op::Create(d) => {
                devs[*d].create(&format!("n{i}"), "start", &format!("body {i}"));
            }
            Op::EditBody(d, n, v) => {
                let notes = local_notes(&devs[*d]);
                if let Some(id) = notes.get(n % notes.len().max(1)) {
                    devs[*d].set_body(id, &format!("edit {i} {v}"));
                }
            }
            Op::Rename(d, n, v) => {
                let notes = local_notes(&devs[*d]);
                if let Some(id) = notes.get(n % notes.len().max(1)) {
                    devs[*d].edit(id, |x| x.path = path(*v));
                }
            }
            Op::Retag(d, n, v) => {
                let notes = local_notes(&devs[*d]);
                if let Some(id) = notes.get(n % notes.len().max(1)) {
                    devs[*d].edit(id, |x| x.tags = vec![format!("t{}", v % 3)]);
                }
            }
            Op::Delete(d, n) => {
                let notes = local_notes(&devs[*d]);
                if let Some(id) = notes.get(n % notes.len().max(1)) {
                    devs[*d].delete(id);
                }
            }
            Op::Sync(d, seed) => {
                hide(&devs[*d], &remote, *seed);
                devs[*d].sync();
                devs[*d].faults().lock().unwrap().hidden.clear();
                w.check_immutable();
            }
        }
    }
    // Everything is delivered now.
    let mut refs: Vec<&mut Device> = devs.iter_mut().collect();
    w.converge(&mut refs);
    let refs: Vec<&Device> = devs.iter().collect();
    assert_converged(&refs);
    assert_no_revision_lost(&remote, &refs);
    // Every device knows exactly the same set of revisions.
    let known = |d: &Device| {
        let mut v: Vec<_> = d.syncer.known_revisions().into_iter().collect();
        v.sort();
        v
    };
    for d in &refs[1..] {
        assert_eq!(
            known(refs[0]),
            known(d),
            "revision sets differ on {}",
            d.name
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn two_devices_converge_to_identical_heads(ops in proptest::collection::vec(op(2), 1..14)) {
        run(2, &ops);
    }

    #[test]
    fn three_devices_converge_to_identical_heads(ops in proptest::collection::vec(op(3), 1..16)) {
        run(3, &ops);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    #[ignore = "expensive: run by `just test-slow`"]
    fn four_devices_converge_under_long_random_histories(ops in proptest::collection::vec(op(4), 10..48)) {
        run(4, &ops);
    }
}
