//! Joining a device to an existing vault from the sync location
//! (`docs/ADR/0008-device-join-and-header-adoption.md`).
//!
//! The flow has four steps, each a method on [`App`], and nothing reaches the disk before the
//! third:
//!
//! 1. **Search** ([`App::join_search_start`], [`App::join_search_poll`]): list `h/` of the sync
//!    location on a worker thread and parse the header files. Nothing else on the provider is read
//!    and nothing is written (a WebDAV account is checked first, exactly as in the sync settings:
//!    an account that can delete or overwrite is refused).
//! 2. **Verify** ([`App::join_verify`]): the user types the passphrase of the current header (or
//!    the recovery key). The header is opened with it (KDF cap, unwrap, `header_mac`) in memory;
//!    the result carries the vault fingerprint and generation for the user to compare with another
//!    device. A header that does not open is never trusted, whatever its name says.
//! 3. **Confirm** ([`App::join_confirm`]): the verified header is written as the new vault's
//!    header, the vault is unlocked, the sync location and the verified generation are stored and
//!    the first sync starts.
//! 4. **First sync**: the session shows a join screen until a cycle succeeds
//!    ([`App::join_in_progress`]). If the user cancels, or gives up after a failure,
//!    [`App::join_cancel`] removes the vault directory, but only while it holds no notes.
//!
//! A join with the recovery key requires a new passphrase only after the first successful sync
//! (ADR O4), so a brand-new device never publishes a header generation before it has synced.

use crate::app::{App, State};
use crate::error::{Error, Result};
use crate::sync_messages::describe_error;
use crate::sync_settings::{SyncBackend, SyncSettings, check_sync_folder};
use crate::webdav::{
    CheckResult, WebDavInput, config_from_input, describe_dav_error, run_check, save_credentials,
};
use nuvoletta_crypto::{RecoveryKey, Vault};
use nuvoletta_storage::layout::META_DIR;
use nuvoletta_storage::{Cleanup, Locked, Secret, remove_unused_vault};
use nuvoletta_sync::state::{STATE_DIR, record_verified_header_gen};
use nuvoletta_sync::{FolderProvider, HeaderScan, Provider, fingerprint, scan_headers};
use nuvoletta_webdav::{Options, WebDavConfig, WebDavProvider, format_fingerprint};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Where to look for the vault.
#[derive(Clone)]
pub enum JoinSource {
    /// A folder that an external tool keeps in step (desktop).
    Folder(PathBuf),
    /// A WebDAV location (desktop and Android).
    WebDav(WebDavInput),
}

impl std::fmt::Debug for JoinSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JoinSource(..)")
    }
}

/// A vault found on the sync location, as shown for the user to choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinCandidate {
    /// Position to pass to [`App::join_verify`].
    pub index: usize,
    /// The vault fingerprint (the raw vault id in grouped hex), to compare with another device.
    pub fingerprint: String,
    /// The newest header generation present.
    pub newest_generation: u32,
    /// How many header generations are present.
    pub headers: usize,
    /// More than one different header of the newest generation exists (ADR O6).
    pub diverged: bool,
    /// Argon2id memory cost of the newest header, MiB.
    pub kdf_mib: u32,
    /// Argon2id passes of the newest header.
    pub kdf_passes: u32,
    /// The newest header needs more work than this device accepts for a passphrase (ADR O5); the
    /// recovery key still works. The text to show.
    pub too_costly: Option<String>,
}

/// How a search ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinSearchEvent {
    /// One or more vaults were found.
    Found(Vec<JoinCandidate>),
    /// The location was readable but holds no vault header.
    NoVault {
        /// What was found instead and what to check.
        message: String,
    },
    /// The location could not be read.
    Failed {
        /// What went wrong.
        message: String,
        /// What to do about it.
        hint: String,
    },
    /// The server's certificate is not signed by a known authority (as in the WebDAV settings).
    NeedsPin {
        /// SHA-256 fingerprint, upper-case hex pairs separated by `:`.
        fingerprint: String,
        /// Why the certificate is not trusted.
        reason: String,
    },
    /// The WebDAV account can delete or overwrite files: refused.
    Refused {
        /// One-line headline.
        title: String,
        /// What was found, why it matters, what to do.
        explanation: String,
    },
}

/// The secret typed to join.
#[derive(Clone, Copy)]
pub enum JoinSecret<'a> {
    /// The passphrase of the current header.
    Passphrase(&'a str),
    /// The printable recovery key.
    RecoveryKey(&'a str),
}

impl std::fmt::Debug for JoinSecret<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JoinSecret(..)")
    }
}

/// What the user must see and confirm before the join writes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinPreview {
    /// The vault fingerprint.
    pub fingerprint: String,
    /// The generation of the header that opened.
    pub generation: u32,
    /// The newest generation in the folder.
    pub newest_generation: u32,
    /// The recovery key was used: a new passphrase is required after the first sync.
    pub recovery_key: bool,
    /// Things to read before confirming (a diverging header, an older header used).
    pub warnings: Vec<String>,
}

/// What [`App::join_cancel`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinCancel {
    /// Nothing had been written; the join state was dropped.
    Discarded,
    /// The vault directory was removed (it held no notes).
    Removed,
    /// The vault holds notes, so it was left in place at this path.
    Kept(PathBuf),
}

/// A join in progress after the first sync started.
pub(crate) struct Joining {
    /// The recovery key opened the header: ask for a new passphrase after the first sync.
    pub(crate) recovery_key: bool,
}

#[derive(Clone)]
enum Origin {
    Folder(PathBuf),
    WebDav(WebDavConfig),
}

enum Done {
    Scan(HeaderScan, Origin),
    Event(JoinSearchEvent),
}

struct Verified {
    vault: Vault,
    recovery_key: bool,
    origin: Origin,
}

/// State of a join before the vault exists.
#[derive(Default)]
pub(crate) struct JoinState {
    job: Option<Receiver<Done>>,
    found: Option<(HeaderScan, Origin)>,
    verified: Option<Verified>,
}

enum Input {
    Folder(PathBuf),
    WebDav(WebDavConfig),
}

fn scan_with<P: Provider + ?Sized>(provider: &P, origin: Origin, backend: SyncBackend) -> Done {
    match scan_headers(provider) {
        Ok(scan) => Done::Scan(scan, origin),
        Err(e) => {
            let p = describe_error(&e, backend);
            Done::Event(JoinSearchEvent::Failed {
                message: p.message,
                hint: p.hint,
            })
        }
    }
}

fn search(input: Input) -> Done {
    match input {
        Input::Folder(p) => {
            // `FolderProvider::new` would create a missing folder; a join must never.
            if !p.is_dir() {
                return Done::Event(JoinSearchEvent::Failed {
                    message: "That folder does not exist or cannot be read.".to_owned(),
                    hint: "Choose the folder your sync tool keeps in step with your other devices."
                        .to_owned(),
                });
            }
            match FolderProvider::new(&p) {
                Ok(provider) => scan_with(&provider, Origin::Folder(p), SyncBackend::Folder),
                Err(e) => Done::Event(JoinSearchEvent::Failed {
                    message: format!("The folder could not be read ({e})."),
                    hint: "Check that it exists and that you may read it.".to_owned(),
                }),
            }
        }
        Input::WebDav(cfg) => {
            let outcome = run_check(cfg);
            match outcome.result {
                CheckResult::Safe => {
                    let provider = WebDavProvider::new(outcome.cfg.clone(), Options::default());
                    scan_with(&provider, Origin::WebDav(outcome.cfg), SyncBackend::WebDav)
                }
                CheckResult::Refused(r) => Done::Event(JoinSearchEvent::Refused {
                    title: "This account can change or delete files, so it is not used".to_owned(),
                    explanation: r.explanation(),
                }),
                CheckResult::NeedsPin { fp, reason } => Done::Event(JoinSearchEvent::NeedsPin {
                    fingerprint: format_fingerprint(&fp),
                    reason,
                }),
                CheckResult::Failed(e) => {
                    let (message, hint) = describe_dav_error(&e);
                    Done::Event(JoinSearchEvent::Failed { message, hint })
                }
            }
        }
    }
}

fn candidates(scan: &HeaderScan, limit: &crate::platform::KdfLimit) -> Vec<JoinCandidate> {
    scan.groups
        .iter()
        .enumerate()
        .map(|(index, g)| {
            let newest = g.newest_generation();
            let top: Vec<_> = g.at_generation(newest).collect();
            let kdf = top.first().map(|h| h.kdf_params()).unwrap_or_default();
            let costly = top.iter().all(|h| limit.check(&h.kdf_params()).is_err());
            JoinCandidate {
                index,
                fingerprint: fingerprint(&g.vault_id),
                newest_generation: newest,
                headers: g.candidates.len(),
                diverged: top.len() > 1,
                kdf_mib: kdf.m_kib / 1024,
                kdf_passes: kdf.t,
                too_costly: costly.then(|| limit.check(&kdf).err()).flatten(),
            }
        })
        .collect()
}

/// Store what the first sync needs before the session exists: the verified generation (so a
/// header older than the one just opened is flagged later) and the sync location. Returns the
/// canonical sync folder for a folder origin.
fn prepare(
    vault: &nuvoletta_storage::Unlocked,
    origin: &Origin,
    generation: u32,
) -> Result<Option<PathBuf>> {
    let root = vault.root();
    record_verified_header_gen(
        &root.join(META_DIR).join(STATE_DIR),
        vault.keys(),
        generation,
    )
    .map_err(|e| Error::Sync(e.to_string()))?;
    match origin {
        Origin::Folder(p) => Ok(Some(check_sync_folder(root, p)?)),
        Origin::WebDav(cfg) => {
            save_credentials(root, vault.keys(), cfg)?;
            Ok(None)
        }
    }
}

fn wrong_cap_or_unlock(blocked: Option<String>) -> Error {
    match blocked {
        Some(m) => Error::Input(m),
        None => Error::Crypto(nuvoletta_crypto::Error::Unlock),
    }
}

impl App {
    /// Start looking for a vault at `source` on a worker thread; poll with
    /// [`App::join_search_poll`]. Replaces an earlier search or verification.
    ///
    /// # Errors
    /// [`Error::Input`] for an unusable address, user name, password or fingerprint; the
    /// "wrong state" errors if a vault is open.
    pub fn join_search_start(&mut self, source: &JoinSource) -> Result<()> {
        if !matches!(self.state, State::NoVault) {
            return Err(self.wrong_state());
        }
        let input = match source {
            JoinSource::Folder(p) => Input::Folder(p.clone()),
            JoinSource::WebDav(i) => Input::WebDav(config_from_input(i)?),
        };
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("nuvoletta-join-search".to_owned())
            .spawn(move || {
                let _ = tx.send(search(input));
            })
            .map_err(|_| Error::Sync("could not start a background task".to_owned()))?;
        self.join = Some(JoinState {
            job: Some(rx),
            ..JoinState::default()
        });
        Ok(())
    }

    /// Whether a search is running.
    #[must_use]
    pub fn join_search_running(&self) -> bool {
        self.join.as_ref().is_some_and(|j| j.job.is_some())
    }

    /// If the search has finished, take its result.
    pub fn join_search_poll(&mut self) -> Option<JoinSearchEvent> {
        let limit = self.kdf_limit;
        let js = self.join.as_mut()?;
        let done = match js.job.as_ref()?.try_recv() {
            Ok(d) => d,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Done::Event(JoinSearchEvent::Failed {
                message: "The search stopped unexpectedly.".to_owned(),
                hint: "Try again.".to_owned(),
            }),
        };
        js.job = None;
        Some(match done {
            Done::Event(e) => e,
            Done::Scan(scan, origin) => {
                if scan.groups.is_empty() {
                    let skipped = scan.ignored.len();
                    return Some(JoinSearchEvent::NoVault {
                        message: if skipped == 0 {
                            "There is no vault header in the h folder of this location. Check that it is \
                             the folder your other device syncs, and that sync has finished there."
                                .to_owned()
                        } else {
                            format!(
                                "This location has no usable vault header ({skipped} file(s) in its h folder \
                                 were skipped because they are not valid headers). Check that it is the folder \
                                 your other device syncs, and that sync has finished there."
                            )
                        },
                    });
                }
                let found = candidates(&scan, &limit);
                js.found = Some((scan, origin));
                JoinSearchEvent::Found(found)
            }
        })
    }

    /// Open the header of vault `index` with the typed secret, in memory, and return what the
    /// user must confirm. Nothing is written. Runs Argon2 (about a second on a phone).
    ///
    /// Only the newest generation is tried unless `allow_older` is set (the user chose to use an
    /// older header after the newest one did not open). Two different headers of one generation
    /// are both tried; the one the secret opens is used and the user is warned about the other.
    ///
    /// # Errors
    /// The generic unlock error for a wrong secret or a header that does not authenticate;
    /// [`Error::Input`] if every header that could be tried is above this device's KDF limit.
    pub fn join_verify(
        &mut self,
        index: usize,
        secret: JoinSecret<'_>,
        allow_older: bool,
    ) -> Result<JoinPreview> {
        if !matches!(self.state, State::NoVault) {
            return Err(self.wrong_state());
        }
        let limit = self.kdf_limit;
        let js = self
            .join
            .as_mut()
            .ok_or(Error::Invalid("Look for the vault first."))?;
        js.verified = None;
        let (scan, origin) = js
            .found
            .as_ref()
            .ok_or(Error::Invalid("Look for the vault first."))?;
        let group = scan
            .groups
            .get(index)
            .ok_or(Error::Invalid("Choose one of the vaults that were found."))?;
        let key = match secret {
            JoinSecret::RecoveryKey(k) => Some(RecoveryKey::parse(k)?),
            JoinSecret::Passphrase(_) => None,
        };
        let typed = match (&secret, &key) {
            (JoinSecret::Passphrase(p), _) => Secret::Passphrase(p),
            (_, Some(k)) => Secret::RecoveryKey(k),
            _ => return Err(Error::Invalid("Enter the passphrase or the recovery key.")),
        };
        let mut gens: Vec<u32> = group
            .candidates
            .iter()
            .map(nuvoletta_crypto::VaultHeader::generation)
            .collect();
        gens.dedup();
        if !allow_older {
            gens.truncate(1);
        }
        let newest = group.newest_generation();
        let mut blocked = None;
        let mut opened: Option<(Vault, u32, usize)> = None;
        'outer: for g in gens {
            let set: Vec<_> = group.at_generation(g).collect();
            for h in &set {
                // The recovery key unwraps without Argon2: the cap protects the passphrase path.
                if key.is_none()
                    && let Err(m) = limit.check(&h.kdf_params())
                {
                    blocked.get_or_insert(m);
                    continue;
                }
                match typed.open(h) {
                    Ok(v) => {
                        opened = Some((v, g, set.len() - 1));
                        break 'outer;
                    }
                    Err(nuvoletta_storage::Error::Crypto(nuvoletta_crypto::Error::Unlock)) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let Some((vault, generation, others)) = opened else {
            return Err(wrong_cap_or_unlock(blocked));
        };
        let mut warnings = Vec::new();
        if others > 0 {
            warnings.push(format!(
                "The folder holds {others} other header(s) of generation {generation} for this vault. This \
                 device uses the one your {} opened and does not trust the other. This happens when the \
                 passphrase was changed on two devices at the same moment: check that your other devices \
                 still unlock.",
                if key.is_some() { "recovery key" } else { "passphrase" }
            ));
        }
        if generation < newest {
            warnings.push(format!(
                "You are using an older header (generation {generation}) than the newest in the folder \
                 ({newest}). The passphrase you typed is not the current one. Anyone who knows it can join \
                 this vault, so use this only if you cannot get the current passphrase."
            ));
        }
        let preview = JoinPreview {
            fingerprint: fingerprint(&group.vault_id),
            generation,
            newest_generation: newest,
            recovery_key: key.is_some(),
            warnings,
        };
        js.verified = Some(Verified {
            vault,
            recovery_key: key.is_some(),
            origin: origin.clone(),
        });
        Ok(preview)
    }

    /// Create the vault in `dir` from the verified header, unlock it, store the sync location and
    /// start the first sync. From here [`App::join_in_progress`] is true until a cycle succeeds.
    ///
    /// If anything fails after the directory was created, the directory is removed again (it
    /// holds no notes yet) and the error is returned.
    ///
    /// # Errors
    /// [`Error::Invalid`] if nothing was verified; [`Error::Storage`] if `dir` already holds a
    /// vault or cannot be written; [`Error::Invalid`] for a sync folder inside the vault.
    pub fn join_confirm(&mut self, dir: &Path) -> Result<()> {
        if !matches!(self.state, State::NoVault) {
            return Err(self.wrong_state());
        }
        let verified = self
            .join
            .as_mut()
            .and_then(|j| j.verified.take())
            .ok_or(Error::Invalid(
                "Enter the passphrase or the recovery key first.",
            ))?;
        self.join = None;
        let Verified {
            vault,
            recovery_key,
            origin,
        } = verified;
        let generation = vault.header().generation();
        let unlocked = Locked::join(dir, vault)?;
        // From here the directory is ours: a failure undoes it (it holds no notes yet).
        let folder = match prepare(&unlocked, &origin, generation) {
            Ok(f) => f,
            Err(e) => {
                drop(unlocked);
                let _ = remove_unused_vault(dir);
                return Err(e);
            }
        };
        self.install(unlocked, false)?;
        self.settings.last_vault = Some(dir.to_path_buf());
        let next = match origin {
            Origin::Folder(_) => SyncSettings {
                enabled: true,
                backend: SyncBackend::Folder,
                folder,
                webdav_configured: false,
            },
            Origin::WebDav(cfg) => {
                self.session_mut()?.sync.webdav = Some(cfg);
                SyncSettings {
                    enabled: true,
                    backend: SyncBackend::WebDav,
                    folder: None,
                    webdav_configured: true,
                }
            }
        };
        self.session_mut()?.joining = Some(Joining { recovery_key });
        let applied = self.apply_sync_settings(next);
        // The first sync; a failure to start it is in the status, and the join screen offers
        // retry and cancel.
        let _ = self.sync_now();
        applied
    }

    /// The fingerprint and header generation of the open vault, for comparing with what a
    /// joining device shows. `None` without a vault, or while a sync cycle has the vault.
    #[must_use]
    pub fn vault_identity(&self) -> Option<(String, u32)> {
        let header = match &self.state {
            State::NoVault => return None,
            State::Locked(l) => l.header(),
            State::Unlocked(s) => s.vault.as_ref()?.header(),
        };
        Some((fingerprint(&header.vault_id()), header.generation()))
    }

    /// Whether the vault was just joined and its first sync has not succeeded yet. The UI shows
    /// the join screen (progress, retry, cancel) instead of the workspace.
    #[must_use]
    pub fn join_in_progress(&self) -> bool {
        self.session().is_ok_and(|s| s.joining.is_some())
    }

    /// Cancel a join, or give up after the first sync failed. Before the vault exists this drops
    /// the join state. After, it closes the vault and removes the directory **only if it holds no
    /// notes** (ADR O7): otherwise it is left in place and the path is returned so the user can
    /// be told.
    ///
    /// # Errors
    /// [`Error::Invalid`] if no join is in progress; errors of closing the vault or removing the
    /// directory.
    pub fn join_cancel(&mut self) -> Result<JoinCancel> {
        if matches!(self.state, State::NoVault) {
            self.join = None;
            return Ok(JoinCancel::Discarded);
        }
        if !self.join_in_progress() {
            return Err(Error::Invalid("No join is in progress."));
        }
        // A cycle that is running finishes first (it may have succeeded just now).
        self.wait_sync();
        if !self.join_in_progress() {
            return Err(Error::Invalid("The join has already finished."));
        }
        let root = self.session()?.root.clone();
        self.close_vault(false)?;
        match remove_unused_vault(&root)? {
            Cleanup::Removed => {
                if self.settings.last_vault.as_deref() == Some(root.as_path()) {
                    self.settings.last_vault = None;
                }
                Ok(JoinCancel::Removed)
            }
            Cleanup::Kept => Ok(JoinCancel::Kept(root)),
        }
    }
}
