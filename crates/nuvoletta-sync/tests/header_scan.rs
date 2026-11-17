//! Finding header candidates on a provider: grouping, ordering and what is ignored.

use nuvoletta_crypto::{KdfParams, Vault, VaultHeader};
use nuvoletta_sync::{FolderProvider, fingerprint, scan_headers};
use std::fs;
use std::path::Path;

const FAST: KdfParams = KdfParams::MIN_ACCEPTED;

fn put(root: &Path, name: &str, bytes: &[u8]) {
    let dir = root.join("h");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), bytes).unwrap();
}

fn file(generation: u32) -> String {
    format!("{generation:08x}.nvhdr")
}

fn next(v: &Vault, pass: &str) -> VaultHeader {
    v.change_passphrase(pass, FAST).unwrap()
}

#[test]
fn candidates_are_grouped_by_vault_and_sorted_newest_first() {
    let d = tempfile::tempdir().unwrap();
    let a = Vault::create("first vault passphrase", FAST).unwrap().vault;
    let b = Vault::create("second vault passphrase", FAST)
        .unwrap()
        .vault;
    let a2 = next(&a, "first vault passphrase 2");
    put(d.path(), &file(1), a.header().as_bytes());
    put(d.path(), &file(2), a2.as_bytes());
    // Another vault's header in the same folder (a shared folder, or an attacker's file). Its
    // generation is also 1, so its name would collide: it is stored as a stray copy.
    put(
        d.path(),
        "00000001.nvhdr (conflict).nvhdr",
        b.header().as_bytes(),
    );

    let scan = scan_headers(&FolderProvider::new(d.path()).unwrap()).unwrap();
    assert_eq!(scan.groups.len(), 2);
    let ga = scan
        .groups
        .iter()
        .find(|g| g.vault_id == a.header().vault_id())
        .unwrap();
    let gens: Vec<_> = ga.candidates.iter().map(VaultHeader::generation).collect();
    assert_eq!(gens, [2, 1]);
    assert_eq!(ga.newest_generation(), 2);
    let gb = scan
        .groups
        .iter()
        .find(|g| g.vault_id == b.header().vault_id())
        .unwrap();
    assert_eq!(gb.candidates.len(), 1);
}

#[test]
fn a_name_that_disagrees_with_the_content_is_ignored() {
    let d = tempfile::tempdir().unwrap();
    let a = Vault::create("first vault passphrase", FAST).unwrap().vault;
    // A forged name: generation 1 content under the name of generation 9.
    put(d.path(), &file(9), a.header().as_bytes());
    let scan = scan_headers(&FolderProvider::new(d.path()).unwrap()).unwrap();
    assert!(scan.groups.is_empty());
    assert_eq!(scan.ignored.len(), 1);
    assert_eq!(scan.ignored[0].reason, "name and content disagree");
}

#[test]
fn wrong_sizes_and_garbage_are_ignored_without_being_read_as_headers() {
    let d = tempfile::tempdir().unwrap();
    let a = Vault::create("first vault passphrase", FAST).unwrap().vault;
    let mut longer = a.header().as_bytes().to_vec();
    longer.push(0);
    put(d.path(), &file(1), &longer);
    put(d.path(), &file(2), &a.header().as_bytes()[..100]);
    let mut bad = *a.header().as_bytes();
    bad[0] ^= 0xff;
    put(d.path(), &file(3), &bad);
    put(d.path(), "readme.txt", b"hello");
    let scan = scan_headers(&FolderProvider::new(d.path()).unwrap()).unwrap();
    assert!(scan.groups.is_empty());
    let reasons: Vec<_> = scan.ignored.iter().map(|i| i.reason).collect();
    assert_eq!(reasons.iter().filter(|r| **r == "wrong size").count(), 2);
    assert!(reasons.contains(&"not a valid header"));
}

#[test]
fn two_different_headers_of_one_generation_are_both_kept_and_duplicates_once() {
    let d = tempfile::tempdir().unwrap();
    let a = Vault::create("first vault passphrase", FAST).unwrap().vault;
    let x = next(&a, "passphrase chosen on device X");
    let y = next(&a, "passphrase chosen on device Y");
    assert_eq!(x.generation(), y.generation());
    put(d.path(), &file(2), x.as_bytes());
    put(d.path(), "00000002.nvhdr (copy).nvhdr", y.as_bytes());
    put(d.path(), "00000002.nvhdr (again).nvhdr", y.as_bytes());
    let scan = scan_headers(&FolderProvider::new(d.path()).unwrap()).unwrap();
    assert_eq!(scan.groups.len(), 1);
    assert_eq!(scan.groups[0].at_generation(2).count(), 2);
}

#[test]
fn a_missing_h_directory_is_an_empty_scan() {
    let d = tempfile::tempdir().unwrap();
    let scan = scan_headers(&FolderProvider::new(d.path()).unwrap()).unwrap();
    assert!(scan.groups.is_empty() && scan.ignored.is_empty());
}

#[test]
fn the_fingerprint_is_the_vault_id_and_nothing_else() {
    let a = Vault::create("first vault passphrase", FAST).unwrap().vault;
    let fp = fingerprint(&a.header().vault_id());
    let compact: String = fp.split(' ').collect();
    let expected: String = a
        .header()
        .vault_id()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(compact, expected);
    assert_eq!(fp.split(' ').count(), 8);
}

#[test]
fn the_verified_generation_only_goes_up_and_older_headers_are_refused() {
    use nuvoletta_sync::state::{
        ensure_not_rolled_back, record_verified_header_gen, verified_header_gen,
    };
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("sync-state");
    let a = Vault::create("first vault passphrase", FAST).unwrap().vault;
    let other = Vault::create("second vault passphrase", FAST)
        .unwrap()
        .vault;
    assert_eq!(verified_header_gen(&state, a.keys()), 0);
    ensure_not_rolled_back(&state, a.keys(), 1).unwrap();
    record_verified_header_gen(&state, a.keys(), 5).unwrap();
    record_verified_header_gen(&state, a.keys(), 3).unwrap();
    assert_eq!(verified_header_gen(&state, a.keys()), 5);
    ensure_not_rolled_back(&state, a.keys(), 5).unwrap();
    ensure_not_rolled_back(&state, a.keys(), 6).unwrap();
    assert!(ensure_not_rolled_back(&state, a.keys(), 4).is_err());
    assert_eq!(verified_header_gen(&state, other.keys()), 0);
}
