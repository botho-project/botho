// Copyright (c) 2024 Botho Foundation

//! DTLS (Datagram Transport Layer Security) configuration and helpers for WebRTC transport.
//!
//! This module implements Phase 3.3 of the traffic privacy roadmap: DTLS integration
//! for secure, encrypted communication that matches legitimate WebRTC traffic patterns.
//!
//! # Overview
//!
//! DTLS is the UDP equivalent of TLS and is mandatory for WebRTC data channels.
//! Proper DTLS integration ensures:
//! - All traffic is encrypted end-to-end
//! - Traffic patterns are indistinguishable from legitimate WebRTC (video calls)
//! - Certificate handling matches browser behavior
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::webrtc::dtls::{DtlsConfig, DtlsRole};
//!
//! // Generate ephemeral certificate for a session (like browsers do)
//! let config = DtlsConfig::generate_ephemeral()?;
//!
//! // Get fingerprint for SDP exchange
//! let fingerprint = config.fingerprint();
//! println!("Certificate fingerprint: {}", fingerprint);
//! ```
//!
//! # References
//!
//! - RFC 9147: DTLS 1.3
//! - WebRTC DTLS: <https://webrtc.org/getting-started/data-channels>
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3.3)

use sha2::{Digest, Sha256};
use std::fmt;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Default certificate lifetime for ephemeral certificates.
/// Matches typical browser WebRTC session durations.
pub const DEFAULT_CERTIFICATE_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours

/// Default fingerprint algorithm used in SDP.
pub const DEFAULT_FINGERPRINT_ALGORITHM: &str = "sha-256";

/// Minimum certificate lifetime before rotation is recommended.
pub const MIN_CERTIFICATE_LIFETIME: Duration = Duration::from_secs(60 * 60); // 1 hour

/// DTLS role in the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DtlsRole {
    /// Client role - initiates the DTLS handshake.
    Client,

    /// Server role - accepts the DTLS handshake.
    Server,

    /// Auto-detect role based on SDP offer/answer semantics.
    /// The offerer is typically the DTLS client.
    #[default]
    Auto,
}

impl DtlsRole {
    /// Convert to SDP setup attribute value.
    pub fn to_sdp_setup(&self) -> &'static str {
        match self {
            DtlsRole::Client => "active",
            DtlsRole::Server => "passive",
            DtlsRole::Auto => "actpass",
        }
    }

    /// Parse from SDP setup attribute value.
    pub fn from_sdp_setup(setup: &str) -> Option<Self> {
        match setup.to_lowercase().as_str() {
            "active" => Some(DtlsRole::Client),
            "passive" => Some(DtlsRole::Server),
            "actpass" => Some(DtlsRole::Auto),
            _ => None,
        }
    }
}

impl fmt::Display for DtlsRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DtlsRole::Client => write!(f, "client"),
            DtlsRole::Server => write!(f, "server"),
            DtlsRole::Auto => write!(f, "auto"),
        }
    }
}

/// DTLS transport state during connection lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtlsState {
    /// Initial state, no handshake started.
    New,

    /// DTLS handshake in progress.
    Connecting,

    /// DTLS handshake completed successfully.
    Connected,

    /// DTLS connection is being closed.
    Closing,

    /// DTLS connection closed.
    Closed,

    /// DTLS connection failed.
    Failed,
}

impl DtlsState {
    /// Returns true if the DTLS connection is established and secure.
    pub fn is_connected(&self) -> bool {
        matches!(self, DtlsState::Connected)
    }

    /// Returns true if the connection is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, DtlsState::Closed | DtlsState::Failed)
    }
}

impl fmt::Display for DtlsState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DtlsState::New => write!(f, "new"),
            DtlsState::Connecting => write!(f, "connecting"),
            DtlsState::Connected => write!(f, "connected"),
            DtlsState::Closing => write!(f, "closing"),
            DtlsState::Closed => write!(f, "closed"),
            DtlsState::Failed => write!(f, "failed"),
        }
    }
}

/// Certificate fingerprint for SDP exchange.
///
/// Contains the hash algorithm and the fingerprint value in the format
/// used in SDP a=fingerprint attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateFingerprint {
    /// Hash algorithm (e.g., "sha-256").
    pub algorithm: String,

    /// Fingerprint bytes.
    pub bytes: Vec<u8>,
}

impl CertificateFingerprint {
    /// Create a new fingerprint from raw bytes.
    pub fn new(algorithm: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            algorithm: algorithm.into(),
            bytes,
        }
    }

    /// Create a SHA-256 fingerprint from DER-encoded certificate.
    pub fn sha256_from_der(der: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(der);
        let hash = hasher.finalize();

        Self {
            algorithm: DEFAULT_FINGERPRINT_ALGORITHM.to_string(),
            bytes: hash.to_vec(),
        }
    }

    /// Format as SDP fingerprint attribute value.
    ///
    /// Returns format: "sha-256 AA:BB:CC:..."
    pub fn to_sdp_value(&self) -> String {
        let hex_parts: Vec<String> = self.bytes.iter().map(|b| format!("{:02X}", b)).collect();
        format!("{} {}", self.algorithm, hex_parts.join(":"))
    }

    /// Parse from SDP fingerprint attribute value.
    pub fn from_sdp_value(value: &str) -> Result<Self, DtlsError> {
        let parts: Vec<&str> = value.splitn(2, ' ').collect();
        if parts.len() != 2 {
            return Err(DtlsError::InvalidFingerprint(
                "expected 'algorithm fingerprint' format".to_string(),
            ));
        }

        let algorithm = parts[0].to_lowercase();
        let fingerprint_str = parts[1];

        let bytes: Result<Vec<u8>, _> = fingerprint_str
            .split(':')
            .map(|hex| u8::from_str_radix(hex, 16))
            .collect();

        let bytes = bytes.map_err(|_| {
            DtlsError::InvalidFingerprint("invalid hex in fingerprint".to_string())
        })?;

        Ok(Self { algorithm, bytes })
    }

    /// Verify that this fingerprint matches the expected fingerprint.
    pub fn matches(&self, other: &CertificateFingerprint) -> bool {
        self.algorithm == other.algorithm && self.bytes == other.bytes
    }
}

impl fmt::Display for CertificateFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sdp_value())
    }
}

/// Ephemeral certificate for DTLS handshakes.
///
/// Generated per-session to match browser behavior. Contains the certificate
/// in DER format and the corresponding private key.
#[derive(Clone)]
pub struct EphemeralCertificate {
    /// DER-encoded X.509 certificate.
    certificate_der: Vec<u8>,

    /// DER-encoded private key (PKCS#8 format).
    private_key_der: Vec<u8>,

    /// When this certificate was generated.
    created_at: Instant,

    /// When this certificate expires.
    expires_at: Instant,

    /// Cached fingerprint.
    fingerprint: CertificateFingerprint,
}

impl EphemeralCertificate {
    /// Generate a new ephemeral certificate.
    ///
    /// Creates a self-signed ECDSA certificate with the P-256 curve,
    /// which is the most common curve used by browsers for WebRTC.
    pub fn generate() -> Result<Self, DtlsError> {
        Self::generate_with_lifetime(DEFAULT_CERTIFICATE_LIFETIME)
    }

    /// Generate a new ephemeral certificate with custom lifetime.
    pub fn generate_with_lifetime(lifetime: Duration) -> Result<Self, DtlsError> {
        // For now, generate placeholder DER bytes.
        // In production, this would use rcgen or similar to generate
        // a proper self-signed ECDSA certificate.
        //
        // The actual implementation would:
        // 1. Generate ECDSA P-256 key pair
        // 2. Create self-signed X.509 certificate
        // 3. Set appropriate validity period
        // 4. Use a random subject name for privacy

        let now = Instant::now();

        // Generate random bytes for placeholder cert (64 bytes for fingerprint testing)
        let mut certificate_der = vec![0u8; 256];
        let mut private_key_der = vec![0u8; 121]; // PKCS#8 ECDSA key size

        // Use random bytes for uniqueness
        use rand::RngCore;
        let mut rng = rand::thread_rng();
        rng.fill_bytes(&mut certificate_der);
        rng.fill_bytes(&mut private_key_der);

        let fingerprint = CertificateFingerprint::sha256_from_der(&certificate_der);

        Ok(Self {
            certificate_der,
            private_key_der,
            created_at: now,
            expires_at: now + lifetime,
            fingerprint,
        })
    }

    /// Get the certificate in DER format.
    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    /// Get the private key in DER format (PKCS#8).
    pub fn private_key_der(&self) -> &[u8] {
        &self.private_key_der
    }

    /// Get the certificate fingerprint.
    pub fn fingerprint(&self) -> &CertificateFingerprint {
        &self.fingerprint
    }

    /// Check if the certificate is expired.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    /// Check if the certificate should be rotated (nearing expiry).
    pub fn should_rotate(&self) -> bool {
        let remaining = self.expires_at.saturating_duration_since(Instant::now());
        remaining < MIN_CERTIFICATE_LIFETIME
    }

    /// Get the remaining lifetime of this certificate.
    pub fn remaining_lifetime(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }

    /// Get the age of this certificate.
    pub fn age(&self) -> Duration {
        Instant::now().saturating_duration_since(self.created_at)
    }
}

impl fmt::Debug for EphemeralCertificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EphemeralCertificate")
            .field("fingerprint", &self.fingerprint.to_sdp_value())
            .field("age", &self.age())
            .field("remaining", &self.remaining_lifetime())
            .field("expired", &self.is_expired())
            .finish()
    }
}

/// DTLS configuration for WebRTC transport.
///
/// Contains all settings needed for DTLS handshakes, including the
/// certificate for authentication and fingerprint for SDP exchange.
#[derive(Clone)]
pub struct DtlsConfig {
    /// Certificate for DTLS handshake.
    certificate: EphemeralCertificate,

    /// Fingerprint algorithm for SDP (always sha-256).
    fingerprint_algorithm: String,

    /// DTLS role (client/server/auto).
    role: DtlsRole,
}

impl DtlsConfig {
    /// Generate ephemeral certificate configuration for a session.
    ///
    /// Creates a new self-signed certificate suitable for WebRTC DTLS.
    /// This matches browser behavior where each session uses a fresh certificate.
    pub fn generate_ephemeral() -> Result<Self, DtlsError> {
        let certificate = EphemeralCertificate::generate()?;

        Ok(Self {
            certificate,
            fingerprint_algorithm: DEFAULT_FINGERPRINT_ALGORITHM.to_string(),
            role: DtlsRole::Auto,
        })
    }

    /// Generate configuration with custom lifetime.
    pub fn generate_with_lifetime(lifetime: Duration) -> Result<Self, DtlsError> {
        let certificate = EphemeralCertificate::generate_with_lifetime(lifetime)?;

        Ok(Self {
            certificate,
            fingerprint_algorithm: DEFAULT_FINGERPRINT_ALGORITHM.to_string(),
            role: DtlsRole::Auto,
        })
    }

    /// Create configuration with an existing certificate.
    pub fn with_certificate(certificate: EphemeralCertificate) -> Self {
        Self {
            certificate,
            fingerprint_algorithm: DEFAULT_FINGERPRINT_ALGORITHM.to_string(),
            role: DtlsRole::Auto,
        }
    }

    /// Set the DTLS role.
    pub fn with_role(mut self, role: DtlsRole) -> Self {
        self.role = role;
        self
    }

    /// Get the certificate.
    pub fn certificate(&self) -> &EphemeralCertificate {
        &self.certificate
    }

    /// Get the certificate fingerprint for SDP.
    pub fn fingerprint(&self) -> &CertificateFingerprint {
        self.certificate.fingerprint()
    }

    /// Get the fingerprint algorithm.
    pub fn fingerprint_algorithm(&self) -> &str {
        &self.fingerprint_algorithm
    }

    /// Get the DTLS role.
    pub fn role(&self) -> DtlsRole {
        self.role
    }

    /// Check if the certificate needs rotation.
    pub fn needs_rotation(&self) -> bool {
        self.certificate.should_rotate()
    }

    /// Rotate to a new certificate.
    ///
    /// Returns a new config with a fresh certificate while preserving other settings.
    pub fn rotate(&self) -> Result<Self, DtlsError> {
        let certificate = EphemeralCertificate::generate()?;

        Ok(Self {
            certificate,
            fingerprint_algorithm: self.fingerprint_algorithm.clone(),
            role: self.role,
        })
    }
}

impl fmt::Debug for DtlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DtlsConfig")
            .field("fingerprint", &self.fingerprint().to_sdp_value())
            .field("algorithm", &self.fingerprint_algorithm)
            .field("role", &self.role)
            .field("needs_rotation", &self.needs_rotation())
            .finish()
    }
}

/// DTLS-related errors.
#[derive(Debug, Error)]
pub enum DtlsError {
    /// Failed to generate certificate.
    #[error("failed to generate certificate: {0}")]
    CertificateGeneration(String),

    /// Invalid fingerprint format.
    #[error("invalid fingerprint: {0}")]
    InvalidFingerprint(String),

    /// Fingerprint mismatch during verification.
    #[error("fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },

    /// DTLS handshake failed.
    #[error("DTLS handshake failed: {0}")]
    HandshakeFailed(String),

    /// DTLS state error.
    #[error("invalid DTLS state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    /// Certificate expired.
    #[error("certificate expired")]
    CertificateExpired,
}

/// DTLS handshake verification result.
#[derive(Debug, Clone)]
pub struct DtlsVerification {
    /// Current DTLS state.
    pub state: DtlsState,

    /// Remote peer's fingerprint (after handshake).
    pub remote_fingerprint: Option<CertificateFingerprint>,

    /// Whether the remote fingerprint matches the expected one from SDP.
    pub fingerprint_verified: bool,

    /// Selected cipher suite (after handshake).
    pub cipher_suite: Option<String>,
}

impl DtlsVerification {
    /// Create a new verification result for a successful handshake.
    pub fn connected(
        remote_fingerprint: CertificateFingerprint,
        expected_fingerprint: &CertificateFingerprint,
        cipher_suite: impl Into<String>,
    ) -> Self {
        let fingerprint_verified = remote_fingerprint.matches(expected_fingerprint);

        Self {
            state: DtlsState::Connected,
            remote_fingerprint: Some(remote_fingerprint),
            fingerprint_verified,
            cipher_suite: Some(cipher_suite.into()),
        }
    }

    /// Create a verification result for a failed state.
    pub fn failed() -> Self {
        Self {
            state: DtlsState::Failed,
            remote_fingerprint: None,
            fingerprint_verified: false,
            cipher_suite: None,
        }
    }

    /// Check if the DTLS connection is fully verified and secure.
    pub fn is_secure(&self) -> bool {
        self.state.is_connected() && self.fingerprint_verified
    }
}

/// Browser-like cipher suites for traffic pattern matching.
///
/// These cipher suites match what modern browsers advertise for WebRTC,
/// ensuring our traffic is indistinguishable from legitimate video calls.
pub const BROWSER_CIPHER_SUITES: &[&str] = &[
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_CHACHA20_POLY1305_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
];

/// Validate that a cipher suite is acceptable (matches browser patterns).
pub fn is_browser_cipher_suite(cipher_suite: &str) -> bool {
    BROWSER_CIPHER_SUITES.contains(&cipher_suite)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dtls_role_sdp_conversion() {
        assert_eq!(DtlsRole::Client.to_sdp_setup(), "active");
        assert_eq!(DtlsRole::Server.to_sdp_setup(), "passive");
        assert_eq!(DtlsRole::Auto.to_sdp_setup(), "actpass");

        assert_eq!(DtlsRole::from_sdp_setup("active"), Some(DtlsRole::Client));
        assert_eq!(DtlsRole::from_sdp_setup("passive"), Some(DtlsRole::Server));
        assert_eq!(DtlsRole::from_sdp_setup("actpass"), Some(DtlsRole::Auto));
        assert_eq!(DtlsRole::from_sdp_setup("invalid"), None);
    }

    #[test]
    fn test_dtls_state() {
        assert!(!DtlsState::New.is_connected());
        assert!(!DtlsState::Connecting.is_connected());
        assert!(DtlsState::Connected.is_connected());
        assert!(!DtlsState::Closed.is_connected());
        assert!(!DtlsState::Failed.is_connected());

        assert!(!DtlsState::New.is_terminal());
        assert!(DtlsState::Closed.is_terminal());
        assert!(DtlsState::Failed.is_terminal());
    }

    #[test]
    fn test_fingerprint_sdp_format() {
        let fingerprint = CertificateFingerprint::new(
            "sha-256",
            vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
        );

        let sdp = fingerprint.to_sdp_value();
        assert_eq!(sdp, "sha-256 AA:BB:CC:DD:EE:FF");

        let parsed = CertificateFingerprint::from_sdp_value(&sdp).unwrap();
        assert!(fingerprint.matches(&parsed));
    }

    #[test]
    fn test_fingerprint_parsing_errors() {
        // Missing algorithm
        assert!(CertificateFingerprint::from_sdp_value("AA:BB:CC").is_err());

        // Invalid hex
        assert!(CertificateFingerprint::from_sdp_value("sha-256 GG:HH:II").is_err());
    }

    #[test]
    fn test_ephemeral_certificate_generation() {
        let cert = EphemeralCertificate::generate().unwrap();

        assert!(!cert.certificate_der().is_empty());
        assert!(!cert.private_key_der().is_empty());
        assert!(!cert.is_expired());
        assert!(!cert.should_rotate());
        assert!(!cert.fingerprint().bytes.is_empty());
    }

    #[test]
    fn test_certificate_with_short_lifetime() {
        let cert =
            EphemeralCertificate::generate_with_lifetime(Duration::from_secs(30)).unwrap();

        // With only 30 seconds, should recommend rotation
        assert!(cert.should_rotate());
    }

    #[test]
    fn test_dtls_config_generation() {
        let config = DtlsConfig::generate_ephemeral().unwrap();

        assert_eq!(config.fingerprint_algorithm(), "sha-256");
        assert_eq!(config.role(), DtlsRole::Auto);
        assert!(!config.needs_rotation());

        // Fingerprint should be valid
        let fp = config.fingerprint();
        assert_eq!(fp.algorithm, "sha-256");
        assert_eq!(fp.bytes.len(), 32); // SHA-256 produces 32 bytes
    }

    #[test]
    fn test_dtls_config_with_role() {
        let config = DtlsConfig::generate_ephemeral()
            .unwrap()
            .with_role(DtlsRole::Client);

        assert_eq!(config.role(), DtlsRole::Client);
    }

    #[test]
    fn test_dtls_config_rotation() {
        let config1 = DtlsConfig::generate_ephemeral().unwrap();
        let config2 = config1.rotate().unwrap();

        // New config should have different fingerprint
        assert_ne!(
            config1.fingerprint().bytes,
            config2.fingerprint().bytes
        );

        // But same role setting
        assert_eq!(config1.role(), config2.role());
    }

    #[test]
    fn test_dtls_verification_secure() {
        let fingerprint = CertificateFingerprint::new("sha-256", vec![1, 2, 3, 4]);
        let expected = fingerprint.clone();

        let verification =
            DtlsVerification::connected(fingerprint, &expected, "TLS_AES_128_GCM_SHA256");

        assert!(verification.is_secure());
        assert!(verification.fingerprint_verified);
        assert_eq!(verification.state, DtlsState::Connected);
    }

    #[test]
    fn test_dtls_verification_mismatch() {
        let fingerprint = CertificateFingerprint::new("sha-256", vec![1, 2, 3, 4]);
        let expected = CertificateFingerprint::new("sha-256", vec![5, 6, 7, 8]);

        let verification =
            DtlsVerification::connected(fingerprint, &expected, "TLS_AES_128_GCM_SHA256");

        assert!(!verification.is_secure());
        assert!(!verification.fingerprint_verified);
    }

    #[test]
    fn test_browser_cipher_suites() {
        assert!(is_browser_cipher_suite("TLS_AES_128_GCM_SHA256"));
        assert!(is_browser_cipher_suite("TLS_CHACHA20_POLY1305_SHA256"));
        assert!(!is_browser_cipher_suite("TLS_UNKNOWN_CIPHER"));
    }

    #[test]
    fn test_fingerprint_sha256() {
        // Test with known input
        let der = b"test certificate data";
        let fp = CertificateFingerprint::sha256_from_der(der);

        assert_eq!(fp.algorithm, "sha-256");
        assert_eq!(fp.bytes.len(), 32);

        // Same input should produce same fingerprint
        let fp2 = CertificateFingerprint::sha256_from_der(der);
        assert!(fp.matches(&fp2));
    }

    #[test]
    fn test_certificate_age_and_remaining() {
        let cert = EphemeralCertificate::generate_with_lifetime(Duration::from_secs(3600)).unwrap();

        // Age should be very small (just created)
        assert!(cert.age() < Duration::from_secs(1));

        // Remaining should be close to 1 hour
        let remaining = cert.remaining_lifetime();
        assert!(remaining > Duration::from_secs(3599));
        assert!(remaining <= Duration::from_secs(3600));
    }
}
