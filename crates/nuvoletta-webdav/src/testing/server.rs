//! A small in-process WebDAV server over TLS for the tests of this crate and of the application
//! core (feature `test-server`; never part of a release build).
//!
//! Why hand-written rather than `dav-server`: the tests need to control exactly what the server
//! permits (create but not overwrite, delete allowed per directory, `If-None-Match` honoured or
//! not, `oc:permissions` reported or not) and to inject faults (throttling, delays, redirects,
//! interrupted uploads). A permission-restricting wrapper around a full server would hide those
//! behaviours behind its own, and would pull in hyper and tokio for tests that must stay small.
//! This server is about 400 lines, synchronous, in memory, and speaks just the HTTP/1.1 and
//! WebDAV subset the client uses.

#![allow(dead_code, missing_docs, clippy::all)]

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const USER: &str = "alice";
pub const PASS: &str = "s3cret-app-password";
pub const BASE: &str = "/dav/";

/// What the account may do.
#[derive(Clone, Debug)]
pub struct Policy {
    pub allow_create: bool,
    pub allow_mkcol: bool,
    pub allow_overwrite: bool,
    pub allow_copy_over: bool,
    pub allow_move: bool,
    /// Delete is allowed in these directories ("" is the root); `all_delete` allows everywhere.
    pub delete_in: HashSet<String>,
    pub all_delete: bool,
    /// Honour `If-None-Match: *` with 412 (otherwise a taken name answers 403 when overwrite is
    /// not allowed, like a server that checks permissions first).
    pub honour_if_none_match: bool,
    /// Report this `oc:permissions` for collections.
    pub oc_permissions: Option<String>,
    pub send_etags: bool,
    pub send_mtimes: bool,
    /// Status returned for DELETE instead of the real behaviour (for fail-closed tests).
    pub delete_status_override: Option<u16>,
}

impl Policy {
    /// Read + create only: the credential Nuvoletta wants.
    pub fn create_only() -> Self {
        Self {
            allow_create: true,
            allow_mkcol: true,
            allow_overwrite: false,
            allow_copy_over: false,
            allow_move: false,
            delete_in: HashSet::new(),
            all_delete: false,
            honour_if_none_match: true,
            oc_permissions: None,
            send_etags: true,
            send_mtimes: true,
            delete_status_override: None,
        }
    }

    pub fn full() -> Self {
        Self {
            allow_overwrite: true,
            allow_copy_over: true,
            allow_move: true,
            all_delete: true,
            ..Self::create_only()
        }
    }

    fn can_delete(&self, dir: &str) -> bool {
        self.all_delete || self.delete_in.contains(dir)
    }
}

/// Faults to inject.
#[derive(Default, Clone, Debug)]
pub struct Faults {
    /// Answer 503 (with this `Retry-After`) to the next N requests of any kind.
    pub throttle: Option<(usize, Option<u32>)>,
    /// Sleep this long before answering requests whose path contains the text.
    pub delay: Vec<(String, Duration)>,
    /// A PUT whose path contains the text stores only half of the body and then drops the
    /// connection without answering (an interrupted upload on a server that keeps partials).
    /// Applies this many times per entry (a client may retry once on a fresh connection).
    pub partial_put: Vec<(String, usize)>,
    /// Redirect every request whose path contains the text to this Location.
    pub redirect: Vec<(String, String)>,
    /// Extra `href`s (hostile or foreign) appended to every PROPFIND answer as plain files.
    pub extra_hrefs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct File {
    pub data: Vec<u8>,
    pub etag: u64,
    pub mtime: i64,
}

#[derive(Default)]
pub struct Store {
    pub files: BTreeMap<String, File>,
    pub dirs: BTreeSet<String>,
    pub policy: Option<Policy>,
    pub faults: Faults,
    pub log: Vec<(String, String)>,
    counter: u64,
}

pub struct TestServer {
    pub port: u16,
    pub fingerprint: [u8; 32],
    pub store: Arc<Mutex<Store>>,
    stop: Arc<AtomicBool>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn b64decode(s: &str) -> Vec<u8> {
    let t = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| *c != b'=') {
        let Some(v) = t.iter().position(|x| *x == c) else {
            return Vec::new();
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parent_of(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(p, _)| p.to_owned())
}

struct Req {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    close: bool,
}

impl Resp {
    fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            close: false,
        }
    }
}

impl Store {
    fn bump(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// `path` relative to BASE without trailing slash; None if outside.
    fn rel(path: &str) -> Option<String> {
        let p = pct_decode(path);
        let p = p
            .strip_prefix(BASE)
            .or_else(|| (p == BASE.trim_end_matches('/')).then_some(""))?;
        Some(p.trim_end_matches('/').to_owned())
    }

    fn dir_exists(&self, rel: &str) -> bool {
        rel.is_empty() || self.dirs.contains(rel)
    }

    fn propfind(&self, rel: &str, depth: &str) -> Resp {
        let policy = self.policy.clone().unwrap_or_else(Policy::create_only);
        let is_dir = self.dir_exists(rel);
        let is_file = self.files.contains_key(rel);
        if !is_dir && !is_file {
            return Resp::new(404);
        }
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">"#,
        );
        let entry = |xml: &mut String, rel: &str, dir: bool, f: Option<&File>| {
            let href = if rel.is_empty() {
                BASE.to_owned()
            } else {
                format!("{BASE}{rel}{}", if dir { "/" } else { "" })
            };
            xml.push_str(&format!(
                "<d:response><d:href>{href}</d:href><d:propstat><d:prop>"
            ));
            if dir {
                xml.push_str("<d:resourcetype><d:collection/></d:resourcetype>");
                if let Some(p) = &policy.oc_permissions {
                    xml.push_str(&format!("<oc:permissions>{p}</oc:permissions>"));
                }
            } else {
                xml.push_str("<d:resourcetype/>");
                if let Some(f) = f {
                    xml.push_str(&format!(
                        "<d:getcontentlength>{}</d:getcontentlength>",
                        f.data.len()
                    ));
                    if policy.send_etags {
                        xml.push_str(&format!("<d:getetag>\"{}\"</d:getetag>", f.etag));
                    }
                    if policy.send_mtimes {
                        xml.push_str(
                            "<d:getlastmodified>Tue, 15 Nov 1994 08:12:31 GMT</d:getlastmodified>",
                        );
                    }
                }
            }
            xml.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        };
        entry(&mut xml, rel, is_dir, self.files.get(rel));
        if is_dir && depth != "0" {
            let prefix = if rel.is_empty() {
                String::new()
            } else {
                format!("{rel}/")
            };
            for d in &self.dirs {
                if let Some(c) = d.strip_prefix(&prefix) {
                    if !c.is_empty() && !c.contains('/') {
                        entry(&mut xml, d, true, None);
                    }
                }
            }
            for (n, f) in &self.files {
                if let Some(c) = n.strip_prefix(&prefix) {
                    if !c.is_empty() && !c.contains('/') {
                        entry(&mut xml, n, false, Some(f));
                    }
                }
            }
        }
        for h in &self.faults.extra_hrefs {
            xml.push_str(&format!("<d:response><d:href>{h}</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>5</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"));
        }
        xml.push_str("</d:multistatus>");
        let mut r = Resp::new(207);
        r.headers.push((
            "Content-Type".into(),
            "application/xml; charset=utf-8".into(),
        ));
        r.body = xml.into_bytes();
        r
    }

    fn handle(&mut self, req: &Req) -> Resp {
        self.log.push((req.method.clone(), req.path.clone()));
        let policy = self.policy.clone().unwrap_or_else(Policy::create_only);
        // Faults.
        if let Some((n, ra)) = &mut self.faults.throttle {
            if *n > 0 {
                *n -= 1;
                let mut r = Resp::new(503);
                if let Some(s) = ra {
                    r.headers.push(("Retry-After".into(), s.to_string()));
                }
                return r;
            }
        }
        for (pat, loc) in &self.faults.redirect {
            if req.path.contains(pat.as_str()) {
                let mut r = Resp::new(302);
                r.headers.push(("Location".into(), loc.clone()));
                return r;
            }
        }
        let Some(rel) = Self::rel(&req.path) else {
            return Resp::new(404);
        };
        let dir_of = |r: &str| parent_of(r);
        match req.method.as_str() {
            "PROPFIND" => self.propfind(&rel, req.headers.get("depth").map_or("1", String::as_str)),
            "GET" => match self.files.get(&rel) {
                Some(f) => {
                    let mut r = Resp::new(200);
                    r.body = f.data.clone();
                    r
                }
                None => Resp::new(404),
            },
            "MKCOL" => {
                if self.dir_exists(&rel) || self.files.contains_key(&rel) {
                    return Resp::new(405);
                }
                if !policy.allow_mkcol {
                    return Resp::new(403);
                }
                if !self.dir_exists(&dir_of(&rel)) {
                    return Resp::new(409);
                }
                self.dirs.insert(rel);
                Resp::new(201)
            }
            "PUT" => {
                if !self.dir_exists(&dir_of(&rel)) {
                    return Resp::new(409);
                }
                let exists = self.files.contains_key(&rel);
                let inm = req.headers.get("if-none-match").is_some_and(|v| v == "*");
                if exists {
                    if inm && policy.honour_if_none_match {
                        return Resp::new(412);
                    }
                    if !policy.allow_overwrite {
                        return Resp::new(403);
                    }
                } else if !policy.allow_create {
                    return Resp::new(403);
                }
                let etag = self.bump();
                self.files.insert(
                    rel,
                    File {
                        data: req.body.clone(),
                        etag,
                        mtime: 0,
                    },
                );
                Resp::new(if exists { 204 } else { 201 })
            }
            "DELETE" => {
                if let Some(s) = policy.delete_status_override {
                    return Resp::new(s);
                }
                if !policy.can_delete(&dir_of(&rel)) {
                    return Resp::new(403);
                }
                if self.files.remove(&rel).is_some() {
                    Resp::new(204)
                } else {
                    Resp::new(404)
                }
            }
            "MOVE" | "COPY" => {
                let Some(dest) = req.headers.get("destination") else {
                    return Resp::new(400);
                };
                let dest_path = dest
                    .split_once("://")
                    .and_then(|(_, r)| r.split_once('/'))
                    .map_or(dest.clone(), |(_, p)| format!("/{p}"));
                let Some(drel) = Self::rel(&dest_path) else {
                    return Resp::new(502);
                };
                let Some(src) = self.files.get(&rel).cloned() else {
                    return Resp::new(404);
                };
                let dexists = self.files.contains_key(&drel);
                let overwrite = req.headers.get("overwrite").is_none_or(|v| v == "T");
                if req.method == "MOVE" {
                    if !policy.allow_move {
                        return Resp::new(403);
                    }
                    if dexists && !overwrite {
                        return Resp::new(412);
                    }
                    self.files.remove(&rel);
                } else if dexists && (!overwrite || !policy.allow_copy_over) {
                    return Resp::new(if overwrite { 403 } else { 412 });
                }
                self.files.insert(drel, src);
                Resp::new(if dexists { 204 } else { 201 })
            }
            _ => Resp::new(501),
        }
    }
}

fn read_request<R: BufRead>(r: &mut R) -> Option<Req> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut it = line.split_whitespace();
    let method = it.next()?.to_owned();
    let path = it.next()?.to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

fn write_response<W: Write>(w: &mut W, r: &Resp) -> std::io::Result<()> {
    let reason = match r.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\n",
        r.status,
        if r.status == 204 { 0 } else { r.body.len() }
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    if r.status != 204 {
        w.write_all(&r.body)?;
    }
    w.flush()
}

fn serve<S: Read + Write>(stream: S, store: &Arc<Mutex<Store>>) {
    let mut reader = BufReader::new(stream);
    while let Some(req) = read_request(&mut reader) {
        let auth_ok = req
            .headers
            .get("authorization")
            .and_then(|a| a.strip_prefix("Basic "))
            .is_some_and(|b| b64decode(b) == format!("{USER}:{PASS}").as_bytes());
        let (delay, partial) = {
            let mut st = store.lock().unwrap();
            let delay = st
                .faults
                .delay
                .iter()
                .find(|(p, _)| req.path.contains(p.as_str()))
                .map(|(_, d)| *d);
            let partial = req.method == "PUT"
                && st
                    .faults
                    .partial_put
                    .iter_mut()
                    .find(|(p, n)| *n > 0 && req.path.contains(p.as_str()))
                    .map(|(_, n)| *n -= 1)
                    .is_some();
            (delay, partial)
        };
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        if partial {
            let mut st = store.lock().unwrap();
            if let Some(rel) = Store::rel(&req.path) {
                let half = req.body[..req.body.len() / 2].to_vec();
                st.files.insert(
                    rel,
                    File {
                        data: half,
                        etag: 1,
                        mtime: 0,
                    },
                );
            }
            return; // drop the connection without an answer
        }
        let resp = if auth_ok {
            store.lock().unwrap().handle(&req)
        } else {
            let mut r = Resp::new(401);
            r.headers
                .push(("WWW-Authenticate".into(), "Basic realm=\"test\"".into()));
            r
        };
        if write_response(reader.get_mut(), &resp).is_err() || resp.close {
            return;
        }
    }
}

impl TestServer {
    pub fn start(policy: Policy) -> Self {
        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
        ])
        .unwrap();
        let cert_der: CertificateDer<'static> = cert.cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
        let fingerprint: [u8; 32] = Sha256::digest(cert_der.as_ref()).into();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = Arc::new(
            ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert_der], key)
                .unwrap(),
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let store = Arc::new(Mutex::new(Store {
            policy: Some(policy),
            ..Store::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (st2, stop2) = (store.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop2.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(tcp) = conn else { continue };
                let (config, store) = (config.clone(), st2.clone());
                std::thread::spawn(move || {
                    let Ok(conn) = ServerConnection::new(config) else {
                        return;
                    };
                    serve(StreamOwned::new(conn, tcp), &store);
                });
            }
        });
        Self {
            port,
            fingerprint,
            store,
            stop,
        }
    }

    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}{BASE}", self.port)
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(&mut self.store.lock().unwrap())
    }

    pub fn set_policy(&self, p: Policy) {
        self.with(|s| s.policy = Some(p));
    }

    pub fn methods(&self) -> Vec<String> {
        self.with(|s| s.log.iter().map(|(m, _)| m.clone()).collect())
    }

    pub fn clear_log(&self) {
        self.with(|s| s.log.clear());
    }

    pub fn file_names(&self) -> Vec<String> {
        self.with(|s| s.files.keys().cloned().collect())
    }
}
