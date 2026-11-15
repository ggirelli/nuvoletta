//! Shared helpers: the test server, provider construction and a vault "device" that syncs
//! through WebDAV.
#![allow(dead_code)]

pub use nuvoletta_webdav::testing::*;

use nuvoletta_core::{Note, NoteId, now_ms};
use nuvoletta_crypto::KdfParams;
use nuvoletta_storage::{Locked, Unlocked};
use nuvoletta_sync::{SyncConfig, SyncReport, Syncer};
use nuvoletta_webdav::{Options, RetryPolicy, WebDavConfig, WebDavProvider};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const PASSPHRASE: &str = "correct horse battery staple";
pub const FAST: KdfParams = KdfParams::MIN_ACCEPTED;

/// Timeouts and pauses small enough for tests.
pub fn fast_opts() -> Options {
    Options {
        connect_timeout: Duration::from_secs(5),
        response_timeout: Duration::from_secs(5),
        total_timeout: Duration::from_secs(10),
        retry: RetryPolicy {
            attempts: 3,
            max_wait: Duration::from_millis(500),
            base_wait: Duration::from_millis(10),
        },
    }
}

/// Configuration for `server` that trusts its self-signed certificate by pin.
pub fn cfg(server: &TestServer) -> WebDavConfig {
    WebDavConfig::new(&server.url(), USER, PASS, Some(server.fingerprint)).unwrap()
}

pub fn provider(server: &TestServer) -> WebDavProvider {
    WebDavProvider::new(cfg(server), fast_opts())
}

pub fn sync_config() -> SyncConfig {
    SyncConfig {
        checkpoint_interval_ms: 0,
        retries_before_warning: 2,
        partial_upload_grace_ms: 0,
    }
}

/// A vault instance with its own sync state, syncing through a shared WebDAV server.
pub struct Device {
    pub name: &'static str,
    pub dir: PathBuf,
    pub vault: Unlocked,
    pub syncer: Syncer<Arc<WebDavProvider>>,
}

impl Device {
    pub fn new(
        tmp: &tempfile::TempDir,
        name: &'static str,
        server: &TestServer,
        join: Option<&Device>,
    ) -> Self {
        let dir = tmp.path().join(name);
        let vault = match join {
            None => Locked::create(&dir, PASSPHRASE, FAST).unwrap().0,
            Some(of) => {
                fs::create_dir_all(dir.join(".nuvoletta")).unwrap();
                fs::copy(
                    of.dir.join(".nuvoletta/vault-header"),
                    dir.join(".nuvoletta/vault-header"),
                )
                .unwrap();
                Locked::open(&dir)
                    .unwrap()
                    .unlock_with_passphrase(PASSPHRASE)
                    .unwrap()
            }
        };
        let p = Arc::new(provider(server));
        let syncer = Syncer::open_with(&vault, p, sync_config()).unwrap();
        Self {
            name,
            dir,
            vault,
            syncer,
        }
    }

    pub fn sync(&mut self) -> SyncReport {
        self.try_sync()
            .unwrap_or_else(|e| panic!("{} sync failed: {e}", self.name))
    }

    pub fn try_sync(&mut self) -> nuvoletta_sync::Result<SyncReport> {
        self.syncer.sync(&mut self.vault)
    }

    pub fn create(&mut self, title: &str, path: &str, body: &str) -> NoteId {
        let n = Note::new(title, path, vec![], body).unwrap();
        self.vault.create_note(&n).unwrap();
        n.id
    }

    pub fn body(&self, id: &NoteId) -> Option<String> {
        self.vault.read_note(id).ok().map(|n| n.body.clone())
    }

    pub fn set_body(&mut self, id: &NoteId, body: &str) {
        let mut n = self.vault.read_note(id).unwrap();
        n.body = body.to_owned();
        n.modified_at = now_ms();
        self.vault.update_note(&n).unwrap();
    }
}
