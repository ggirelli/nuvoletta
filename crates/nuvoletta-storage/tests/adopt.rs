//! Adopting a newer vault header: refusals and the locked-device path.

mod common;

use common::{PASSPHRASE, create, note};
use nuvoletta_crypto::{ObjType, ObjectMeta, VaultHeader, seal};
use nuvoletta_storage::layout::{HEADER_FILE, META_DIR, NOTES_DIR};
use nuvoletta_storage::{Error, Locked, Refusal, Secret};
use std::fs;

const NEW_PW: &str = "the passphrase set on the other device";
const FAST: nuvoletta_crypto::KdfParams = common::FAST;

fn refused(e: Result<impl std::fmt::Debug, Error>) -> Refusal {
    match e {
        Err(Error::HeaderRefused(r)) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A second device of the same vault: a copy of the vault directory.
fn other_device(origin: &std::path::Path, copy: &std::path::Path) {
    fn cp(a: &std::path::Path, b: &std::path::Path) {
        fs::create_dir_all(b).unwrap();
        for e in fs::read_dir(a).unwrap() {
            let e = e.unwrap();
            let to = b.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                cp(&e.path(), &to);
            } else if e.file_name() != "lock" {
                fs::copy(e.path(), to).unwrap();
            }
        }
    }
    cp(origin, copy);
}

#[test]
fn an_unlocked_device_adopts_a_newer_header_and_keeps_working() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (mut dev_a, _rk) = create(a.path());
    let n = note("t", "p", "body");
    dev_a.create_note(&n).unwrap();
    other_device(a.path(), b.path());
    dev_a.change_passphrase(NEW_PW, FAST).unwrap();
    let newer = dev_a.header().clone();
    assert_eq!(newer.generation(), 2);
    drop(dev_a);

    let mut dev_b = Locked::open(b.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    let verified = dev_b
        .verify_header(&newer, &Secret::Passphrase(NEW_PW))
        .unwrap();
    assert_eq!(verified.generation(), 2);
    // Nothing written until adopt.
    let path = b.path().join(META_DIR).join(HEADER_FILE);
    assert_ne!(fs::read(&path).unwrap(), newer.as_bytes());
    dev_b.adopt_header(verified).unwrap();
    assert_eq!(fs::read(&path).unwrap(), newer.as_bytes());
    assert_eq!(dev_b.header().generation(), 2);
    assert_eq!(dev_b.read_note(&n.id).unwrap().body, "body");
    // The session keeps working with the same keys.
    let mut changed = dev_b.read_note(&n.id).unwrap();
    changed.body = "edited after adoption".into();
    dev_b.update_note(&changed).unwrap();
    drop(dev_b.lock());
    assert!(
        Locked::open(b.path())
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .is_err()
    );
    assert!(
        Locked::open(b.path())
            .unwrap()
            .unlock_with_passphrase(NEW_PW)
            .is_ok()
    );
}

#[test]
fn an_older_replayed_the_same_or_another_vaults_header_is_refused() {
    let a = tempfile::tempdir().unwrap();
    let (mut dev, _rk) = create(a.path());
    let gen1 = dev.header().clone();
    dev.change_passphrase(NEW_PW, FAST).unwrap();
    // Generation 1 replayed to a device already at generation 2.
    assert_eq!(
        refused(dev.verify_header(&gen1, &Secret::Passphrase(PASSPHRASE))),
        Refusal::Older {
            local: 2,
            candidate: 1
        }
    );
    // The header it already holds.
    let cur = dev.header().clone();
    assert_eq!(
        refused(dev.verify_header(&cur, &Secret::Passphrase(NEW_PW))),
        Refusal::AlreadyCurrent
    );
    // A header of another vault, even one the typed passphrase opens.
    let o = tempfile::tempdir().unwrap();
    let (other, _) = create(o.path());
    let foreign = other.header().clone();
    assert_eq!(
        refused(dev.verify_header(&foreign, &Secret::Passphrase(PASSPHRASE))),
        Refusal::OtherVault
    );
    // The header on disk is untouched.
    let on_disk = fs::read(a.path().join(META_DIR).join(HEADER_FILE)).unwrap();
    assert_eq!(on_disk, cur.as_bytes());
}

#[test]
fn a_wrong_secret_or_tampered_candidate_is_the_generic_unlock_error() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (dev_a, _rk) = create(a.path());
    drop(dev_a);
    other_device(a.path(), b.path());
    // Device B changes the passphrase; device A has not seen it.
    let mut dev_b = Locked::open(b.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    dev_b.change_passphrase(NEW_PW, FAST).unwrap();
    let newer = dev_b.header().clone();
    drop(dev_b);
    let dev = Locked::open(a.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    // The old passphrase does not open the new header, nor does a wrong one.
    for secret in [
        Secret::Passphrase("not the passphrase"),
        Secret::Passphrase(PASSPHRASE),
    ] {
        assert!(matches!(
            dev.verify_header(&newer, &secret),
            Err(Error::Crypto(nuvoletta_crypto::Error::Unlock))
        ));
    }
    // A flipped byte anywhere in the candidate never verifies (MAC, wrap, parameters).
    for i in (0..236).step_by(5) {
        let mut bytes = *newer.as_bytes();
        bytes[i] ^= 1;
        if let Ok(h) = VaultHeader::parse(&bytes) {
            assert!(
                dev.verify_header(&h, &Secret::Passphrase(NEW_PW)).is_err(),
                "byte {i}"
            );
        }
    }
}

#[test]
fn a_locked_device_verifies_a_candidate_against_its_notes() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (mut dev_a, _rk) = create(a.path());
    dev_a.create_note(&note("t", "p", "body")).unwrap();
    other_device(a.path(), b.path());
    dev_a.change_passphrase(NEW_PW, FAST).unwrap();
    let newer = dev_a.header().clone();
    drop(dev_a);

    // Device B was never unlocked since the change: only the old header is on it.
    let mut locked = Locked::open(b.path()).unwrap();
    let verified = locked
        .verify_header(&newer, &Secret::Passphrase(NEW_PW))
        .unwrap();
    locked.adopt_header(verified).unwrap();
    assert_eq!(locked.header().generation(), 2);
    drop(locked);
    let unlocked = Locked::open(b.path())
        .unwrap()
        .unlock_with_passphrase(NEW_PW)
        .unwrap();
    assert_eq!(unlocked.list().len(), 1);
}

#[test]
fn a_locked_device_refuses_a_candidate_whose_keys_do_not_open_its_data() {
    let a = tempfile::tempdir().unwrap();
    let c = tempfile::tempdir().unwrap();
    let (mut dev_a, _rk) = create(a.path());
    dev_a.create_note(&note("t", "p", "body")).unwrap();
    // Device C is a copy taken before the change, but its notes were replaced by a note sealed
    // under another vault's keys (the candidate's DEK is not the DEK of that data).
    other_device(a.path(), c.path());
    dev_a.change_passphrase(NEW_PW, FAST).unwrap();
    let newer = dev_a.header().clone();
    drop(dev_a);
    let foreign = tempfile::tempdir().unwrap();
    let (mut f, _) = create(foreign.path());
    let fnote = note("f", "f", "foreign");
    f.create_note(&fnote).unwrap();
    let bytes = f.read_revision_bytes(&fnote.id).unwrap();
    drop(f);
    let notes = c.path().join(NOTES_DIR);
    for e in fs::read_dir(&notes).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    fs::write(notes.join(format!("{}.nvnote", fnote.id)), bytes).unwrap();
    let locked = Locked::open(c.path()).unwrap();
    assert_eq!(
        refused(locked.verify_header(&newer, &Secret::Passphrase(NEW_PW))),
        Refusal::DifferentKeys
    );
}

#[test]
fn a_locked_device_with_nothing_sealed_cannot_check_and_refuses() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (mut dev_a, _rk) = create(a.path());
    other_device(a.path(), b.path());
    dev_a.change_passphrase(NEW_PW, FAST).unwrap();
    let newer = dev_a.header().clone();
    drop(dev_a);
    let locked = Locked::open(b.path()).unwrap();
    assert_eq!(
        refused(locked.verify_header(&newer, &Secret::Passphrase(NEW_PW))),
        Refusal::NothingToCheck
    );
    drop(locked);
    // A K_local-sealed state file is enough to check against.
    let vault = Locked::open(b.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    let state = b.path().join(META_DIR).join("sync-state");
    fs::create_dir_all(&state).unwrap();
    let sealed = seal(
        vault.keys(),
        &ObjectMeta::new(ObjType::LocalState, [0; 16]).unwrap(),
        b"state",
    )
    .unwrap();
    fs::write(state.join("peers"), sealed).unwrap();
    drop(vault);
    let mut locked = Locked::open(b.path()).unwrap();
    let verified = locked
        .verify_header(&newer, &Secret::Passphrase(NEW_PW))
        .unwrap();
    locked.adopt_header(verified).unwrap();
}

#[test]
fn adopting_when_the_header_on_disk_changed_is_refused() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (mut dev_a, _rk) = create(a.path());
    dev_a.create_note(&note("t", "p", "body")).unwrap();
    other_device(a.path(), b.path());
    dev_a.change_passphrase(NEW_PW, FAST).unwrap();
    let newer = dev_a.header().clone();
    drop(dev_a);
    let mut dev_b = Locked::open(b.path())
        .unwrap()
        .unlock_with_passphrase(PASSPHRASE)
        .unwrap();
    let verified = dev_b
        .verify_header(&newer, &Secret::Passphrase(NEW_PW))
        .unwrap();
    // Something else replaced the header file in the meantime.
    fs::write(b.path().join(META_DIR).join(HEADER_FILE), newer.as_bytes()).unwrap();
    assert!(matches!(
        dev_b.adopt_header(verified),
        Err(Error::HeaderChanged)
    ));
}
