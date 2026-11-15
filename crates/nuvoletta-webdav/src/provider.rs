//! [`WebDavProvider`]: the sync [`Provider`] over WebDAV.
//!
//! What it does and, as important, what it never does:
//!
//! * **Creates** files with `PUT` + `If-None-Match: *` directly under their final names (there
//!   is no temporary name and no `MOVE`: a rename needs delete rights, which the account must
//!   not have). A `412` means the name is taken, and nothing is ever overwritten.
//! * **Never deletes, moves or overwrites** during normal operation: [`Provider::delete`] always
//!   fails and `can_delete` is false (so garbage collection is unavailable). The only `DELETE`
//!   in the crate is the credential check removing its own probe files.
//! * **Refuses to operate** until the credential check ([`crate::permissions`]) has proved
//!   that the account can create but not delete or overwrite. The check runs on the first
//!   operation of every provider instance (that is, once per unlocked session), and again on
//!   demand with [`WebDavProvider::verify`].
//! * Lists with `PROPFIND` (depth 1) one directory at a time; the entries' `href`s are never
//!   followed and must lie inside the sync root.
//! * Creates missing directories with `MKCOL` (allowed with create-only permissions: it needs the
//!   *create* right of the parent, not update or delete).

use crate::client::{Client, Options};
use crate::config::WebDavConfig;
use crate::error::DavError;
use crate::permissions::{self, Refusal, Verdict, mkcol, propfind};
use nuvoletta_sync::names::{Class, classify};
use nuvoletta_sync::{Capabilities, Provider, ProviderError, RemoteEntry};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Largest file read (a note is at most 16 MiB of body plus overhead).
const MAX_REMOTE_FILE: u64 = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 6;
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Debug)]
enum State {
    Unchecked,
    Safe,
    Refused(Refusal),
}

/// The WebDAV provider. See the module documentation.
pub struct WebDavProvider {
    client: Client,
    state: Mutex<State>,
    dirs: Mutex<HashSet<String>>,
    saw_etag: AtomicBool,
    saw_mtime: AtomicBool,
}

impl std::fmt::Debug for WebDavProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebDavProvider(..)")
    }
}

fn components(name: &str) -> Result<Vec<&str>, ProviderError> {
    let bad = || ProviderError::Other(format!("invalid name: {name:?}"));
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&str> = name.split('/').collect();
    for p in &parts {
        if p.is_empty()
            || *p == "."
            || *p == ".."
            || p.contains(['\\', '\0', ':'])
            || p.chars().any(char::is_control)
        {
            return Err(bad());
        }
    }
    Ok(parts)
}

fn encode(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| utf8_percent_encode(p, SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

impl WebDavProvider {
    /// A provider for `cfg`. Nothing is sent until the first operation.
    #[must_use]
    pub fn new(cfg: WebDavConfig, opts: Options) -> Self {
        Self {
            client: Client::new(cfg, opts),
            state: Mutex::new(State::Unchecked),
            dirs: Mutex::new(HashSet::new()),
            saw_etag: AtomicBool::new(false),
            saw_mtime: AtomicBool::new(false),
        }
    }

    /// Run the credential check now, as when the user has just entered the credentials (also
    /// when it passed before), and remember the result.
    ///
    /// # Errors
    /// [`DavError`] for connection, certificate and authentication problems; the permissions are
    /// then unknown and the provider stays unusable.
    pub fn verify(&self) -> Result<Verdict, DavError> {
        self.verify_with(true)
    }

    fn verify_with(&self, setup: bool) -> Result<Verdict, DavError> {
        let verdict = permissions::check(&self.client, setup)?;
        if let Ok(mut s) = self.state.lock() {
            *s = match &verdict {
                Verdict::Safe { .. } => State::Safe,
                Verdict::Refused(r) => State::Refused(r.clone()),
            };
        }
        Ok(verdict)
    }

    fn ensure_verified(&self) -> Result<(), ProviderError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ProviderError::Other("internal error".into()))?
            .clone();
        match state {
            State::Safe => Ok(()),
            State::Refused(r) => Err(ProviderError::UnsafeCredentials(r.explanation())),
            State::Unchecked => match self.verify_with(false)? {
                Verdict::Safe { .. } => Ok(()),
                Verdict::Refused(r) => Err(ProviderError::UnsafeCredentials(r.explanation())),
            },
        }
    }

    /// Path relative to the root named by an `href`, or `None` if it lies outside the root, has
    /// an unsafe segment or is the root itself (empty string for the root).
    fn rel_of_href(&self, href: &str) -> Option<String> {
        let cfg = self.client.config();
        let path = if let Some(rest) = href
            .strip_prefix("https://")
            .or_else(|| href.strip_prefix("http://"))
        {
            let (auth, p) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
            if !href.starts_with("https://")
                || !auth.eq_ignore_ascii_case(&cfg.origin()["https://".len()..])
            {
                return None;
            }
            format!("/{p}")
        } else {
            href.to_owned()
        };
        let decoded = percent_decode_str(&path).decode_utf8().ok()?;
        let base = percent_decode_str(cfg.base_path()).decode_utf8().ok()?;
        let rest = decoded.strip_prefix(base.as_ref()).or_else(|| {
            // The server may omit the trailing slash of the collection itself.
            (decoded.as_ref() == base.trim_end_matches('/')).then_some("")
        })?;
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() {
            return Some(String::new());
        }
        components(rest).ok()?;
        Some(rest.to_owned())
    }

    fn list_dir(
        &self,
        dir: &str,
        depth: usize,
        skip: &dyn Fn(&str) -> bool,
        out: &mut Vec<RemoteEntry>,
    ) -> Result<(), ProviderError> {
        let encoded = encode(&dir.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>());
        let props = match propfind(&self.client, &encoded, 1) {
            Ok(p) => p,
            Err(DavError::NotFound) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut subdirs = Vec::new();
        for p in props {
            let Some(rel) = self.rel_of_href(&p.href) else {
                continue; // outside the root or unsafe: never read as truth
            };
            let Some(child) = rel.strip_prefix(&prefix) else {
                continue; // the directory itself (or something unrelated)
            };
            if child.is_empty() || child.contains('/') {
                continue;
            }
            if p.is_collection {
                if depth < MAX_DEPTH && !skip(&rel) {
                    subdirs.push(rel);
                }
            } else {
                if p.etag.is_some() {
                    self.saw_etag.store(true, Ordering::Relaxed);
                }
                if p.mtime_ms.is_some() {
                    self.saw_mtime.store(true, Ordering::Relaxed);
                }
                out.push(RemoteEntry {
                    name: rel,
                    size: p.size.unwrap_or(0),
                    etag: p.etag,
                    mtime_ms: p.mtime_ms,
                });
            }
        }
        subdirs.sort();
        for d in subdirs {
            self.list_dir(&d, depth + 1, skip, out)?;
        }
        Ok(())
    }

    /// Create the directories of `dirs` (`MKCOL`, once per session each).
    fn ensure_dirs(&self, dirs: &[&str]) -> Result<(), ProviderError> {
        for k in 1..=dirs.len() {
            let path = dirs[..k].join("/");
            let known = self.dirs.lock().is_ok_and(|d| d.contains(&path));
            if known {
                continue;
            }
            mkcol(&self.client, &encode(&dirs[..k])).map_err(|e| match e {
                DavError::Forbidden => ProviderError::Forbidden(
                    "the account cannot create folders on the server".to_owned(),
                ),
                other => other.into(),
            })?;
            if let Ok(mut d) = self.dirs.lock() {
                d.insert(path);
            }
        }
        Ok(())
    }

    fn put_once(&self, rel: &str, bytes: &[u8]) -> Result<u16, ProviderError> {
        Ok(self
            .client
            .send_limited(
                "PUT",
                rel,
                &[
                    ("If-None-Match", "*"),
                    ("Content-Type", "application/octet-stream"),
                ],
                Some(bytes),
                64 * 1024,
            )?
            .status)
    }

    fn exists(&self, rel: &str) -> Result<bool, ProviderError> {
        let r = self.client.send_limited(
            "PROPFIND",
            rel,
            &[("Depth", "0"), ("Content-Type", "application/xml")],
            Some(permissions::PROPFIND_BODY),
            1024 * 1024,
        )?;
        Ok(r.status == 207)
    }
}

impl Provider for WebDavProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Overwriting is impossible for an account that passed the check, and a taken name
            // answers 412 (or 403, which is resolved by looking at the name).
            create_only: true,
            atomic_rename: false,
            etags: self.saw_etag.load(Ordering::Relaxed),
            mtimes: self.saw_mtime.load(Ordering::Relaxed),
            can_delete: false,
            authoritative: true,
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<RemoteEntry>, ProviderError> {
        self.ensure_verified()?;
        let parts = components(prefix)?;
        let mut out = Vec::new();
        self.list_dir(&parts.join("/"), 0, &|_| false, &mut out)?;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn list_cycle(&self) -> Result<Vec<RemoteEntry>, ProviderError> {
        self.ensure_verified()?;
        // The root and the `h`, `d`, `s` trees; never the 256 shards of `o/` (objects are
        // fetched by name) and never unknown directories.
        let mut out = Vec::new();
        let skip = |rel: &str| !rel.contains('/') && !matches!(rel, "h" | "d" | "s");
        self.list_dir("", 0, &skip, &mut out)?;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn get(&self, name: &str) -> Result<Vec<u8>, ProviderError> {
        self.ensure_verified()?;
        let parts = components(name)?;
        if parts.is_empty() {
            return Err(ProviderError::Other("empty name".into()));
        }
        let r = self
            .client
            .send_limited("GET", &encode(&parts), &[], None, MAX_REMOTE_FILE)?;
        match r.status {
            200 => Ok(r.body),
            404 => Err(ProviderError::NotFound(name.to_owned())),
            401 => Err(DavError::Auth.into()),
            403 => Err(DavError::Forbidden.into()),
            s => Err(DavError::Status(s).into()),
        }
    }

    fn put_new(&self, name: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        if !matches!(classify(name), Class::Protocol(_)) {
            return Err(ProviderError::Other(format!(
                "refusing to create non-protocol name: {name:?}"
            )));
        }
        self.ensure_verified()?;
        let parts = components(name)?;
        let Some((_, dirs)) = parts.split_last() else {
            return Err(ProviderError::Other("empty name".into()));
        };
        let rel = encode(&parts);
        let mut status = self.put_once(&rel, bytes)?;
        if matches!(status, 404 | 409) {
            // A parent folder is missing.
            self.ensure_dirs(dirs)?;
            status = self.put_once(&rel, bytes)?;
        }
        match status {
            200 | 201 | 204 => Ok(()),
            412 => Err(ProviderError::AlreadyExists(name.to_owned())),
            // Servers that check permissions before preconditions refuse with 403/405 when the
            // name exists: tell the two cases apart by looking.
            403 | 405 | 409 => {
                if self.exists(&rel)? {
                    Err(ProviderError::AlreadyExists(name.to_owned()))
                } else {
                    Err(ProviderError::Forbidden(
                        "the account cannot create files here".to_owned(),
                    ))
                }
            }
            401 => Err(DavError::Auth.into()),
            423 => Err(DavError::Locked.into()),
            507 => Err(ProviderError::Other(
                "the server has no space left for this account".to_owned(),
            )),
            413 => Err(ProviderError::Other(
                "the server refuses files of this size".to_owned(),
            )),
            s => Err(DavError::Status(s).into()),
        }
    }

    fn delete(&self, _name: &str) -> Result<(), ProviderError> {
        Err(ProviderError::Forbidden(
            "Nuvoletta never deletes files on WebDAV".to_owned(),
        ))
    }
}
