//! Test harness: in-process vault instances syncing through a shared folder, with a
//! fault-injecting provider wrapper.
#![allow(dead_code)]

use nuvoletta_core::{Note, NoteId, now_ms};
use nuvoletta_crypto::KdfParams;
use nuvoletta_storage::{Locked, Unlocked};
use nuvoletta_sync::{
    Capabilities, FolderProvider, Provider, ProviderError, RemoteEntry, RevId, SyncConfig,
    SyncReport, Syncer,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const PASSPHRASE: &str = "correct horse battery staple";
/// Cheapest accepted Argon2 parameters, so tests stay fast.
pub const FAST: KdfParams = KdfParams::MIN_ACCEPTED;

/// Faults injected between one device and the shared folder. Names are matched by substring.
#[derive(Default)]
pub struct Faults {
    /// Invisible to `list` and `get` (delayed propagation, reordering).
    pub hidden: Vec<String>,
    /// `get` returns half of the bytes and `list` reports half the size (truncation).
    pub truncated: Vec<String>,
    /// `put_new` reports success but writes nothing (dropped upload), this many times.
    pub drop_puts: Vec<(String, usize)>,
    /// After a successful `put_new`, also create a duplicate named `<stem> (1).<ext>`.
    pub duplicate_puts: bool,
    /// Every operation from this count on fails with a transient error (a crash or outage).
    pub fail_from_op: Option<usize>,
    /// Operations performed so far.
    pub ops: usize,
    /// Mutating operations seen: `put:<name>` or `delete:<name>`.
    pub mutations: Vec<String>,
    /// Names requested with `get`, in order.
    pub gets: Vec<String>,
}

#[derive(Clone)]
pub struct Faulty {
    inner: FolderProvider,
    pub faults: Arc<Mutex<Faults>>,
}

impl Faulty {
    pub fn new(root: &Path) -> Self {
        Self {
            inner: FolderProvider::new(root).unwrap(),
            faults: Arc::new(Mutex::new(Faults::default())),
        }
    }

    fn tick(&self) -> Result<(), ProviderError> {
        let mut f = self.faults.lock().unwrap();
        f.ops += 1;
        match f.fail_from_op {
            Some(n) if f.ops >= n => Err(ProviderError::Transient("injected outage".into())),
            _ => Ok(()),
        }
    }
}

fn matches(list: &[String], name: &str) -> bool {
    list.iter().any(|p| name.contains(p.as_str()))
}

impl Provider for Faulty {
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError> {
        self.tick()?;
        let f = self.faults.lock().unwrap();
        Ok(self
            .inner
            .list(prefix)?
            .into_iter()
            .filter(|e| !matches(&f.hidden, &e.name))
            .map(|mut e| {
                if matches(&f.truncated, &e.name) {
                    e.size /= 2;
                }
                e
            })
            .collect())
    }

    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError> {
        self.tick()?;
        let mut f = self.faults.lock().unwrap();
        f.gets.push(name.to_owned());
        if matches(&f.hidden, name) {
            return Err(ProviderError::NotFound(name.to_owned()));
        }
        let mut bytes = self.inner.get(name)?;
        if matches(&f.truncated, name) {
            bytes.truncate(bytes.len() / 2);
        }
        Ok(bytes)
    }

    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        self.tick()?;
        let mut f = self.faults.lock().unwrap();
        f.mutations.push(format!("put:{name}"));
        if let Some((_, n)) = f
            .drop_puts
            .iter_mut()
            .find(|(p, n)| name.contains(p.as_str()) && *n > 0)
        {
            *n -= 1;
            return Ok(());
        }
        self.inner.put_new(name, bytes)?;
        if f.duplicate_puts {
            let (stem, ext) = name.rsplit_once('.').unwrap();
            let dup = format!("{stem} (1).{ext}");
            let root = self.inner.root();
            fs::write(root.join(dup), bytes).unwrap();
        }
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), ProviderError> {
        self.tick()?;
        self.faults
            .lock()
            .unwrap()
            .mutations
            .push(format!("delete:{name}"));
        self.inner.delete(name)
    }
}

/// One vault instance with its own sync state, talking to the shared folder.
pub struct Device {
    pub name: &'static str,
    pub dir: PathBuf,
    pub vault: Unlocked,
    pub syncer: Syncer<Faulty>,
    pub remote: PathBuf,
}

pub fn config() -> SyncConfig {
    SyncConfig {
        checkpoint_interval_ms: 0,
        retries_before_warning: 2,
        ..SyncConfig::default()
    }
}

impl Device {
    pub fn faults(&self) -> Arc<Mutex<Faults>> {
        self.syncer.provider().faults.clone()
    }

    pub fn sync(&mut self) -> SyncReport {
        self.try_sync()
            .unwrap_or_else(|e| panic!("{} sync failed: {e}", self.name))
    }

    pub fn try_sync(&mut self) -> nuvoletta_sync::Result<SyncReport> {
        self.syncer.sync(&mut self.vault)
    }

    /// Simulate a process restart: drop the syncer and reload everything from disk.
    pub fn restart(&mut self) {
        let provider = Faulty::new(&self.remote);
        let locked = Locked::open(&self.dir).unwrap();
        self.vault = locked.unlock_with_passphrase(PASSPHRASE).unwrap();
        self.syncer = Syncer::open_with(&self.vault, provider, config()).unwrap();
    }

    pub fn create(&mut self, title: &str, path: &str, body: &str) -> NoteId {
        let n = Note::new(title, path, vec![], body).unwrap();
        self.vault.create_note(&n).unwrap();
        n.id
    }

    pub fn note(&self, id: &NoteId) -> Note {
        self.vault.read_note(id).unwrap()
    }

    pub fn body(&self, id: &NoteId) -> Option<String> {
        self.vault.read_note(id).ok().map(|n| n.body.clone())
    }

    pub fn edit(&mut self, id: &NoteId, f: impl FnOnce(&mut Note)) {
        let mut n = self.note(id);
        f(&mut n);
        n.modified_at = now_ms();
        self.vault.update_note(&n).unwrap();
    }

    pub fn set_body(&mut self, id: &NoteId, body: &str) {
        self.edit(id, |n| n.body = body.to_owned());
    }

    pub fn delete(&mut self, id: &NoteId) {
        self.vault.delete_note(id).unwrap();
    }

    pub fn heads(&self, id: &NoteId) -> Vec<RevId> {
        self.syncer.heads(id)
    }

    /// Collect garbage as if it were `days` days later than now.
    pub fn gc(&mut self, days: i64, dry_run: bool) -> nuvoletta_sync::GcReport {
        self.try_gc(days, dry_run)
            .unwrap_or_else(|e| panic!("{} gc failed: {e}", self.name))
    }

    pub fn try_gc(
        &mut self,
        days: i64,
        dry_run: bool,
    ) -> nuvoletta_sync::Result<nuvoletta_sync::GcReport> {
        let opts = nuvoletta_sync::GcOptions {
            dry_run,
            now_ms: now_ms() + days * 24 * 60 * 60 * 1000,
            ..nuvoletta_sync::GcOptions::default()
        };
        self.syncer.collect_garbage(&self.vault, &opts)
    }

    pub fn exists(&self, id: &NoteId) -> bool {
        self.vault.summary(id).is_some()
    }
}

pub struct World {
    pub tmp: tempfile::TempDir,
    pub remote: PathBuf,
    next: usize,
    seen_files: BTreeMap<String, Vec<u8>>,
}

pub fn all_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

impl World {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote");
        fs::create_dir_all(&remote).unwrap();
        Self {
            tmp,
            remote,
            next: 0,
            seen_files: BTreeMap::new(),
        }
    }

    fn device(&self, name: &'static str, dir: PathBuf, vault: Unlocked) -> Device {
        let provider = Faulty::new(&self.remote);
        let syncer = Syncer::open_with(&vault, provider, config()).unwrap();
        Device {
            name,
            dir,
            vault,
            syncer,
            remote: self.remote.clone(),
        }
    }

    /// The first device creates the vault.
    pub fn first(&mut self, name: &'static str) -> Device {
        let dir = self.tmp.path().join(name);
        let (vault, _rk) = Locked::create(&dir, PASSPHRASE, FAST).unwrap();
        self.next += 1;
        self.device(name, dir, vault)
    }

    /// Another device of the same vault: same header, empty notes.
    pub fn join(&mut self, name: &'static str, of: &Device) -> Device {
        let dir = self.tmp.path().join(name);
        fs::create_dir_all(dir.join(".nuvoletta")).unwrap();
        fs::copy(
            of.dir.join(".nuvoletta/vault-header"),
            dir.join(".nuvoletta/vault-header"),
        )
        .unwrap();
        let vault = Locked::open(&dir)
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .unwrap();
        self.next += 1;
        self.device(name, dir, vault)
    }

    /// A byte-for-byte clone of a device directory, including its sync state: the same device
    /// id used twice (a restored backup or a copied vault).
    pub fn clone_device(&mut self, name: &'static str, of: &Device) -> Device {
        let dir = self.tmp.path().join(name);
        copy_dir(&of.dir, &dir);
        let vault = Locked::open(&dir)
            .unwrap()
            .unlock_with_passphrase(PASSPHRASE)
            .unwrap();
        self.device(name, dir, vault)
    }

    /// Remote files never change and never disappear (single writer, immutable files).
    pub fn check_immutable(&mut self) {
        let now = all_files(&self.remote);
        for (name, bytes) in &self.seen_files {
            match now.get(name) {
                Some(b) => assert_eq!(b, bytes, "remote file {name} was modified"),
                None => panic!("remote file {name} disappeared"),
            }
        }
        self.seen_files = now;
    }

    /// Like `converge`, but remote files may legitimately disappear (garbage collection).
    pub fn converge_gc(&mut self, devs: &mut [&mut Device]) {
        for round in 0..16 {
            let mut changed = false;
            for d in devs.iter_mut() {
                let r = d.sync();
                changed |= r.intents_created
                    + r.objects_pushed
                    + r.segments_uploaded
                    + r.entries_applied
                    + r.objects_fetched
                    + r.fast_forwards
                    + r.auto_merges
                    > 0;
            }
            if !changed {
                return;
            }
            assert!(round < 15, "devices did not settle");
        }
    }

    /// Sync every device in turn until a whole round changes nothing.
    pub fn converge(&mut self, devs: &mut [&mut Device]) {
        for round in 0..12 {
            let mut changed = false;
            for d in devs.iter_mut() {
                let r = d.sync();
                self.check_immutable();
                changed |= r.intents_created
                    + r.objects_pushed
                    + r.segments_uploaded
                    + r.entries_applied
                    + r.objects_fetched
                    + r.fast_forwards
                    + r.auto_merges
                    > 0;
            }
            if !changed {
                return;
            }
            assert!(round < 11, "devices did not settle");
        }
    }
}

pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// All devices agree on the heads of every note and, where there is one head, on its content.
pub fn assert_converged(devs: &[&Device]) {
    let mut notes = BTreeSet::new();
    for d in devs {
        for (n, _, _) in d.syncer.known_revisions() {
            notes.insert(n);
        }
    }
    for n in &notes {
        let first = devs[0].heads(n);
        for d in &devs[1..] {
            assert_eq!(first, d.heads(n), "heads of {n} differ on {}", d.name);
        }
        if first.len() == 1 {
            let c0 = devs[0].vault.read_note(n).ok();
            for d in &devs[1..] {
                let c = d.vault.read_note(n).ok();
                assert_eq!(
                    c0.as_ref()
                        .map(|x| (&x.title, x.path.as_str(), &x.tags, &x.body)),
                    c.as_ref()
                        .map(|x| (&x.title, x.path.as_str(), &x.tags, &x.body)),
                    "content of {n} differs on {}",
                    d.name
                );
            }
        }
    }
}

/// No revision ever lost: every content revision in any device's graph has its object on the
/// remote, and the remote objects are all still there.
pub fn assert_no_revision_lost(remote: &Path, devs: &[&Device]) {
    let files = all_files(remote);
    for d in devs {
        for (_, rev, is_delete) in d.syncer.known_revisions() {
            if is_delete {
                continue;
            }
            let name = format!("o/{:02x}/{rev}.nvobj", rev.0[0]);
            assert!(
                files.contains_key(&name),
                "object {rev} missing from the remote"
            );
        }
    }
}
