//! Adopting a newer vault header after the passphrase or recovery key was changed on another
//! device (`docs/ADR/0008-device-join-and-header-adoption.md`, part C).
//!
//! Like the join, in steps, and nothing is written before the last:
//!
//! 1. **Search** ([`App::adopt_search_start`], [`App::adopt_search_poll`]): list `h/` of the sync
//!    location on a worker thread and keep the headers of *this* vault that are not older than the
//!    one held here.
//! 2. **Verify** ([`App::adopt_verify`]): the user types the new passphrase (or recovery key). The
//!    candidate is opened with it (KDF cap, unwrap, header MAC) and bound to this vault: on an
//!    unlocked device the unwrapped DEK must equal the one in memory; on a **locked** device the
//!    keys it yields must open existing sealed local data (a note or a `K_local` file), because
//!    the DEK is not in memory (O1). A generation lower than the highest verified one is a
//!    rollback and is refused. Of two different headers of one generation the one the secret opens
//!    is used and the user is warned about the other (O6).
//! 3. **Commit** ([`App::adopt_commit`]): a header that lowers the KDF cost needs an explicit
//!    confirmation; then the header file is replaced atomically and the verified generation is
//!    recorded. The data key is unchanged, so no other file is touched.

use crate::app::{App, State};
use crate::error::{Error, Result};
use crate::join::{Done, Input, JoinSearchEvent, JoinSecret, JoinSource, search};
use crate::sync::{add_notice, provider_for, save_notices};
use crate::sync_settings::{SyncBackend, SyncSettings};
use crate::webdav::config_from_input;
use nuvoletta_crypto::{KdfParams, RecoveryKey, VaultHeader};
use nuvoletta_storage::layout::META_DIR;
use nuvoletta_storage::{Refusal, Secret, VerifiedHeader};
use nuvoletta_sync::state::{
    STATE_DIR, ensure_not_rolled_back, record_verified_header_gen, verified_header_gen,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Most candidates opened with one typed secret (each runs Argon2): a bound against a folder
/// stuffed with headers.
const MAX_TRIES: usize = 8;

/// What a search for a newer header found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptSearchEvent {
    /// A newer header (or a different one of the same generation) exists.
    Found {
        /// The generation this device holds.
        current_generation: u32,
        /// The newest generation offered.
        newest_generation: u32,
        /// More than one different header of the newest generation exists (O6).
        diverged: bool,
        /// The newest header needs more Argon2 work than this device accepts (O5).
        too_costly: Option<String>,
    },
    /// Nothing newer than what this device holds.
    UpToDate,
    /// The location could not be read.
    Failed {
        /// What went wrong.
        message: String,
        /// What to do about it.
        hint: String,
    },
    /// The server's certificate is not signed by a known authority.
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

/// What the user must see before a verified header replaces the one on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptPreview {
    /// The generation that opened.
    pub generation: u32,
    /// The generation held here now.
    pub current_generation: u32,
    /// How many generations are skipped (the other device may have changed it several times).
    pub jump: u32,
    /// The new header makes brute-forcing the passphrase cheaper; the text to show. Committing
    /// needs `accept_weaker_kdf`.
    pub weaker_kdf: Option<String>,
    /// Things to read (a diverging header of the same generation).
    pub warnings: Vec<String>,
}

/// What [`App::adopt_commit`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptOutcome {
    /// The generation now held.
    pub generation: u32,
    /// Warnings to keep showing (a diverging header of the same generation).
    pub warnings: Vec<String>,
}

struct Verified {
    header: VerifiedHeader,
    weaker: Option<String>,
    warnings: Vec<String>,
}

/// State of an adoption in progress.
#[derive(Default)]
pub(crate) struct AdoptState {
    job: Option<Receiver<Done>>,
    candidates: Vec<VaultHeader>,
    verified: Option<Verified>,
}

fn state_dir(root: &Path) -> PathBuf {
    root.join(META_DIR).join(STATE_DIR)
}

/// A reason the new header lowers the cost of guessing the passphrase, if it does: lower than
/// the header held here, or below the cost of a new vault.
fn weaker(new: &KdfParams, held: &KdfParams) -> Option<String> {
    let base = KdfParams::DEFAULT;
    let lower = |a: u32, b: u32, c: u32| a < b || a < c;
    if lower(new.m_kib, held.m_kib, base.m_kib)
        || lower(new.t, held.t, base.t)
        || lower(new.p, held.p, base.p)
    {
        Some(format!(
            "The new header makes guessing your passphrase cheaper: it uses {} MiB, {} passes and {} lane(s), \
             against {} MiB, {} passes and {} lane(s) now (a new vault uses {} MiB, {} and {}). It was set \
             on another device, on purpose or by someone who holds your keys. Adopt it only if you chose \
             this.",
            new.m_kib / 1024,
            new.t,
            new.p,
            held.m_kib / 1024,
            held.t,
            held.p,
            base.m_kib / 1024,
            base.t,
            base.p
        ))
    } else {
        None
    }
}

fn refusal_error(r: Refusal) -> Error {
    Error::Input(match r {
        Refusal::OtherVault => "That header belongs to another vault.".to_owned(),
        Refusal::Older { local, candidate } => format!(
            "That header (generation {candidate}) is older than generation {local}, which this device already \
             knows. It was refused: an old header would bring back an old passphrase."
        ),
        Refusal::AlreadyCurrent => "This device already has that header.".to_owned(),
        Refusal::DifferentKeys => {
            "That header opened, but its keys do not match the notes on this device, so it was refused. \
             Check that the sync location is the right one."
                .to_owned()
        }
        Refusal::NothingToCheck => {
            "This device has no notes or sync data to check the new header against, so it was refused. Join \
             the vault again instead."
                .to_owned()
        }
    })
}

impl App {
    fn adopt_local(&self) -> Result<(PathBuf, VaultHeader)> {
        match &self.state {
            State::NoVault => Err(Error::NoVault),
            State::Locked(l) => Ok((l.root().to_path_buf(), l.header().clone())),
            State::Unlocked(s) => {
                let v = s.vault.as_ref().ok_or(Error::SyncBusy)?;
                Ok((s.root.clone(), v.header().clone()))
            }
        }
    }

    /// Whether the location to search must be given by the user: a locked device cannot open the
    /// WebDAV credentials sealed on it, and a device without a synced folder has none to use.
    #[must_use]
    pub fn adopt_needs_source(&self) -> bool {
        match &self.state {
            State::Locked(l) => {
                let cfg = SyncSettings::load(l.root());
                !(cfg.backend == SyncBackend::Folder
                    && cfg.folder.as_deref().is_some_and(Path::is_dir))
            }
            State::Unlocked(_) => !self.sync_cfg.active(),
            State::NoVault => false,
        }
    }

    /// Start looking for a newer header of this vault on a worker thread; poll with
    /// [`App::adopt_search_poll`]. `source` is the location to read; `None` uses the one this
    /// device syncs with (the folder, or on an unlocked device the stored WebDAV account).
    ///
    /// # Errors
    /// [`Error::SyncNotConfigured`] or [`Error::Input`] if there is no location to use, an
    /// unusable address; [`Error::SyncBusy`] while a sync cycle has the vault.
    pub fn adopt_search_start(&mut self, source: Option<&JoinSource>) -> Result<()> {
        self.adopt_local()?;
        let input = match (source, &self.state) {
            (Some(JoinSource::Folder(p)), _) => Input::Folder(p.clone()),
            (Some(JoinSource::WebDav(i)), _) => Input::WebDav(config_from_input(i)?),
            (None, State::Unlocked(s)) => {
                if !self.sync_cfg.active() {
                    return Err(Error::SyncNotConfigured);
                }
                Input::Provider(
                    provider_for(&self.sync_cfg, s.sync.webdav.as_ref()),
                    self.sync_cfg.backend,
                )
            }
            (None, State::Locked(l)) => {
                let cfg = SyncSettings::load(l.root());
                match cfg.folder {
                    Some(f) if cfg.backend == SyncBackend::Folder && f.is_dir() => Input::Folder(f),
                    _ => {
                        return Err(Error::Input(
                            "Enter the sync location: this locked device cannot read the saved WebDAV account."
                                .to_owned(),
                        ));
                    }
                }
            }
            (None, State::NoVault) => return Err(Error::NoVault),
        };
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("nuvoletta-adopt-search".to_owned())
            .spawn(move || {
                let _ = tx.send(search(input));
            })
            .map_err(|_| Error::Sync("could not start a background task".to_owned()))?;
        self.adopt = Some(AdoptState {
            job: Some(rx),
            ..AdoptState::default()
        });
        Ok(())
    }

    /// Whether a search is running.
    #[must_use]
    pub fn adopt_search_running(&self) -> bool {
        self.adopt.as_ref().is_some_and(|a| a.job.is_some())
    }

    /// If the search has finished, take its result.
    pub fn adopt_search_poll(&mut self) -> Option<AdoptSearchEvent> {
        let (_, local) = self.adopt_local().ok()?;
        let limit = self.kdf_limit;
        let st = self.adopt.as_mut()?;
        let done = match st.job.as_ref()?.try_recv() {
            Ok(d) => d,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Done::Event(JoinSearchEvent::Failed {
                message: "The search stopped unexpectedly.".to_owned(),
                hint: "Try again.".to_owned(),
            }),
        };
        st.job = None;
        Some(match done {
            Done::Event(JoinSearchEvent::Failed { message, hint }) => {
                AdoptSearchEvent::Failed { message, hint }
            }
            Done::Event(JoinSearchEvent::NeedsPin {
                fingerprint,
                reason,
            }) => AdoptSearchEvent::NeedsPin {
                fingerprint,
                reason,
            },
            Done::Event(JoinSearchEvent::Refused { title, explanation }) => {
                AdoptSearchEvent::Refused { title, explanation }
            }
            Done::Event(_) => AdoptSearchEvent::UpToDate,
            Done::Scan(scan, _) => {
                let held = local.generation();
                let candidates: Vec<VaultHeader> = scan
                    .groups
                    .into_iter()
                    .find(|g| g.vault_id == local.vault_id())
                    .map(|g| g.candidates)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|h| {
                        h.generation() > held
                            || (h.generation() == held && h.as_bytes() != local.as_bytes())
                    })
                    .collect();
                let Some(top) = candidates.first() else {
                    return Some(AdoptSearchEvent::UpToDate);
                };
                let newest = top.generation();
                let at_top: Vec<_> = candidates
                    .iter()
                    .filter(|h| h.generation() == newest)
                    .collect();
                let too_costly = at_top
                    .iter()
                    .all(|h| limit.check(&h.kdf_params()).is_err())
                    .then(|| limit.check(&top.kdf_params()).err())
                    .flatten();
                let event = AdoptSearchEvent::Found {
                    current_generation: held,
                    newest_generation: newest,
                    diverged: at_top.len() > 1,
                    too_costly,
                };
                st.candidates = candidates;
                event
            }
        })
    }

    /// Open the newest candidate header the typed secret opens, in memory, and bind it to this
    /// vault. Nothing is written. Runs Argon2 for a passphrase.
    ///
    /// Candidates are tried newest first; a generation lower than the highest verified one is a
    /// rollback and is never tried.
    ///
    /// # Errors
    /// The generic unlock error for a wrong secret; [`Error::Input`] with an explanation when a
    /// header was refused (another vault, older, keys that do not match the local data, no local
    /// data to check, above this device's KDF limit); [`Error::SyncBusy`].
    pub fn adopt_verify(&mut self, secret: JoinSecret<'_>) -> Result<AdoptPreview> {
        let (root, local) = self.adopt_local()?;
        let limit = self.kdf_limit;
        let candidates = self
            .adopt
            .as_ref()
            .filter(|a| !a.candidates.is_empty())
            .map(|a| a.candidates.clone())
            .ok_or(Error::Invalid("Look for the new header first."))?;
        let key = match secret {
            JoinSecret::RecoveryKey(k) => Some(RecoveryKey::parse(k)?),
            JoinSecret::Passphrase(_) => None,
        };
        let typed = match (&secret, &key) {
            (JoinSecret::Passphrase(p), _) => Secret::Passphrase(p),
            (_, Some(k)) => Secret::RecoveryKey(k),
            _ => return Err(Error::Invalid("Enter the passphrase or the recovery key.")),
        };
        // The highest generation this device verified on the provider: lower is a rollback. On a
        // locked device it is only readable once a candidate has opened (it is sealed).
        let floor = match &self.state {
            State::Unlocked(s) => s
                .vault
                .as_ref()
                .map_or(0, |v| verified_header_gen(&state_dir(&root), v.keys())),
            _ => 0,
        };
        let mut refusal: Option<Refusal> = None;
        let mut blocked = None;
        let mut rolled_back = None;
        let mut found: Option<(VerifiedHeader, u32)> = None;
        let mut tries = 0;
        for h in &candidates {
            if h.generation() < floor {
                rolled_back.get_or_insert(h.generation());
                continue;
            }
            if key.is_none()
                && let Err(m) = limit.check(&h.kdf_params())
            {
                blocked.get_or_insert(m);
                continue;
            }
            if tries >= MAX_TRIES {
                break;
            }
            tries += 1;
            let result = match &self.state {
                State::Unlocked(s) => s
                    .vault
                    .as_ref()
                    .ok_or(Error::SyncBusy)?
                    .verify_header(h, &typed),
                State::Locked(l) => l.verify_header(h, &typed),
                State::NoVault => return Err(Error::NoVault),
            };
            match result {
                Ok(v) => {
                    let others = candidates
                        .iter()
                        .filter(|o| {
                            o.generation() == h.generation() && o.as_bytes() != h.as_bytes()
                        })
                        .count();
                    found = Some((v, u32::try_from(others).unwrap_or(u32::MAX)));
                    break;
                }
                Err(nuvoletta_storage::Error::Crypto(nuvoletta_crypto::Error::Unlock)) => {}
                Err(nuvoletta_storage::Error::HeaderRefused(r)) => {
                    refusal.get_or_insert(r);
                }
                Err(e) => return Err(e.into()),
            }
        }
        let Some((verified, others)) = found else {
            return Err(if let Some(r) = refusal {
                refusal_error(r)
            } else if let Some(m) = blocked {
                Error::Input(m)
            } else if let Some(g) = rolled_back {
                refusal_error(Refusal::Older {
                    local: floor,
                    candidate: g,
                })
            } else {
                Error::Crypto(nuvoletta_crypto::Error::Unlock)
            });
        };
        // A locked device could not read the remembered generation before; its keys are known now.
        ensure_not_rolled_back(&state_dir(&root), verified.keys(), verified.generation()).map_err(
            |_| {
                refusal_error(Refusal::Older {
                    local: verified_header_gen(&state_dir(&root), verified.keys()),
                    candidate: verified.generation(),
                })
            },
        )?;
        let mut warnings = Vec::new();
        if others > 0 {
            warnings.push(format!(
                "The sync location holds {others} other header(s) of generation {} for this vault. This \
                 device uses the one your {} opened and does not trust the other. This happens when the \
                 passphrase was changed on two devices at the same moment: check your other devices.",
                verified.generation(),
                if key.is_some() { "recovery key" } else { "passphrase" }
            ));
        }
        let preview = AdoptPreview {
            generation: verified.generation(),
            current_generation: local.generation(),
            jump: verified.generation().saturating_sub(local.generation()),
            weaker_kdf: weaker(&verified.kdf_params(), &local.kdf_params()),
            warnings: warnings.clone(),
        };
        if let Some(st) = self.adopt.as_mut() {
            st.verified = Some(Verified {
                weaker: preview.weaker_kdf.clone(),
                header: verified,
                warnings,
            });
        }
        Ok(preview)
    }

    /// Replace this device's header with the verified one. Needs `accept_weaker_kdf` if
    /// [`AdoptPreview::weaker_kdf`] was set. On an unlocked device the session keeps its keys and
    /// the notices about the newer header are cleared; on a locked device the vault stays locked
    /// and opens with the new passphrase.
    ///
    /// # Errors
    /// [`Error::Input`] if nothing was verified or a weaker header was not confirmed (the
    /// verification is kept); storage errors; [`Error::SyncBusy`].
    pub fn adopt_commit(&mut self, accept_weaker_kdf: bool) -> Result<AdoptOutcome> {
        let Some(mut st) = self.adopt.take() else {
            return Err(Error::Invalid("Enter the new passphrase first."));
        };
        let Some(v) = st.verified.take() else {
            self.adopt = Some(st);
            return Err(Error::Invalid("Enter the new passphrase first."));
        };
        if v.weaker.is_some() && !accept_weaker_kdf {
            let msg = v.weaker.clone().unwrap_or_default();
            st.verified = Some(v);
            self.adopt = Some(st);
            return Err(Error::Input(format!("{msg} Confirm to adopt it anyway.")));
        }
        let Verified {
            header, warnings, ..
        } = v;
        let generation = header.generation();
        match &mut self.state {
            State::Unlocked(s) => {
                let root = s.root.clone();
                let vault = s.vault.as_mut().ok_or(Error::SyncBusy)?;
                ensure_not_rolled_back(&state_dir(&root), vault.keys(), generation)
                    .map_err(|e| Error::Sync(e.to_string()))?;
                vault.adopt_header(header)?;
                let keys = vault.keys();
                record_verified_header_gen(&state_dir(&root), keys, generation)
                    .map_err(|e| Error::Sync(e.to_string()))?;
                // The adoption is what those notices asked for.
                s.sync
                    .notices
                    .retain(|n| n.code != "header-newer" && n.code != "header-diverged");
                for w in &warnings {
                    add_notice(
                        &mut s.sync.notices,
                        crate::sync_types::SyncNotice {
                            code: "header-diverged-other".to_owned(),
                            title: "Another header of the same generation exists".to_owned(),
                            detail: w.clone(),
                            first_seen_ms: nuvoletta_core::now_ms(),
                        },
                    );
                }
                if let Some(v) = s.vault.as_ref() {
                    let _ = save_notices(&root, v.keys(), &s.sync.notices);
                }
            }
            State::Locked(l) => {
                let root = l.root().to_path_buf();
                record_verified_header_gen(&state_dir(&root), header.keys(), generation)
                    .map_err(|e| Error::Sync(e.to_string()))?;
                l.adopt_header(header)?;
            }
            State::NoVault => return Err(Error::NoVault),
        }
        Ok(AdoptOutcome {
            generation,
            warnings,
        })
    }

    /// Drop a search or a verification without changing anything.
    pub fn adopt_cancel(&mut self) {
        self.adopt = None;
    }
}
