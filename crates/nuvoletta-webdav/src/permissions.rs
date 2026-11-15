//! The credential check: Nuvoletta only works against a WebDAV location where the account can
//! **read and create** files but can **neither delete nor overwrite** them, so that a stolen sync
//! password cannot destroy the remote copy (`docs/WEBDAV.md`, `docs/ADR/0007-webdav-backend.md`).
//!
//! The check proves the restriction by trying, never by trusting what the server says:
//!
//! 1. `PROPFIND` the root (depth 0). A Nextcloud/ownCloud server reports `oc:permissions`: a `D`
//!    (delete), `W` (write), `N` (rename) or `V` (move) refuses immediately without writing
//!    anything, and a missing `C` (create) refuses too. This is a fast path only; it never
//!    replaces the probe.
//! 2. `MKCOL` the protocol directories `h`, `d`, `o` (needed later anyway; this also proves that
//!    creating folders works with create-only permissions).
//! 3. In the root and in each of `h/`, `d/`, `o/` (permissions can differ per directory):
//!    make sure the stable probe `.nuvoletta-probe` exists (`PUT` with `If-None-Match: *`; an
//!    existing one is reused, so a server that cannot delete never collects more than a handful of
//!    probe files; when the user has just entered the credentials, one fresh
//!    `.nuvoletta-probe-<random>` is created in the root as well to prove that creating works),
//!    then try to overwrite it
//!    (`PUT`), copy over it (`COPY` with `Overwrite: T`), move it (`MOVE`) and delete it
//!    (`DELETE`). Every attempt must be *refused* (401, 403, 405 or 501); a success, or an
//!    answer that is neither success nor a clear refusal, ends the check. After each refusal the
//!    file is read back to make sure it is still there with the original content.
//! 4. If the account turned out to be unsafe, best-effort `DELETE` of the probe files (with the
//!    probes' own delete attempts, the only `DELETE`s the client ever sends, only for its own probe
//!    names). Probe files that remain are harmless: the names are outside the protocol grammar, so
//!    every device classifies them as stray and ignores them.
//!
//! Unknown means refuse: a missing answer, an odd status or a probe that cannot be completed is
//! [`RefusalReason::Indeterminate`], never "probably fine".

use crate::client::Client;
use crate::error::DavError;
use crate::xml::{Prop, parse_multistatus};

/// Prefix of the probe files. Not a protocol name: [`nuvoletta_sync::names::classify`] reports
/// it as stray.
pub const PROBE_PREFIX: &str = ".nuvoletta-probe-";
/// Name of the stable probe file (reused by every check and every device).
pub const PROBE_NAME: &str = ".nuvoletta-probe";

const V1: &[u8] = b"nuvoletta credential check (original)";
const V2: &[u8] = b"nuvoletta credential check (overwritten)";

/// The outcome of the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Create worked; overwrite, copy-over, move and delete were all refused.
    Safe {
        /// The probe files that exist on the server (they cannot be deleted: harmless, ignored
        /// by every device, reused by later checks).
        probe_files: Vec<String>,
    },
    /// The account must not be used.
    Refused(Refusal),
}

/// A refusal with the reason and any probe files left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Why.
    pub reason: RefusalReason,
    /// Probe files that could not be deleted.
    pub leftovers: Vec<String>,
}

/// Why a credential is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalReason {
    /// A delete succeeded (or the server reports delete permission) at the named place.
    CanDelete(String),
    /// An overwrite (or copy over an existing file) succeeded, or the server reports update
    /// permission.
    CanOverwrite(String),
    /// A move/rename succeeded, or the server reports rename permission.
    CanMove(String),
    /// The account cannot create files or folders: Nuvoletta needs read and create.
    CannotCreate(String),
    /// The result could not be determined; refused because unknown is unsafe.
    Indeterminate(String),
}

impl Refusal {
    /// A plain-words explanation for the user: what was found, why it matters and what to do.
    #[must_use]
    pub fn explanation(&self) -> String {
        let (found, extra) = match &self.reason {
            RefusalReason::CanDelete(w) => (format!("This account can delete files ({w})."), ""),
            RefusalReason::CanOverwrite(w) => (
                format!("This account can overwrite existing files ({w})."),
                "",
            ),
            RefusalReason::CanMove(w) => {
                (format!("This account can rename or move files ({w})."), "")
            }
            RefusalReason::CannotCreate(w) => {
                return format!(
                    "This account cannot create files or folders ({w}). Nuvoletta needs an account that can read and create files, but not change or delete them. {}",
                    SETUP_HINT
                );
            }
            RefusalReason::Indeterminate(d) => (
                format!("Nuvoletta could not find out what this account may do ({d})."),
                " When the permissions cannot be checked, the account is not used.",
            ),
        };
        format!(
            "{found}{extra} Nuvoletta refuses to use an account that can change or delete files: anyone who got hold of this password could wipe out all your synced notes on the server, and the synced copy is what protects you if a device is lost. {SETUP_HINT}"
        )
    }
}

/// What to do about a refusal.
pub const SETUP_HINT: &str = "Create a dedicated account, or a folder share, that can read and create files but not edit or delete them, and use that address and password instead (see the WebDAV setup guide, docs/WEBDAV.md).";

fn ok(status: u16) -> bool {
    matches!(status, 200 | 201 | 202 | 204)
}

fn refused(status: u16) -> bool {
    matches!(status, 401 | 403 | 405 | 501)
}

fn random_id() -> Result<String, DavError> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| DavError::Protocol(format!("no randomness: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn status(
    c: &Client,
    method: &str,
    rel: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<u16, DavError> {
    Ok(c.send_limited(method, rel, headers, body, 1024 * 1024)?
        .status)
}

/// What an `oc:permissions` string says about this account. `None` means nothing objectionable.
fn analyse_oc(perm: &str) -> Option<RefusalReason> {
    let here = "the server reports its permissions for this folder".to_owned();
    if perm.contains('D') {
        Some(RefusalReason::CanDelete(here))
    } else if perm.contains('W') {
        Some(RefusalReason::CanOverwrite(here))
    } else if perm.contains('N') || perm.contains('V') {
        Some(RefusalReason::CanMove(here))
    } else if !perm.contains('C') {
        Some(RefusalReason::CannotCreate(here))
    } else {
        None
    }
}

/// Request body of the `PROPFIND` calls (also asks for the ownCloud/Nextcloud permissions).
pub(crate) const PROPFIND_BODY: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop><d:resourcetype/><d:getcontentlength/><d:getetag/><d:getlastmodified/><oc:permissions/></d:prop></d:propfind>"#;

/// `PROPFIND` a directory of the sync root (`""` is the root).
pub(crate) fn propfind(c: &Client, dir: &str, depth: u8) -> Result<Vec<Prop>, DavError> {
    let rel = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let d = if depth == 0 { "0" } else { "1" };
    let r = c.send(
        "PROPFIND",
        &rel,
        &[
            ("Depth", d),
            ("Content-Type", "application/xml; charset=utf-8"),
        ],
        Some(PROPFIND_BODY),
    )?;
    match r.status {
        207 => parse_multistatus(&r.body),
        404 => Err(DavError::NotFound),
        401 => Err(DavError::Auth),
        403 => Err(DavError::Forbidden),
        s => Err(DavError::Status(s)),
    }
}

/// Create a protocol directory (idempotent: an existing one answers 405).
pub(crate) fn mkcol(c: &Client, dir: &str) -> Result<(), DavError> {
    match status(c, "MKCOL", &format!("{dir}/"), &[], None)? {
        s if ok(s) || s == 405 => Ok(()),
        401 => Err(DavError::Auth),
        403 => Err(DavError::Forbidden),
        409 => Err(DavError::Conflict),
        s => Err(DavError::Status(s)),
    }
}

/// Run the check. `setup` is true when the user has just entered the credentials: the check
/// then also creates one uniquely named probe file to prove that creating works. Errors are
/// connectivity and authentication problems (the permissions are then unknown and nothing may
/// be used).
///
/// # Errors
/// [`DavError`].
pub fn check(c: &Client, setup: bool) -> Result<Verdict, DavError> {
    let root = propfind(c, "", 0)?;
    if let Some(reason) = root
        .iter()
        .find_map(|p| p.oc_permissions.as_deref())
        .and_then(analyse_oc)
    {
        return Ok(Verdict::Refused(Refusal {
            reason,
            leftovers: Vec::new(),
        }));
    }
    for dir in ["h", "d", "o"] {
        match mkcol(c, dir) {
            Ok(()) => {}
            Err(DavError::Forbidden) => {
                return Ok(Verdict::Refused(Refusal {
                    reason: RefusalReason::CannotCreate(
                        "the folder for Nuvoletta's files could not be created".to_owned(),
                    ),
                    leftovers: Vec::new(),
                }));
            }
            Err(e) => return Err(e),
        }
    }
    let mut files: Vec<String> = Vec::new();
    if setup {
        // One fresh file proves that creating works even if the stable probes exist already.
        let rel = format!("{PROBE_PREFIX}{}", random_id()?);
        match status(c, "PUT", &rel, &[("If-None-Match", "*")], Some(V1))? {
            s if ok(s) => files.push(rel),
            s if refused(s) => {
                return Ok(Verdict::Refused(Refusal {
                    reason: RefusalReason::CannotCreate("in the main folder".to_owned()),
                    leftovers: Vec::new(),
                }));
            }
            s => {
                return Ok(Verdict::Refused(Refusal {
                    reason: RefusalReason::Indeterminate(format!(
                        "unexpected answer {s} when creating a test file in the main folder"
                    )),
                    leftovers: Vec::new(),
                }));
            }
        }
    }
    let mut verdict = None;
    for dir in ["", "h", "d", "o"] {
        match probe_dir(c, dir, dir.is_empty(), &mut files) {
            Ok(None) => {}
            Ok(Some(reason)) => {
                verdict = Some(reason);
                break;
            }
            Err(e) => {
                cleanup(c, &mut files);
                return Err(e);
            }
        }
    }
    Ok(match verdict {
        Some(reason) => {
            cleanup(c, &mut files);
            Verdict::Refused(Refusal {
                reason,
                leftovers: files,
            })
        }
        None => Verdict::Safe { probe_files: files },
    })
}

/// Best-effort removal of our own probe files; with the probes' `DELETE` attempts these are the
/// only `DELETE`s this client ever sends. Only used when the account turned out to be unsafe.
fn cleanup(c: &Client, files: &mut Vec<String>) {
    files.retain(|rel| {
        debug_assert!(
            rel.rsplit('/')
                .next()
                .is_some_and(|f| f.starts_with(PROBE_NAME))
        );
        !matches!(status(c, "DELETE", rel, &[], None), Ok(s) if ok(s) || s == 404)
    });
}

/// Make sure a stable probe file exists in the directory and return its content. Creates it if
/// missing; an existing one (from an earlier check, possibly by another device) is reused, which
/// is why a server that cannot delete never collects more than a handful of probe files.
fn ensure_probe(
    c: &Client,
    rel: &str,
    here: &str,
    files: &mut Vec<String>,
) -> Result<Result<Vec<u8>, RefusalReason>, DavError> {
    let st = status(
        c,
        "PUT",
        rel,
        &[
            ("If-None-Match", "*"),
            ("Content-Type", "application/octet-stream"),
        ],
        Some(V1),
    )?;
    let existing = |c: &Client| -> Result<Option<Vec<u8>>, DavError> {
        let r = c.send_limited("GET", rel, &[], None, 1024 * 1024)?;
        Ok((r.status == 200).then_some(r.body))
    };
    let body = if ok(st) {
        Some(V1.to_vec())
    } else if st == 412 || refused(st) {
        // Taken (412), or refused: a server that checks permissions before preconditions says
        // 403 for an existing name too. If it is there, reuse it; if not, creating is refused.
        existing(c)?
    } else {
        return Ok(Err(RefusalReason::Indeterminate(format!(
            "unexpected answer {st} when creating a test file {here}"
        ))));
    };
    match body {
        Some(b) => {
            files.push(rel.to_owned());
            Ok(Ok(b))
        }
        None => Ok(Err(RefusalReason::CannotCreate(here.to_owned()))),
    }
}

fn probe_dir(
    c: &Client,
    dir: &str,
    with_copy: bool,
    files: &mut Vec<String>,
) -> Result<Option<RefusalReason>, DavError> {
    let join = |n: &str| {
        if dir.is_empty() {
            n.to_owned()
        } else {
            format!("{dir}/{n}")
        }
    };
    let here = if dir.is_empty() {
        "in the main folder".to_owned()
    } else {
        format!("in the {dir}/ folder")
    };
    let indeterminate = |what: &str, s: u16| {
        Ok(Some(RefusalReason::Indeterminate(format!(
            "unexpected answer {s} when {what} {here}"
        ))))
    };
    let read_back = |rel: &str| -> Result<Option<Vec<u8>>, DavError> {
        let r = c.send_limited("GET", rel, &[], None, 1024 * 1024)?;
        Ok((r.status == 200).then_some(r.body))
    };
    // A different content every time, so that a silent overwrite is visible.
    let v2 = [V2, random_id()?.as_bytes()].concat();

    let a = join(PROBE_NAME);
    let orig = match ensure_probe(c, &a, &here, files)? {
        Ok(b) => b,
        Err(reason) => return Ok(Some(reason)),
    };
    // Overwrite.
    let s = status(
        c,
        "PUT",
        &a,
        &[("Content-Type", "application/octet-stream")],
        Some(&v2),
    )?;
    if ok(s) {
        return Ok(Some(RefusalReason::CanOverwrite(here)));
    }
    if !refused(s) {
        return indeterminate("overwriting a test file", s);
    }
    match read_back(&a)? {
        Some(b) if b == orig => {}
        Some(b) if b == v2 => return Ok(Some(RefusalReason::CanOverwrite(here))),
        _ => return indeterminate("reading back a test file", 0),
    }
    // Copy over an existing file.
    let b = join(&format!("{PROBE_NAME}-b"));
    if with_copy {
        if let Err(reason) = ensure_probe(c, &b, &here, files)? {
            return Ok(Some(reason));
        }
        let dest = c.url_of(&b);
        let s = status(
            c,
            "COPY",
            &a,
            &[("Destination", &dest), ("Overwrite", "T")],
            None,
        )?;
        if ok(s) {
            return Ok(Some(RefusalReason::CanOverwrite(here)));
        }
        if !refused(s) {
            return indeterminate("copying over a test file", s);
        }
    }
    // Move.
    let moved = join(&format!("{PROBE_NAME}-moved"));
    let dest = c.url_of(&moved);
    let s = status(
        c,
        "MOVE",
        &a,
        &[("Destination", &dest), ("Overwrite", "F")],
        None,
    )?;
    if ok(s) {
        files.retain(|x| x != &a);
        files.push(moved);
        return Ok(Some(RefusalReason::CanMove(here)));
    }
    if !refused(s) {
        return indeterminate("moving a test file", s);
    }
    // Delete.
    let targets: Vec<&String> = if with_copy { vec![&a, &b] } else { vec![&a] };
    for rel in targets {
        let s = status(c, "DELETE", rel, &[], None)?;
        if ok(s) {
            files.retain(|x| x != rel);
            return Ok(Some(RefusalReason::CanDelete(here)));
        }
        if !refused(s) {
            return indeterminate("deleting a test file", s);
        }
        if read_back(rel)?.is_none() {
            return Ok(Some(RefusalReason::CanDelete(here)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oc_permission_letters() {
        assert!(matches!(
            analyse_oc("RGDNVCK"),
            Some(RefusalReason::CanDelete(_))
        ));
        assert!(matches!(
            analyse_oc("SRGCKW"),
            Some(RefusalReason::CanOverwrite(_))
        ));
        assert!(matches!(
            analyse_oc("SGCKNV"),
            Some(RefusalReason::CanMove(_))
        ));
        assert!(matches!(
            analyse_oc("SG"),
            Some(RefusalReason::CannotCreate(_))
        ));
        assert_eq!(analyse_oc("SGCK"), None);
    }

    #[test]
    fn probe_names_are_stray_to_the_protocol() {
        use nuvoletta_sync::names::{Class, classify};
        for p in [
            format!("{PROBE_PREFIX}0123456789abcdef"),
            PROBE_NAME.to_owned(),
            format!("h/{PROBE_NAME}-moved"),
            format!("d/{PROBE_NAME}-b"),
            format!("o/{PROBE_NAME}"),
        ] {
            assert_eq!(classify(&p), Class::Stray, "{p}");
        }
    }

    #[test]
    fn explanations_are_plain_and_name_the_fix() {
        let r = Refusal {
            reason: RefusalReason::CanDelete("in the main folder".into()),
            leftovers: vec![],
        };
        let t = r.explanation();
        assert!(t.contains("could wipe out") && t.contains("read and create"));
    }
}
