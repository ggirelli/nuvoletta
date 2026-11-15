//! Trusting a server: the built-in web PKI roots (`webpki-roots`, no operating-system store and
//! so no JNI on Android), or one certificate pinned by its SHA-256 fingerprint.
//!
//! Pinning is for servers with a self-signed certificate, typical of a NAS. The user confirms the
//! fingerprint once ([`fetch_fingerprint`] performs only the TLS handshake and sends nothing);
//! afterwards a connection is accepted only if the server's leaf certificate hashes to exactly
//! that value. Name and validity period are then not checked (the pin identifies the exact
//! certificate), but the handshake signature is, so the peer must hold the certificate's key.
//! If the server renews its certificate the pin no longer matches and the user must confirm the
//! new fingerprint again; there is never a silent fallback.

use crate::error::{DavError, TlsFailure};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError,
    SignatureScheme, StreamOwned,
};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, Transport,
    TransportAdapter,
};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// SHA-256 of a DER certificate.
#[must_use]
pub fn fingerprint_of(cert: &[u8]) -> [u8; 32] {
    Sha256::digest(cert).into()
}

/// Accepts the one certificate with the pinned fingerprint.
#[derive(Debug)]
struct PinVerifier {
    pin: [u8; 32],
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        use subtle_eq::ct_eq;
        if ct_eq(&fingerprint_of(end_entity), &self.pin) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Constant-time-ish equality for two fingerprints (not secret, but cheap to do right).
mod subtle_eq {
    pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
        a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
    }
}

/// Records the leaf certificate and accepts everything: only ever used for the handshake of
/// [`fetch_fingerprint`], where nothing but the handshake is sent.
#[derive(Debug)]
struct Recorder {
    seen: Mutex<Option<Vec<u8>>>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if let Ok(mut g) = self.seen.lock() {
            *g = Some(end_entity.as_ref().to_vec());
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn server_name(host: &str) -> Result<ServerName<'static>, DavError> {
    ServerName::try_from(host.to_owned())
        .map_err(|_| DavError::Protocol("the host name is not valid".to_owned()))
}

/// Connect to `host:port`, do the TLS handshake and return the SHA-256 fingerprint of the
/// server's certificate, **without verifying it** and without sending anything else. The caller
/// shows it to the user, who must compare it with the one the server shows.
///
/// # Errors
/// [`DavError::Network`] / [`DavError::Timeout`] if the server cannot be reached,
/// [`DavError::Tls`] if the handshake fails.
pub fn fetch_fingerprint(host: &str, port: u16, timeout: Duration) -> Result<[u8; 32], DavError> {
    let prov = provider();
    let recorder = Arc::new(Recorder {
        seen: Mutex::new(None),
        algs: prov.signature_verification_algorithms,
    });
    let config = ClientConfig::builder_with_provider(prov)
        .with_safe_default_protocol_versions()
        .map_err(|e| DavError::Tls(TlsFailure::Other(e.to_string())))?
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| DavError::Network(format!("cannot resolve the server name: {e}")))?
        .next()
        .ok_or_else(|| DavError::Network("the server name has no address".to_owned()))?;
    let mut sock = TcpStream::connect_timeout(&addr, timeout).map_err(map_io)?;
    sock.set_read_timeout(Some(timeout)).ok();
    sock.set_write_timeout(Some(timeout)).ok();
    let mut conn = ClientConnection::new(Arc::new(config), server_name(host)?)
        .map_err(|e| DavError::Tls(TlsFailure::Other(e.to_string())))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock)
            .map_err(|e| classify_io(&e, false))?;
    }
    let cert = recorder
        .seen
        .lock()
        .ok()
        .and_then(|mut g| g.take())
        .ok_or_else(|| DavError::Tls(TlsFailure::Other("no certificate was presented".into())))?;
    conn.send_close_notify();
    let _ = conn.write_tls(&mut sock);
    Ok(fingerprint_of(&cert))
}

fn map_io(e: std::io::Error) -> DavError {
    use std::io::ErrorKind::{TimedOut, WouldBlock};
    match e.kind() {
        TimedOut | WouldBlock => DavError::Timeout(e.to_string()),
        _ => DavError::Network(e.to_string()),
    }
}

fn classify_rustls(e: &TlsError, pinned: bool) -> DavError {
    match e {
        TlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
            if pinned =>
        {
            DavError::Tls(TlsFailure::PinMismatch)
        }
        TlsError::InvalidCertificate(c) => DavError::Tls(TlsFailure::Untrusted(match c {
            CertificateError::UnknownIssuer => {
                "it is not signed by an authority this app trusts".to_owned()
            }
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                "it is issued for a different name than the address you entered".to_owned()
            }
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                "it has expired".to_owned()
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                "it is not valid yet (check the clock of this device)".to_owned()
            }
            other => format!("{other:?}"),
        })),
        other => DavError::Tls(TlsFailure::Other(other.to_string())),
    }
}

/// Map an I/O error that may wrap a rustls error (as `complete_io` reports them).
pub(crate) fn classify_io(e: &std::io::Error, pinned: bool) -> DavError {
    if let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<TlsError>()) {
        return classify_rustls(inner, pinned);
    }
    map_io_ref(e)
}

fn map_io_ref(e: &std::io::Error) -> DavError {
    use std::io::ErrorKind::{TimedOut, WouldBlock};
    match e.kind() {
        TimedOut | WouldBlock => DavError::Timeout(e.to_string()),
        _ => DavError::Network(e.to_string()),
    }
}

/// Map an error of `ureq` to ours.
pub(crate) fn map_ureq(e: ureq::Error, pinned: bool) -> DavError {
    match e {
        ureq::Error::Timeout(t) => DavError::Timeout(t.to_string()),
        ureq::Error::Io(io) => classify_io(&io, pinned),
        ureq::Error::Rustls(r) => classify_rustls(&r, pinned),
        ureq::Error::Tls(m) => DavError::Tls(TlsFailure::Other(m.to_owned())),
        ureq::Error::HostNotFound => DavError::Network("the server name was not found".to_owned()),
        ureq::Error::ConnectionFailed => DavError::Network("the connection failed".to_owned()),
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => {
            DavError::Redirect("too many redirects".to_owned())
        }
        other => DavError::Protocol(other.to_string()),
    }
}

/// `ureq` cannot take a custom certificate verifier (its `TlsConfig` only offers root stores), so
/// pinning replaces its TLS connector in the chain: this connector wraps the TCP transport in
/// rustls with [`PinVerifier`], which accepts only the pinned certificate.
pub(crate) struct PinnedConnector {
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for PinnedConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinnedConnector")
    }
}

impl PinnedConnector {
    pub(crate) fn new(pin: [u8; 32]) -> Self {
        let prov = provider();
        let verifier = Arc::new(PinVerifier {
            pin,
            algs: prov.signature_verification_algorithms,
        });
        let config = ClientConfig::builder_with_provider(prov)
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        Self {
            config: Arc::new(config),
        }
    }
}

impl<In: Transport> Connector<In> for PinnedConnector {
    type Out = Either<In, PinnedTransport>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        let Some(transport) = chained else {
            return Err(ureq::Error::Tls("no transport to secure"));
        };
        if !details.needs_tls() || transport.is_tls() {
            return Ok(Some(Either::A(transport)));
        }
        let host = details
            .uri
            .authority()
            .map(|a| {
                a.host()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_owned()
            })
            .ok_or(ureq::Error::Tls("the address has no host"))?;
        let name = ServerName::try_from(host).map_err(|_| ureq::Error::Tls("invalid host name"))?;
        let mut conn =
            ClientConnection::new(Arc::clone(&self.config), name).map_err(ureq::Error::Rustls)?;
        let mut sock = TransportAdapter::new(transport.boxed(), details.current_time.clone());
        sock.set_timeout(details.timeout);
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(ureq::Error::Io)?;
        }
        let buffers = LazyBuffers::new(
            details.config.input_buffer_size(),
            details.config.output_buffer_size(),
        );
        Ok(Some(Either::B(PinnedTransport {
            buffers,
            stream: StreamOwned { conn, sock },
        })))
    }
}

/// A TLS stream over the chained transport.
pub(crate) struct PinnedTransport {
    buffers: LazyBuffers,
    stream: StreamOwned<ClientConnection, TransportAdapter<Box<dyn Transport>>>,
}

impl std::fmt::Debug for PinnedTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinnedTransport")
    }
}

impl Transport for PinnedTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let output = &self.buffers.output()[..amount];
        self.stream.write_all(output)?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let input = self.buffers.input_append_buf();
        let amount = self.stream.read(input)?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        self.stream.get_mut().get_mut().is_open()
    }

    fn is_tls(&self) -> bool {
        true
    }
}
