//! A small blocking WebDAV client: one request at a time over HTTPS with `Basic` credentials.
//!
//! Rules enforced here for every request:
//!
//! * HTTPS only (`ureq`'s `https_only`), no proxy from the environment;
//! * redirects are never followed blindly: a redirect to the same origin is followed (at most
//!   [`MAX_REDIRECTS`] times), anything else is an error and the credentials are never sent
//!   anywhere but the configured origin;
//! * 429 and 503 are retried after the server's `Retry-After` (capped), then reported as
//!   throttling;
//! * response bodies are size-limited.

use crate::config::WebDavConfig;
use crate::error::DavError;
use crate::tls::{PinnedConnector, map_ureq};
use std::time::Duration;
use ureq::Agent;
use ureq::config::Config;
use ureq::http::Request;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{Connector, TcpConnector};
use zeroize::Zeroizing;

/// Redirects followed per request (same origin only).
pub const MAX_REDIRECTS: usize = 3;
/// Largest listing answer read.
const MAX_LISTING: u64 = 32 * 1024 * 1024;

/// Pause and attempt policy for throttled requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts per request (1 = never retry).
    pub attempts: u32,
    /// Longest pause honoured; a longer `Retry-After` ends the retries and is reported.
    pub max_wait: Duration,
    /// Pause when the server gave none, doubled per attempt.
    pub base_wait: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            max_wait: Duration::from_secs(10),
            base_wait: Duration::from_secs(1),
        }
    }
}

/// Timeouts and retries; the defaults suit a phone on a slow network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Time to establish the connection.
    pub connect_timeout: Duration,
    /// Time to wait for the response head after the request was sent.
    pub response_timeout: Duration,
    /// Time for a whole request, including transferring the body.
    pub total_timeout: Duration,
    /// Throttling policy.
    pub retry: RetryPolicy,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            response_timeout: Duration::from_secs(60),
            total_timeout: Duration::from_secs(300),
            retry: RetryPolicy::default(),
        }
    }
}

/// What a request returned.
#[derive(Debug)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// `ETag` header.
    pub etag: Option<String>,
    /// Body (empty for most write operations).
    pub body: Vec<u8>,
}

/// Standard base64 (RFC 4648) for the `Basic` header.
fn base64(input: &[u8]) -> Zeroizing<String> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Zeroizing::new(String::with_capacity(input.len().div_ceil(3) * 4));
    for c in input.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(char::from(T[(n >> 18) as usize & 63]));
        out.push(char::from(T[(n >> 12) as usize & 63]));
        out.push(if c.len() > 1 {
            char::from(T[(n >> 6) as usize & 63])
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            char::from(T[n as usize & 63])
        } else {
            '='
        });
    }
    out
}

/// The HTTP layer.
pub struct Client {
    agent: Agent,
    cfg: WebDavConfig,
    auth: Zeroizing<String>,
    opts: Options,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client(..)")
    }
}

fn parse_retry_after(v: &str) -> Option<Duration> {
    // Only the delta-seconds form; an HTTP date is treated as "unspecified".
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

impl Client {
    /// Build a client for `cfg`.
    #[must_use]
    pub fn new(cfg: WebDavConfig, opts: Options) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::WebPki)
            .build();
        let config = Config::builder()
            .https_only(true)
            .proxy(None)
            .allow_non_standard_methods(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(Some(opts.connect_timeout))
            .timeout_recv_response(Some(opts.response_timeout))
            .timeout_global(Some(opts.total_timeout))
            .user_agent("nuvoletta")
            .tls_config(tls)
            .build();
        let agent = match cfg.pin() {
            Some(pin) => {
                let connector = ().chain(TcpConnector::default()).chain(PinnedConnector::new(*pin));
                Agent::with_parts(config, connector, DefaultResolver::default())
            }
            None => Agent::new_with_config(config),
        };
        let mut raw = Zeroizing::new(format!("{}:{}", cfg.username(), cfg.password()).into_bytes());
        let auth = Zeroizing::new(format!("Basic {}", base64(&raw).as_str()));
        raw.fill(0);
        Self {
            agent,
            cfg,
            auth,
            opts,
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &WebDavConfig {
        &self.cfg
    }

    /// Absolute URL for a path relative to the sync root (already percent-encoded segments,
    /// `/`-separated; a trailing `/` marks a collection).
    #[must_use]
    pub fn url_of(&self, rel: &str) -> String {
        format!("{}{}{}", self.cfg.origin(), self.cfg.base_path(), rel)
    }

    /// Send one request, applying the throttling and redirect rules. Any HTTP status is
    /// returned as `Ok` (use [`Response::status`]); only transport-level failures, redirects off
    /// the origin and persistent throttling are errors.
    ///
    /// # Errors
    /// [`DavError`].
    pub fn send(
        &self,
        method: &str,
        rel: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<Response, DavError> {
        self.send_url(method, &self.url_of(rel), headers, body, MAX_LISTING)
    }

    /// Like [`Client::send`] with an explicit body size limit.
    ///
    /// # Errors
    /// [`DavError`].
    pub fn send_limited(
        &self,
        method: &str,
        rel: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        limit: u64,
    ) -> Result<Response, DavError> {
        self.send_url(method, &self.url_of(rel), headers, body, limit)
    }

    fn send_url(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        limit: u64,
    ) -> Result<Response, DavError> {
        let mut url = url.to_owned();
        let mut redirects = 0;
        let mut attempt: u32 = 0;
        loop {
            let (status, location, retry_after, resp) =
                self.once(method, &url, headers, body, limit)?;
            if matches!(status, 301 | 302 | 307 | 308) {
                let target = location
                    .ok_or_else(|| DavError::Redirect("the redirect names no target".to_owned()))?;
                url = self.same_origin(&url, &target)?;
                redirects += 1;
                if redirects > MAX_REDIRECTS {
                    return Err(DavError::Redirect("too many redirects".to_owned()));
                }
                continue;
            }
            if status == 303 {
                return Err(DavError::Redirect(
                    "the server redirected to another address; enter the final address".to_owned(),
                ));
            }
            if matches!(status, 429 | 503) {
                attempt += 1;
                let wait = retry_after.unwrap_or_else(|| {
                    self.opts.retry.base_wait * 2u32.saturating_pow(attempt.saturating_sub(1))
                });
                if attempt >= self.opts.retry.attempts || wait > self.opts.retry.max_wait {
                    return Err(DavError::Throttled { retry_after });
                }
                std::thread::sleep(wait);
                continue;
            }
            return Ok(resp);
        }
    }

    fn once(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        limit: u64,
    ) -> Result<(u16, Option<String>, Option<Duration>, Response), DavError> {
        let mut b = Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", self.auth.as_str());
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = b
            .body(body.unwrap_or(&[]).to_vec())
            .map_err(|e| DavError::Protocol(format!("bad request: {e}")))?;
        let pinned = self.cfg.pin().is_some();
        let resp = self.agent.run(req).map_err(|e| map_ureq(e, pinned))?;
        let status = resp.status().as_u16();
        let h = |n: &str| {
            resp.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let location = h("location");
        let retry_after = h("retry-after").and_then(|v| parse_retry_after(&v));
        let etag = h("etag");
        let bytes = resp
            .into_body()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| map_ureq(e, pinned))?;
        Ok((
            status,
            location,
            retry_after,
            Response {
                status,
                etag,
                body: bytes,
            },
        ))
    }

    /// Resolve a `Location` against `current` and require the same origin.
    fn same_origin(&self, current: &str, location: &str) -> Result<String, DavError> {
        let origin = self.cfg.origin();
        let target = if location.starts_with('/') && !location.starts_with("//") {
            format!("{origin}{location}")
        } else if location.starts_with("https://") || location.starts_with("http://") {
            location.to_owned()
        } else if location.contains("://") || location.starts_with("//") {
            return Err(DavError::Redirect(
                "the redirect target is not a valid address".to_owned(),
            ));
        } else {
            // Relative to the directory of the current path.
            let dir = &current[..=current.rfind('/').unwrap_or(0)];
            format!("{dir}{location}")
        };
        let same_origin = target
            .strip_prefix(origin)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
        if same_origin && !target.contains(['\r', '\n', ' ']) {
            Ok(target)
        } else {
            Err(DavError::Redirect(
                "the server redirected to a different address (not followed, to protect your password)"
                    .to_owned(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()).as_str(), o);
        }
    }
}
