//! WebDAV set-up: the settings flow (enter address, user name and password, optionally trust a
//! self-signed certificate by its fingerprint, check what the account may do, save), and the
//! sealed storage of the credentials on this device.
//!
//! **Credentials** are stored only in `.nuvoletta/device-state/webdav`, sealed as envelope
//! type 4 with `K_local` (`docs/STORAGE_FORMAT.md`). They are never synced, never uploaded, never
//! logged, readable only while the vault is unlocked (the key comes from the unlocked vault), and
//! kept in memory in zeroizing buffers (copies made by the JSON parser and by the UI's text
//! fields cannot be zeroized; see `docs/THREAT_MODEL.md` A5).
//!
//! **The check** runs on a worker thread (it makes network requests) and is polled like a sync
//! cycle. Nothing is saved unless the account can read and create but cannot delete or overwrite
//! (`docs/WEBDAV.md`); a refused account is explained in plain words and never stored.

use crate::app::{App, State};
use crate::error::{Error, Result};
use crate::sync_settings::{SyncBackend, SyncSettings, device_state_dir, write_device_file};
use nuvoletta_crypto::{ObjType, ObjectMeta, Subkeys, open, seal};
use nuvoletta_webdav::{
    ConfigError, DavError, Options, Refusal, TlsFailure, Verdict, WebDavConfig, WebDavProvider,
    fetch_fingerprint, format_fingerprint, parse_fingerprint,
};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;
use zeroize::Zeroizing;

const FILE: &str = "webdav";
const KIND: &str = "webdav-credentials";

/// What the user typed.
#[derive(Clone)]
pub struct WebDavInput {
    /// The folder address, `https://…`.
    pub url: String,
    /// The user name.
    pub username: String,
    /// The password (an app password where the server offers them).
    pub password: Zeroizing<String>,
    /// A certificate fingerprint the user confirmed (hex, `:` separators allowed), if any.
    pub pin: Option<String>,
}

impl std::fmt::Debug for WebDavInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebDavInput(..)")
    }
}

/// The stored WebDAV settings, without the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavSummary {
    /// The folder address.
    pub url: String,
    /// The user name.
    pub username: String,
    /// The fingerprint of the pinned certificate, if one is pinned.
    pub pinned_fingerprint: Option<String>,
}

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebDavEvent {
    /// The account is acceptable; the credentials were saved and sync is on.
    Saved,
    /// The server's certificate is not signed by a known authority. The user may trust exactly
    /// this certificate after comparing the fingerprint with the one the server shows.
    NeedsPin {
        /// SHA-256 fingerprint, upper-case hex pairs separated by `:`.
        fingerprint: String,
        /// Why the certificate is not trusted.
        reason: String,
    },
    /// The account can delete or overwrite files (or its permissions could not be determined):
    /// refused and not saved.
    Refused {
        /// One-line headline.
        title: String,
        /// What was found, why it matters, what to do.
        explanation: String,
    },
    /// The check could not be completed (wrong password, unreachable server, …).
    Failed {
        /// What went wrong.
        message: String,
        /// What to do about it.
        hint: String,
    },
}

enum CheckResult {
    Safe,
    Refused(Refusal),
    NeedsPin { fp: [u8; 32], reason: String },
    Failed(DavError),
}

pub(crate) struct CheckOutcome {
    cfg: WebDavConfig,
    result: CheckResult,
}

fn config_error(e: &ConfigError) -> Error {
    Error::Input(e.to_string())
}

fn run_check(cfg: WebDavConfig) -> CheckOutcome {
    let provider = WebDavProvider::new(cfg.clone(), Options::default());
    let result = match provider.verify() {
        Ok(Verdict::Safe { .. }) => CheckResult::Safe,
        Ok(Verdict::Refused(r)) => CheckResult::Refused(r),
        Err(DavError::Tls(TlsFailure::Untrusted(reason))) if cfg.pin().is_none() => {
            // Handshake only; no credentials are sent to a server that is not trusted.
            let (host, port) = cfg.host_port();
            match fetch_fingerprint(&host, port, Duration::from_secs(10)) {
                Ok(fp) => CheckResult::NeedsPin { fp, reason },
                Err(e) => CheckResult::Failed(e),
            }
        }
        Err(e) => CheckResult::Failed(e),
    };
    CheckOutcome { cfg, result }
}

/// What a failed request means for the user: what went wrong and what to do.
fn describe_dav_error(e: &DavError) -> (String, String) {
    let (m, h): (String, &str) = match e {
        DavError::Auth => (
            "The server rejected the user name or password.".to_owned(),
            "Check them. With Nextcloud, create an app password (Settings, Security) instead of using the login password.",
        ),
        DavError::Forbidden => (
            "The account is not allowed to read this folder.".to_owned(),
            "Check the address and that the account may list and read it.",
        ),
        DavError::NotFound => (
            "The folder does not exist on the server.".to_owned(),
            "Create the folder first (Nuvoletta does not create it) and check the address.",
        ),
        DavError::Network(d) => (
            format!("The server could not be reached ({d})."),
            "Check the address and your connection, then try again.",
        ),
        DavError::Timeout(_) => (
            "The server did not answer in time.".to_owned(),
            "Check the address and your connection, then try again.",
        ),
        DavError::Throttled { .. } => (
            "The server is busy or limiting requests.".to_owned(),
            "Wait a few minutes and try again.",
        ),
        DavError::Tls(TlsFailure::PinMismatch) => (
            "The server presented a different certificate than the one you trusted.".to_owned(),
            "If the server's certificate was renewed on purpose, check the new fingerprint and trust it again. Otherwise someone may be intercepting the connection.",
        ),
        DavError::Tls(TlsFailure::Untrusted(_)) => (
            "The server's certificate is not trusted.".to_owned(),
            "Use an address with a valid certificate, or trust the certificate by its fingerprint.",
        ),
        DavError::Tls(TlsFailure::Other(d)) => (
            format!("The secure connection failed ({d})."),
            "Check the address (it must start with https://) and the server's TLS settings.",
        ),
        DavError::Redirect(d) => (
            format!("The server redirected the request ({d})."),
            "Enter the final address of the folder, exactly as the server shows it.",
        ),
        other => (
            format!("The server gave an unexpected answer ({other})."),
            "Check that the address is a WebDAV folder address.",
        ),
    };
    (m, h.to_owned())
}

// ---- sealed storage -----------------------------------------------------------------------

fn meta() -> Result<ObjectMeta> {
    Ok(ObjectMeta::new(ObjType::LocalState, [0; 16])?)
}

fn encode(cfg: &WebDavConfig) -> Result<Zeroizing<Vec<u8>>> {
    use std::io::Write;
    let mut buf = Zeroizing::new(Vec::<u8>::new());
    let json_str = |buf: &mut Zeroizing<Vec<u8>>, s: &str| {
        serde_json::to_writer(&mut **buf, s).map_err(|e| Error::Settings(e.to_string()))
    };
    let w = |buf: &mut Zeroizing<Vec<u8>>, t: &str| {
        buf.write_all(t.as_bytes())
            .map_err(|e| Error::Settings(e.to_string()))
    };
    w(&mut buf, "{\"kind\":")?;
    json_str(&mut buf, KIND)?;
    w(&mut buf, ",\"url\":")?;
    json_str(&mut buf, cfg.url())?;
    w(&mut buf, ",\"username\":")?;
    json_str(&mut buf, cfg.username())?;
    w(&mut buf, ",\"password\":")?;
    json_str(&mut buf, cfg.password())?;
    if let Some(pin) = cfg.pin() {
        w(&mut buf, ",\"pin\":")?;
        json_str(&mut buf, &format_fingerprint(pin))?;
    }
    w(&mut buf, "}")?;
    Ok(buf)
}

/// Seal and store the credentials (atomic write).
pub(crate) fn save_credentials(root: &Path, keys: &Subkeys, cfg: &WebDavConfig) -> Result<()> {
    let plain = encode(cfg)?;
    let sealed = seal(keys, &meta()?, &plain)?;
    write_device_file(root, FILE, &sealed)
}

/// Load and open the credentials; `None` if missing, damaged, sealed under another key or not
/// WebDAV credentials.
pub(crate) fn load_credentials(root: &Path, keys: &Subkeys) -> Option<WebDavConfig> {
    let bytes = fs::read(device_state_dir(root).join(FILE)).ok()?;
    let opened = open(keys, &bytes).ok()?;
    if opened.meta.obj_type != ObjType::LocalState {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&opened.plaintext).ok()?;
    if v.get("kind")?.as_str()? != KIND {
        return None;
    }
    let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
    let pin = match s("pin") {
        Some(p) => Some(parse_fingerprint(p).ok()?),
        None => None,
    };
    let password = Zeroizing::new(s("password")?.to_owned());
    WebDavConfig::new(s("url")?, s("username")?, &password, pin).ok()
}

fn forget_credentials(root: &Path) {
    let _ = fs::remove_file(device_state_dir(root).join(FILE));
}

// ---- the application surface --------------------------------------------------------------

impl App {
    /// The stored WebDAV settings (without the password); `None` if none are stored or the vault
    /// is locked.
    #[must_use]
    pub fn webdav_summary(&self) -> Option<WebDavSummary> {
        let c = self.session().ok()?.sync.webdav.as_ref()?;
        Some(WebDavSummary {
            url: c.url().to_owned(),
            username: c.username().to_owned(),
            pinned_fingerprint: c.pin().map(format_fingerprint),
        })
    }

    /// Whether a check is running.
    #[must_use]
    pub fn webdav_check_running(&self) -> bool {
        self.session()
            .is_ok_and(|s| s.sync.webdav_job.is_some() || s.sync.webdav_done.is_some())
    }

    /// Check the account and, if it may only read and create, save it and turn WebDAV sync on.
    /// Runs on a worker thread; poll with [`App::poll_webdav`].
    ///
    /// # Errors
    /// [`Error::Input`] for an unusable address, user name, password or fingerprint (with an
    /// explanation), [`Error::SyncBusy`] if a check is already running or the vault is away.
    pub fn webdav_check_start(&mut self, input: &WebDavInput) -> Result<()> {
        let pin = match input
            .pin
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            Some(p) => Some(parse_fingerprint(p).map_err(|e| config_error(&e))?),
            None => None,
        };
        let cfg = WebDavConfig::new(&input.url, input.username.trim(), &input.password, pin)
            .map_err(|e| config_error(&e))?;
        let s = self.session_mut()?;
        if s.sync.webdav_job.is_some() || s.sync.webdav_done.is_some() || s.vault.is_none() {
            return Err(Error::SyncBusy);
        }
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("nuvoletta-webdav-check".to_owned())
            .spawn(move || {
                let _ = tx.send(run_check(cfg));
            })
            .map_err(|_| Error::Sync("could not start a background task".to_owned()))?;
        s.sync.webdav_job = Some(rx);
        Ok(())
    }

    /// If a check has finished, take its result. A pass is saved here (sealed with the vault's
    /// local key), so this waits while a sync cycle has the vault.
    pub fn poll_webdav(&mut self) -> Option<WebDavEvent> {
        let State::Unlocked(s) = &mut self.state else {
            return None;
        };
        if s.sync.webdav_done.is_none() {
            let rx = s.sync.webdav_job.as_ref()?;
            match rx.try_recv() {
                Ok(o) => s.sync.webdav_done = Some(o),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    s.sync.webdav_job = None;
                    return Some(WebDavEvent::Failed {
                        message: "The check stopped unexpectedly.".to_owned(),
                        hint: "Try again.".to_owned(),
                    });
                }
            }
            s.sync.webdav_job = None;
        }
        let safe = matches!(s.sync.webdav_done.as_ref()?.result, CheckResult::Safe);
        if safe && s.vault.is_none() {
            return None; // saving needs the keys; the vault is with the sync worker
        }
        let CheckOutcome { cfg, result } = s.sync.webdav_done.take()?;
        Some(match result {
            CheckResult::Safe => match self.save_webdav(cfg) {
                Ok(()) => WebDavEvent::Saved,
                Err(e) => WebDavEvent::Failed {
                    message: format!("The credentials could not be saved ({}).", e.user_message()),
                    hint: "Check free space and permissions of the vault folder.".to_owned(),
                },
            },
            CheckResult::Refused(r) => WebDavEvent::Refused {
                title: "This account can change or delete files, so it is not used".to_owned(),
                explanation: r.explanation(),
            },
            CheckResult::NeedsPin { fp, reason } => WebDavEvent::NeedsPin {
                fingerprint: format_fingerprint(&fp),
                reason,
            },
            CheckResult::Failed(e) => {
                let (message, hint) = describe_dav_error(&e);
                WebDavEvent::Failed { message, hint }
            }
        })
    }

    fn save_webdav(&mut self, cfg: WebDavConfig) -> Result<()> {
        let s = self.session_mut()?;
        let Some(vault) = s.vault.as_ref() else {
            return Err(Error::SyncBusy);
        };
        save_credentials(&s.root, vault.keys(), &cfg)?;
        s.sync.webdav = Some(cfg);
        let next = SyncSettings {
            enabled: true,
            backend: SyncBackend::WebDav,
            webdav_configured: true,
            ..self.sync_cfg.clone()
        };
        self.apply_sync_settings(next)
    }

    /// Forget the stored WebDAV credentials on this device and turn WebDAV sync off. Nothing is
    /// changed on the server.
    ///
    /// # Errors
    /// [`Error::SyncBusy`] during a sync cycle; [`Error::Settings`].
    pub fn webdav_forget(&mut self) -> Result<()> {
        let s = self.session_mut()?;
        if s.vault.is_none() {
            return Err(Error::SyncBusy);
        }
        forget_credentials(&s.root);
        s.sync.webdav = None;
        let mut next = self.sync_cfg.clone();
        next.webdav_configured = false;
        if next.backend == SyncBackend::WebDav {
            next.enabled = false;
            next.backend = SyncBackend::Folder;
        }
        self.apply_sync_settings(next)
    }
}

pub(crate) type CheckReceiver = Receiver<CheckOutcome>;
