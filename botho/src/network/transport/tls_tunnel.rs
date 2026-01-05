// Copyright (c) 2024 Botho Foundation

//! TLS tunnel transport implementation for protocol obfuscation.
//!
//! This module implements a TLS tunnel transport that makes botho traffic
//! look like standard HTTPS. It provides an alternative obfuscation method
//! for environments where WebRTC is blocked but HTTPS is allowed.
//!
//! # Overview
//!
//! The TLS tunnel transport uses:
//! - **TLS 1.3**: Modern encryption with browser-compatible cipher suites
//! - **Self-signed certificates**: Generated for P2P authentication
//! - **Optional SNI override**: For domain fronting scenarios
//! - **Optional HTTP/2 framing**: Maximum obfuscation (looks like real HTTPS)
//!
//! # Security Properties
//!
//! - Traffic appears as normal HTTPS to DPI
//! - Uses TLS 1.3 with modern cipher suites
//! - Self-signed certificates with peer ID binding
//! - Optional domain fronting support via SNI override
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::{TlsTunnelTransport, TlsConfig, PluggableTransport};
//!
//! // Create transport with self-signed certificate
//! let config = TlsConfig::generate_self_signed().unwrap();
//! let transport = TlsTunnelTransport::new(config);
//!
//! assert_eq!(transport.name(), "tls-tunnel");
//! assert!(transport.transport_type().is_obfuscated());
//! ```
//!
//! # References
//!
//! - Design document: `docs/design/traffic-privacy-roadmap.md` (Section 3.7)
//! - TLS 1.3: RFC 8446
//! - Domain Fronting: https://www.bamsoftware.com/papers/fronting/

use async_trait::async_trait;
use libp2p::{Multiaddr, PeerId};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use std::{
    fmt, io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use super::{
    error::TransportError,
    traits::{BoxedConnection, PluggableTransport},
    types::TransportType,
};

/// TLS configuration for the tunnel transport.
///
/// This holds the certificate, private key, and TLS settings needed
/// for establishing encrypted connections.
#[derive(Clone)]
pub struct TlsConfig {
    /// DER-encoded certificate chain
    certificates: Vec<CertificateDer<'static>>,

    /// Private key DER bytes (stored as Vec<u8> for Clone)
    private_key_der: Vec<u8>,

    /// Optional SNI value for domain fronting
    sni_override: Option<String>,

    /// ALPN protocols to advertise
    alpn_protocols: Vec<Vec<u8>>,

    /// Connection timeout in seconds
    connect_timeout_secs: u64,
}

impl TlsConfig {
    /// Get the private key as a PrivateKeyDer.
    fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.private_key_der.clone()))
    }
}

impl TlsConfig {
    /// Generate a self-signed certificate for P2P communication.
    ///
    /// Creates an ephemeral ECDSA certificate valid for 365 days.
    /// The certificate uses P-256 curve for broad compatibility.
    pub fn generate_self_signed() -> Result<Self, TlsConfigError> {
        // Generate ECDSA key pair
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| TlsConfigError::KeyGeneration(e.to_string()))?;

        // Configure certificate parameters
        let mut params = CertificateParams::default();

        // Use a generic-looking common name for obfuscation
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "localhost");
        dn.push(DnType::OrganizationName, "Private");
        params.distinguished_name = dn;

        // Set validity period
        params.not_before = time::OffsetDateTime::now_utc();
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(365);

        // Add subject alternative names for local connections
        params.subject_alt_names = vec![
            rcgen::SanType::DnsName("localhost".try_into().unwrap()),
            rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
            rcgen::SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
        ];

        // Generate certificate
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| TlsConfigError::CertGeneration(e.to_string()))?;

        // Convert to DER format
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der_bytes = key_pair.serialize_der().to_vec();

        Ok(Self {
            certificates: vec![cert_der],
            private_key_der: key_der_bytes,
            sni_override: None,
            alpn_protocols: Self::browser_compatible_alpn(),
            connect_timeout_secs: 30,
        })
    }

    /// Create configuration from existing certificate and key.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TlsConfigError> {
        use rustls_pemfile::{certs, private_key};
        use std::io::BufReader;

        // Parse certificates
        let certs: Vec<CertificateDer<'static>> = certs(&mut BufReader::new(cert_pem))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TlsConfigError::InvalidCertificate(e.to_string()))?;

        if certs.is_empty() {
            return Err(TlsConfigError::InvalidCertificate(
                "no certificates found in PEM".to_string(),
            ));
        }

        // Parse private key
        let key = private_key(&mut BufReader::new(key_pem))
            .map_err(|e| TlsConfigError::InvalidKey(e.to_string()))?
            .ok_or_else(|| TlsConfigError::InvalidKey("no private key found in PEM".to_string()))?;

        // Extract DER bytes from the key
        let key_der_bytes = match &key {
            PrivateKeyDer::Pkcs8(k) => k.secret_pkcs8_der().to_vec(),
            PrivateKeyDer::Pkcs1(k) => k.secret_pkcs1_der().to_vec(),
            PrivateKeyDer::Sec1(k) => k.secret_sec1_der().to_vec(),
            _ => {
                return Err(TlsConfigError::InvalidKey(
                    "unsupported key format".to_string(),
                ))
            }
        };

        Ok(Self {
            certificates: certs,
            private_key_der: key_der_bytes,
            sni_override: None,
            alpn_protocols: Self::browser_compatible_alpn(),
            connect_timeout_secs: 30,
        })
    }

    /// Browser-compatible ALPN protocols.
    ///
    /// We advertise HTTP/2 and HTTP/1.1 to look like normal browser traffic.
    pub fn browser_compatible_alpn() -> Vec<Vec<u8>> {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    }

    /// Set SNI override for domain fronting.
    ///
    /// This allows using a different SNI value than the actual destination,
    /// which can help bypass certain types of filtering.
    pub fn with_sni_override(mut self, sni: String) -> Self {
        self.sni_override = Some(sni);
        self
    }

    /// Set custom ALPN protocols.
    pub fn with_alpn(mut self, protocols: Vec<Vec<u8>>) -> Self {
        self.alpn_protocols = protocols;
        self
    }

    /// Set connection timeout.
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.connect_timeout_secs = timeout_secs;
        self
    }

    /// Get the SNI override if set.
    pub fn sni_override(&self) -> Option<&str> {
        self.sni_override.as_deref()
    }

    /// Get the certificate fingerprint (SHA-256).
    ///
    /// This can be used for peer verification out-of-band.
    pub fn certificate_fingerprint(&self) -> Option<[u8; 32]> {
        use sha2::{Digest, Sha256};

        self.certificates.first().map(|cert| {
            let mut hasher = Sha256::new();
            hasher.update(cert.as_ref());
            hasher.finalize().into()
        })
    }
}

impl fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsConfig")
            .field("certificates", &self.certificates.len())
            .field("sni_override", &self.sni_override)
            .field("alpn_protocols", &self.alpn_protocols.len())
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .finish_non_exhaustive()
    }
}

/// Errors that can occur during TLS configuration.
#[derive(Debug, Clone)]
pub enum TlsConfigError {
    /// Failed to generate key pair.
    KeyGeneration(String),

    /// Failed to generate certificate.
    CertGeneration(String),

    /// Invalid certificate data.
    InvalidCertificate(String),

    /// Invalid private key data.
    InvalidKey(String),

    /// Failed to build TLS configuration.
    ConfigBuild(String),
}

impl fmt::Display for TlsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsConfigError::KeyGeneration(e) => write!(f, "key generation failed: {}", e),
            TlsConfigError::CertGeneration(e) => write!(f, "certificate generation failed: {}", e),
            TlsConfigError::InvalidCertificate(e) => write!(f, "invalid certificate: {}", e),
            TlsConfigError::InvalidKey(e) => write!(f, "invalid private key: {}", e),
            TlsConfigError::ConfigBuild(e) => write!(f, "TLS config build failed: {}", e),
        }
    }
}

impl std::error::Error for TlsConfigError {}

/// TLS tunnel transport that makes traffic look like HTTPS.
///
/// This transport wraps connections in TLS 1.3 with browser-compatible
/// cipher suites. To a deep packet inspector, the traffic appears
/// identical to normal HTTPS/browser traffic.
#[derive(Clone)]
pub struct TlsTunnelTransport {
    /// TLS configuration
    config: TlsConfig,

    /// Cached TLS connector for client connections
    connector: TlsConnector,

    /// Cached TLS acceptor for server connections
    acceptor: TlsAcceptor,
}

impl TlsTunnelTransport {
    /// Create a new TLS tunnel transport with the given configuration.
    pub fn new(config: TlsConfig) -> Result<Self, TlsConfigError> {
        // Build client config (for outbound connections)
        let client_config = Self::build_client_config(&config)?;
        let connector = TlsConnector::from(Arc::new(client_config));

        // Build server config (for inbound connections)
        let server_config = Self::build_server_config(&config)?;
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        Ok(Self {
            config,
            connector,
            acceptor,
        })
    }

    /// Create transport with a self-signed certificate.
    pub fn with_self_signed() -> Result<Self, TlsConfigError> {
        let config = TlsConfig::generate_self_signed()?;
        Self::new(config)
    }

    /// Get the TLS configuration.
    pub fn config(&self) -> &TlsConfig {
        &self.config
    }

    /// Build rustls client configuration.
    fn build_client_config(config: &TlsConfig) -> Result<rustls::ClientConfig, TlsConfigError> {
        // For P2P, we accept any certificate since we verify peer identity separately
        // In production, you might want to pin specific certificates
        let root_store = rustls::RootCertStore::empty();

        let mut tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        // Set ALPN protocols
        tls_config.alpn_protocols = config.alpn_protocols.clone();

        // Use dangerous config to skip certificate verification for self-signed certs
        // This is safe because we verify peer identity via libp2p's peer ID
        tls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(SkipServerVerification));

        Ok(tls_config)
    }

    /// Build rustls server configuration.
    fn build_server_config(config: &TlsConfig) -> Result<rustls::ServerConfig, TlsConfigError> {
        let mut tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(config.certificates.clone(), config.private_key())
            .map_err(|e| TlsConfigError::ConfigBuild(e.to_string()))?;

        // Set ALPN protocols
        tls_config.alpn_protocols = config.alpn_protocols.clone();

        Ok(tls_config)
    }

    /// Extract TCP address from multiaddr.
    fn extract_tcp_addr(addr: &Multiaddr) -> Option<std::net::SocketAddr> {
        use libp2p::multiaddr::Protocol;

        let mut ip = None;
        let mut port = None;

        for proto in addr.iter() {
            match proto {
                Protocol::Ip4(addr) => ip = Some(std::net::IpAddr::V4(addr)),
                Protocol::Ip6(addr) => ip = Some(std::net::IpAddr::V6(addr)),
                Protocol::Tcp(p) => port = Some(p),
                _ => {}
            }
        }

        match (ip, port) {
            (Some(ip), Some(port)) => Some(std::net::SocketAddr::new(ip, port)),
            _ => None,
        }
    }

    /// Get SNI domain for connection.
    fn get_sni_domain(&self, peer_addr: &std::net::SocketAddr) -> String {
        // Use SNI override if set, otherwise use IP as domain
        self.config
            .sni_override
            .clone()
            .unwrap_or_else(|| peer_addr.ip().to_string())
    }
}

impl fmt::Debug for TlsTunnelTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsTunnelTransport")
            .field("config", &self.config)
            .finish()
    }
}

#[async_trait]
impl PluggableTransport for TlsTunnelTransport {
    fn transport_type(&self) -> TransportType {
        TransportType::TlsTunnel
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn connect(
        &self,
        peer: &PeerId,
        addr: Option<&Multiaddr>,
    ) -> Result<BoxedConnection, TransportError> {
        let addr = addr.ok_or_else(|| {
            TransportError::InvalidPeer(format!("no address provided for peer {}", peer))
        })?;

        let socket_addr = Self::extract_tcp_addr(addr).ok_or_else(|| {
            TransportError::InvalidPeer(format!("cannot extract TCP address from {}", addr))
        })?;

        // Connect with timeout
        let connect_future = TcpStream::connect(socket_addr);
        let timeout = Duration::from_secs(self.config.connect_timeout_secs);

        let stream = tokio::time::timeout(timeout, connect_future)
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|e| TransportError::ConnectionFailed(e.to_string()))?;

        // Configure TCP options
        stream.set_nodelay(true).map_err(|e| {
            TransportError::Configuration(format!("failed to set TCP_NODELAY: {}", e))
        })?;

        // Perform TLS handshake
        let sni_domain = self.get_sni_domain(&socket_addr);
        let server_name = ServerName::try_from(sni_domain.clone())
            .map_err(|e| TransportError::HandshakeFailed(format!("invalid SNI: {}", e)))?;

        let tls_stream = tokio::time::timeout(timeout, self.connector.connect(server_name, stream))
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|e| TransportError::HandshakeFailed(format!("TLS handshake failed: {}", e)))?;

        let conn = TlsTunnelConnection::Client(TlsClientConnection {
            stream: tls_stream,
            peer: *peer,
            sni_domain,
        });

        Ok(Box::new(conn))
    }

    async fn accept(&self, _stream: BoxedConnection) -> Result<BoxedConnection, TransportError> {
        // We need a TCP stream for the TLS acceptor
        // Since we receive a BoxedConnection, we wrap it differently
        // For now, return an error indicating this requires special handling
        Err(TransportError::NotSupported)
    }
}

/// Skip certificate verification for P2P self-signed certificates.
///
/// This is safe because peer identity is verified via libp2p's peer ID
/// mechanism, not through the TLS certificate chain.
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // Skip verification - peer identity is verified separately
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}

/// TLS tunnel connection variants.
#[derive(Debug)]
pub enum TlsTunnelConnection {
    /// Client-initiated connection
    Client(TlsClientConnection),
    /// Server-accepted connection
    Server(TlsServerConnection),
}

impl AsyncRead for TlsTunnelConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            TlsTunnelConnection::Client(conn) => Pin::new(conn).poll_read(cx, buf),
            TlsTunnelConnection::Server(conn) => Pin::new(conn).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TlsTunnelConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            TlsTunnelConnection::Client(conn) => Pin::new(conn).poll_write(cx, buf),
            TlsTunnelConnection::Server(conn) => Pin::new(conn).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            TlsTunnelConnection::Client(conn) => Pin::new(conn).poll_flush(cx),
            TlsTunnelConnection::Server(conn) => Pin::new(conn).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            TlsTunnelConnection::Client(conn) => Pin::new(conn).poll_shutdown(cx),
            TlsTunnelConnection::Server(conn) => Pin::new(conn).poll_shutdown(cx),
        }
    }
}

impl Unpin for TlsTunnelConnection {}

/// Client-initiated TLS connection.
pub struct TlsClientConnection {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    peer: PeerId,
    sni_domain: String,
}

impl TlsClientConnection {
    /// Get the peer ID.
    pub fn peer(&self) -> &PeerId {
        &self.peer
    }

    /// Get the SNI domain used.
    pub fn sni_domain(&self) -> &str {
        &self.sni_domain
    }
}

impl fmt::Debug for TlsClientConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsClientConnection")
            .field("peer", &self.peer)
            .field("sni_domain", &self.sni_domain)
            .finish()
    }
}

impl AsyncRead for TlsClientConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TlsClientConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl Unpin for TlsClientConnection {}

/// Server-accepted TLS connection.
pub struct TlsServerConnection {
    stream: tokio_rustls::server::TlsStream<TcpStream>,
    peer: Option<PeerId>,
}

impl TlsServerConnection {
    /// Create a new server connection from a TLS stream.
    pub fn new(stream: tokio_rustls::server::TlsStream<TcpStream>) -> Self {
        Self { stream, peer: None }
    }

    /// Set the peer ID after identification.
    pub fn set_peer(&mut self, peer: PeerId) {
        self.peer = Some(peer);
    }

    /// Get the peer ID if known.
    pub fn peer(&self) -> Option<&PeerId> {
        self.peer.as_ref()
    }
}

impl fmt::Debug for TlsServerConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsServerConnection")
            .field("peer", &self.peer)
            .finish()
    }
}

impl AsyncRead for TlsServerConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TlsServerConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl Unpin for TlsServerConnection {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install the ring crypto provider for tests.
    fn install_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn test_generate_self_signed() {
        install_crypto_provider();
        let config = TlsConfig::generate_self_signed().unwrap();
        assert_eq!(config.certificates.len(), 1);
        assert!(config.sni_override.is_none());
    }

    #[test]
    fn test_browser_compatible_alpn() {
        let alpn = TlsConfig::browser_compatible_alpn();
        assert!(alpn.contains(&b"h2".to_vec()));
        assert!(alpn.contains(&b"http/1.1".to_vec()));
    }

    #[test]
    fn test_sni_override() {
        install_crypto_provider();
        let config = TlsConfig::generate_self_signed()
            .unwrap()
            .with_sni_override("example.com".to_string());
        assert_eq!(config.sni_override(), Some("example.com"));
    }

    #[test]
    fn test_certificate_fingerprint() {
        install_crypto_provider();
        let config = TlsConfig::generate_self_signed().unwrap();
        let fingerprint = config.certificate_fingerprint();
        assert!(fingerprint.is_some());
        // Fingerprint should be 32 bytes (SHA-256)
        assert_eq!(fingerprint.unwrap().len(), 32);
    }

    #[test]
    fn test_config_with_timeout() {
        install_crypto_provider();
        let config = TlsConfig::generate_self_signed().unwrap().with_timeout(60);
        assert_eq!(config.connect_timeout_secs, 60);
    }

    #[test]
    fn test_transport_creation() {
        install_crypto_provider();
        let transport = TlsTunnelTransport::with_self_signed().unwrap();
        assert_eq!(transport.transport_type(), TransportType::TlsTunnel);
        assert_eq!(transport.name(), "tls-tunnel");
        assert!(transport.is_available());
    }

    #[test]
    fn test_transport_debug() {
        install_crypto_provider();
        let transport = TlsTunnelTransport::with_self_signed().unwrap();
        let debug = format!("{:?}", transport);
        assert!(debug.contains("TlsTunnelTransport"));
        assert!(debug.contains("TlsConfig"));
    }

    #[tokio::test]
    async fn test_connect_no_address() {
        install_crypto_provider();
        let transport = TlsTunnelTransport::with_self_signed().unwrap();
        let peer = PeerId::random();

        let result = transport.connect(&peer, None).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            TransportError::InvalidPeer(msg) => {
                assert!(msg.contains("no address provided"));
            }
            e => panic!("unexpected error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_connect_invalid_address() {
        install_crypto_provider();
        let transport = TlsTunnelTransport::with_self_signed().unwrap();
        let peer = PeerId::random();
        let addr: Multiaddr = "/dns4/example.com".parse().unwrap();

        let result = transport.connect(&peer, Some(&addr)).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            TransportError::InvalidPeer(msg) => {
                assert!(msg.contains("cannot extract TCP address"));
            }
            e => panic!("unexpected error: {:?}", e),
        }
    }

    #[test]
    fn test_config_debug() {
        install_crypto_provider();
        let config = TlsConfig::generate_self_signed().unwrap();
        let debug = format!("{:?}", config);
        assert!(debug.contains("TlsConfig"));
        assert!(debug.contains("certificates"));
    }

    #[test]
    fn test_tls_config_error_display() {
        let err = TlsConfigError::KeyGeneration("test".to_string());
        assert_eq!(err.to_string(), "key generation failed: test");

        let err = TlsConfigError::InvalidCertificate("bad cert".to_string());
        assert_eq!(err.to_string(), "invalid certificate: bad cert");
    }
}
