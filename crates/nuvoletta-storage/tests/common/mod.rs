//! Shared helpers for the storage integration tests.
#![allow(dead_code)]

use nuvoletta_core::Note;
use nuvoletta_crypto::KdfParams;
use nuvoletta_crypto::RecoveryKey;
use nuvoletta_storage::{Locked, Unlocked};
use std::fs;
use std::path::{Path, PathBuf};

pub const PASSPHRASE: &str = "correct horse battery staple";
/// Cheapest accepted Argon2 parameters, so tests stay fast.
pub const FAST: KdfParams = KdfParams::MIN_ACCEPTED;

pub fn create(root: &Path) -> (Unlocked, RecoveryKey) {
    Locked::create(root, PASSPHRASE, FAST).unwrap()
}

pub fn note(title: &str, path: &str, body: &str) -> Note {
    Note::new(title, path, vec!["tag-one".into(), "tag-two".into()], body).unwrap()
}

/// Every file under `dir`, recursively (symlinks are listed, not followed).
pub fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            out.extend(all_files(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}
