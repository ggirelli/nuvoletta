//! The connection settings of a WebDAV location, and the rules a URL must satisfy.
//!
//! Only `https` is accepted: the credentials travel in a `Basic` header and the notes are
//! encrypted but their names, sizes and timing are not, so plain HTTP is refused outright (there
//! is no "allow insecure" switch). A self-signed server certificate can be trusted by pinning its
//! SHA-256 fingerprint (see [`crate::tls`]).

use std::fmt;
use ureq::http::Uri;
use zeroize::Zeroizing;

/// Why a [`WebDavConfig`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The address is not a URL.
    #[error("The address is not a valid web address.")]
    BadUrl,
    /// The address does not start with `https://`.
    #[error(
        "The address must start with https://. Plain http:// is refused because your password and the sync traffic would be readable on the network."
    )]
    NotHttps,
    /// The address contains a user name or password.
    #[error("Do not put the user name or password in the address; enter them in their own fields.")]
    UserInfoInUrl,
    /// The address has a query or other parts that make no sense for a folder.
    #[error("The address must be a plain folder address without ? or # parts.")]
    QueryOrFragment,
    /// The path contains `..` or characters that cannot be part of a folder address.
    #[error("The folder path in the address contains characters that are not allowed.")]
    BadPath,
    /// No user name.
    #[error("Enter the user name.")]
    NoUsername,
    /// No password.
    #[error("Enter the password.")]
    NoPassword,
    /// The fingerprint is not 64 hexadecimal characters.
    #[error("A certificate fingerprint is 64 hexadecimal characters (SHA-256).")]
    BadFingerprint,
}

/// A WebDAV folder and the credentials to use it. The password is zeroized when dropped and is
/// never printed.
#[derive(Clone)]
pub struct WebDavConfig {
    origin: String,
    base_path: String,
    /// Normalised URL (with a trailing slash), as entered.
    url: String,
    username: String,
    password: Zeroizing<String>,
    pin: Option<[u8; 32]>,
}

impl fmt::Debug for WebDavConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebDavConfig")
            .field("url", &"<redacted>")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .field("pinned", &self.pin.is_some())
            .finish()
    }
}

fn path_ok(path: &str) -> bool {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    !decoded.contains('\\')
        && !decoded.chars().any(char::is_control)
        && decoded.split('/').all(|s| s != ".." && s != ".")
}

impl WebDavConfig {
    /// Validate and build a configuration.
    ///
    /// # Errors
    /// [`ConfigError`] with a message that can be shown to the user.
    pub fn new(
        url: &str,
        username: &str,
        password: &str,
        pin: Option<[u8; 32]>,
    ) -> Result<Self, ConfigError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(ConfigError::BadUrl);
        }
        if !url.to_ascii_lowercase().starts_with("https://") {
            return Err(ConfigError::NotHttps);
        }
        let uri: Uri = url.parse().map_err(|_| ConfigError::BadUrl)?;
        let authority = uri.authority().ok_or(ConfigError::BadUrl)?;
        if authority.as_str().contains('@') {
            return Err(ConfigError::UserInfoInUrl);
        }
        if authority.host().is_empty() {
            return Err(ConfigError::BadUrl);
        }
        if uri.query().is_some() || url.contains('#') || url.contains('?') {
            return Err(ConfigError::QueryOrFragment);
        }
        let mut path = uri.path().to_owned();
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        if !path.ends_with('/') {
            path.push('/');
        }
        if !path_ok(&path) || path.contains("//") {
            return Err(ConfigError::BadPath);
        }
        if username.is_empty() {
            return Err(ConfigError::NoUsername);
        }
        if password.is_empty() {
            return Err(ConfigError::NoPassword);
        }
        let origin = format!("https://{}", authority.as_str().to_ascii_lowercase());
        Ok(Self {
            url: format!("{origin}{path}"),
            origin,
            base_path: path,
            username: username.to_owned(),
            password: Zeroizing::new(password.to_owned()),
            pin,
        })
    }

    /// The same location with a pinned certificate fingerprint (or none).
    #[must_use]
    pub fn with_pin(mut self, pin: Option<[u8; 32]>) -> Self {
        self.pin = pin;
        self
    }

    /// `https://host[:port]`, lower case.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Absolute path of the sync root, with leading and trailing slash (percent-encoded as
    /// entered).
    #[must_use]
    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    /// The normalised folder address.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The user name.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The password. Only for sealing it into the device state and for building the request
    /// header; never log it.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// The pinned certificate fingerprint, if any.
    #[must_use]
    pub fn pin(&self) -> Option<&[u8; 32]> {
        self.pin.as_ref()
    }

    /// Host name (or address) and port of the server.
    #[must_use]
    pub fn host_port(&self) -> (String, u16) {
        let auth = &self.origin["https://".len()..];
        let (host, port) = match auth.rsplit_once(':') {
            Some((h, p))
                if !p.is_empty()
                    && p.bytes().all(|b| b.is_ascii_digit())
                    && !auth.ends_with(']') =>
            {
                (h, p.parse().unwrap_or(443))
            }
            _ => (auth, 443),
        };
        (
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned(),
            port,
        )
    }
}

/// Format a fingerprint as upper-case hex pairs separated by `:`.
#[must_use]
pub fn format_fingerprint(fp: &[u8; 32]) -> String {
    fp.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Parse a SHA-256 fingerprint written as hex, with or without `:`/space separators.
///
/// # Errors
/// [`ConfigError::BadFingerprint`].
pub fn parse_fingerprint(text: &str) -> Result<[u8; 32], ConfigError> {
    let hex: String = text
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect();
    if hex.len() != 64 || !hex.is_ascii() {
        return Err(ConfigError::BadFingerprint);
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| ConfigError::BadFingerprint)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_urls_are_normalised() {
        let c = WebDavConfig::new(
            " https://Cloud.Example.org:8443/remote.php/dav/files/me/Notes ",
            "me",
            "pw",
            None,
        )
        .unwrap();
        assert_eq!(
            c.url(),
            "https://cloud.example.org:8443/remote.php/dav/files/me/Notes/"
        );
        assert_eq!(c.base_path(), "/remote.php/dav/files/me/Notes/");
        assert_eq!(c.host_port(), ("cloud.example.org".to_owned(), 8443));
        let c = WebDavConfig::new("https://nas.local", "u", "p", None).unwrap();
        assert_eq!(c.base_path(), "/");
        assert_eq!(c.host_port(), ("nas.local".to_owned(), 443));
    }

    #[test]
    fn plain_http_and_odd_urls_are_refused() {
        let e = |u: &str| WebDavConfig::new(u, "u", "p", None).unwrap_err();
        assert_eq!(e("http://nas.local/dav/"), ConfigError::NotHttps);
        assert_eq!(e("HTTP://nas.local/dav/"), ConfigError::NotHttps);
        assert_eq!(e("ftp://nas.local/dav/"), ConfigError::NotHttps);
        assert_eq!(e("nas.local/dav/"), ConfigError::NotHttps);
        assert_eq!(
            e("https://me:secret@nas.local/dav/"),
            ConfigError::UserInfoInUrl
        );
        assert_eq!(
            e("https://nas.local/dav/?x=1"),
            ConfigError::QueryOrFragment
        );
        assert_eq!(e("https://nas.local/dav/#x"), ConfigError::QueryOrFragment);
        assert_eq!(e("https://nas.local/a/../b/"), ConfigError::BadPath);
        assert_eq!(e("https://nas.local/a//b/"), ConfigError::BadPath);
        assert_eq!(
            WebDavConfig::new("https://x/", "", "p", None).unwrap_err(),
            ConfigError::NoUsername
        );
        assert_eq!(
            WebDavConfig::new("https://x/", "u", "", None).unwrap_err(),
            ConfigError::NoPassword
        );
    }

    #[test]
    fn debug_never_shows_secrets() {
        let c = WebDavConfig::new("https://nas.local/dav/", "alice", "hunter2", None).unwrap();
        let d = format!("{c:?}");
        assert!(!d.contains("hunter2") && !d.contains("alice") && !d.contains("nas.local"));
    }

    #[test]
    fn fingerprints_round_trip() {
        let fp = [0xab; 32];
        let text = format_fingerprint(&fp);
        assert_eq!(parse_fingerprint(&text).unwrap(), fp);
        assert_eq!(
            parse_fingerprint(&text.to_lowercase().replace(':', "")).unwrap(),
            fp
        );
        assert!(parse_fingerprint("abcd").is_err());
        assert!(parse_fingerprint(&"zz".repeat(32)).is_err());
    }
}
