// Copyright (c) 2024 Botho Foundation

//! ICE (Interactive Connectivity Establishment) for NAT traversal.
//!
//! ICE coordinates multiple connection methods to establish peer-to-peer
//! connections through NATs and firewalls. It uses STUN to discover
//! public addresses and TURN as a fallback relay.
//!
//! # Candidate Types
//!
//! 1. **Host candidates**: Direct LAN addresses
//! 2. **Server reflexive (srflx)**: Public IP discovered via STUN
//! 3. **Peer reflexive (prflx)**: Address learned during connectivity checks
//! 4. **Relay candidates**: Via TURN server (fallback)
//!
//! # Connection Flow
//!
//! ```text
//! Alice                                                 Bob
//!   │                                                    │
//!   │─────────────[1. Gather Candidates]─────────────────│
//!   │                                                    │
//!   │←─────────────────[2. Exchange via Signaling]──────→│
//!   │                                                    │
//!   │─────────────[3. Connectivity Checks]───────────────│
//!   │                                                    │
//!   │═══════════════[4. Selected Pair]═══════════════════│
//! ```

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use tracing::{debug, info, warn};
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::ice_transport::ice_gathering_state::RTCIceGatheringState;
use webrtc::peer_connection::RTCPeerConnection;

/// Errors that can occur during ICE operations.
#[derive(Debug, Error)]
pub enum IceError {
    /// ICE gathering timed out
    #[error("ICE gathering timed out after {0:?}")]
    GatheringTimeout(Duration),

    /// ICE connection timed out
    #[error("ICE connection timed out after {0:?}")]
    ConnectionTimeout(Duration),

    /// No suitable candidates found
    #[error("no suitable ICE candidates found")]
    NoCandidates,

    /// All connectivity checks failed
    #[error("all ICE connectivity checks failed")]
    ConnectivityChecksFailed,

    /// Invalid candidate format
    #[error("invalid ICE candidate: {0}")]
    InvalidCandidate(String),

    /// Internal WebRTC error
    #[error("WebRTC error: {0}")]
    WebRtc(String),
}

/// ICE candidate type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IceCandidateType {
    /// Host candidate - local interface address
    Host,
    /// Server reflexive - public address via STUN
    ServerReflexive,
    /// Peer reflexive - discovered during checks
    PeerReflexive,
    /// Relay - via TURN server
    Relay,
}

impl std::fmt::Display for IceCandidateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IceCandidateType::Host => write!(f, "host"),
            IceCandidateType::ServerReflexive => write!(f, "srflx"),
            IceCandidateType::PeerReflexive => write!(f, "prflx"),
            IceCandidateType::Relay => write!(f, "relay"),
        }
    }
}

impl IceCandidateType {
    /// Parse candidate type from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "host" => Some(IceCandidateType::Host),
            "srflx" => Some(IceCandidateType::ServerReflexive),
            "prflx" => Some(IceCandidateType::PeerReflexive),
            "relay" => Some(IceCandidateType::Relay),
            _ => None,
        }
    }

    /// Returns the priority modifier for this candidate type.
    /// Higher values indicate more preferred candidate types.
    pub fn priority_modifier(&self) -> u32 {
        match self {
            IceCandidateType::Host => 126,
            IceCandidateType::PeerReflexive => 110,
            IceCandidateType::ServerReflexive => 100,
            IceCandidateType::Relay => 0,
        }
    }
}

/// ICE connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceConnectionState {
    /// Initial state, gathering not started
    New,
    /// Checking connectivity
    Checking,
    /// At least one working candidate pair
    Connected,
    /// All checks completed, best pair selected
    Completed,
    /// Connection temporarily lost
    Disconnected,
    /// All checks failed
    Failed,
    /// Connection closed
    Closed,
}

impl std::fmt::Display for IceConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IceConnectionState::New => write!(f, "new"),
            IceConnectionState::Checking => write!(f, "checking"),
            IceConnectionState::Connected => write!(f, "connected"),
            IceConnectionState::Completed => write!(f, "completed"),
            IceConnectionState::Disconnected => write!(f, "disconnected"),
            IceConnectionState::Failed => write!(f, "failed"),
            IceConnectionState::Closed => write!(f, "closed"),
        }
    }
}

/// ICE candidate information.
#[derive(Debug, Clone)]
pub struct IceCandidate {
    /// Candidate type
    pub candidate_type: IceCandidateType,
    /// Transport protocol (UDP/TCP)
    pub protocol: String,
    /// Candidate address
    pub address: String,
    /// Port number
    pub port: u16,
    /// Priority value
    pub priority: u32,
    /// Foundation (used for frozen candidate optimization)
    pub foundation: String,
    /// Component ID (1 for RTP, 2 for RTCP)
    pub component: u16,
    /// Related address (for srflx/prflx, the base address)
    pub related_address: Option<String>,
    /// Related port
    pub related_port: Option<u16>,
}

impl IceCandidate {
    /// Create a new ICE candidate.
    pub fn new(
        candidate_type: IceCandidateType,
        protocol: &str,
        address: &str,
        port: u16,
        priority: u32,
    ) -> Self {
        Self {
            candidate_type,
            protocol: protocol.to_string(),
            address: address.to_string(),
            port,
            priority,
            foundation: Self::compute_foundation(candidate_type, address),
            component: 1,
            related_address: None,
            related_port: None,
        }
    }

    /// Compute foundation based on candidate type and base address.
    fn compute_foundation(candidate_type: IceCandidateType, address: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(format!("{}{}", candidate_type, address));
        let result = hasher.finalize();
        hex::encode(&result[..4])
    }

    /// Calculate priority for this candidate.
    pub fn calculate_priority(candidate_type: IceCandidateType, component: u16, local_pref: u32) -> u32 {
        // RFC 8445 priority formula:
        // priority = (2^24) * type_preference + (2^8) * local_preference + (256 - component_id)
        let type_pref = candidate_type.priority_modifier() as u32;
        (1 << 24) * type_pref + (1 << 8) * local_pref + (256 - component as u32)
    }

    /// Convert to SDP candidate attribute string.
    pub fn to_sdp_attribute(&self) -> String {
        let mut sdp = format!(
            "candidate:{} {} {} {} {} {} typ {}",
            self.foundation,
            self.component,
            self.protocol,
            self.priority,
            self.address,
            self.port,
            self.candidate_type
        );

        if let (Some(ref raddr), Some(rport)) = (&self.related_address, self.related_port) {
            sdp.push_str(&format!(" raddr {} rport {}", raddr, rport));
        }

        sdp
    }

    /// Parse from SDP candidate attribute.
    pub fn from_sdp_attribute(sdp: &str) -> Result<Self, IceError> {
        // Parse SDP candidate line
        // Format: candidate:foundation component protocol priority address port typ type [raddr addr rport port]
        let parts: Vec<&str> = sdp.split_whitespace().collect();
        if parts.len() < 8 {
            return Err(IceError::InvalidCandidate(format!(
                "insufficient parts: {}",
                sdp
            )));
        }

        let foundation = parts[0]
            .strip_prefix("candidate:")
            .unwrap_or(parts[0])
            .to_string();
        let component: u16 = parts[1]
            .parse()
            .map_err(|_| IceError::InvalidCandidate("invalid component".to_string()))?;
        let protocol = parts[2].to_string();
        let priority: u32 = parts[3]
            .parse()
            .map_err(|_| IceError::InvalidCandidate("invalid priority".to_string()))?;
        let address = parts[4].to_string();
        let port: u16 = parts[5]
            .parse()
            .map_err(|_| IceError::InvalidCandidate("invalid port".to_string()))?;
        // parts[6] should be "typ"
        let type_str = parts[7];
        let candidate_type = IceCandidateType::from_str(type_str)
            .ok_or_else(|| IceError::InvalidCandidate(format!("unknown type: {}", type_str)))?;

        // Parse optional related address/port
        let mut related_address = None;
        let mut related_port = None;
        let mut i = 8;
        while i < parts.len() {
            match parts[i] {
                "raddr" if i + 1 < parts.len() => {
                    related_address = Some(parts[i + 1].to_string());
                    i += 2;
                }
                "rport" if i + 1 < parts.len() => {
                    related_port = parts[i + 1].parse().ok();
                    i += 2;
                }
                _ => i += 1,
            }
        }

        Ok(Self {
            candidate_type,
            protocol,
            address,
            port,
            priority,
            foundation,
            component,
            related_address,
            related_port,
        })
    }
}

/// TURN server configuration.
#[derive(Debug, Clone)]
pub struct TurnServer {
    /// TURN server URL (turn:host:port)
    pub url: String,
    /// Username for authentication
    pub username: String,
    /// Credential for authentication
    pub credential: String,
}

/// ICE configuration.
#[derive(Debug, Clone)]
pub struct IceConfig {
    /// STUN servers for reflexive candidates
    pub stun_servers: Vec<String>,
    /// Optional TURN servers for relay fallback
    pub turn_servers: Vec<TurnServer>,
    /// ICE candidate gathering timeout
    pub gathering_timeout: Duration,
    /// ICE connection timeout
    pub connection_timeout: Duration,
    /// Enable trickle ICE (send candidates as gathered)
    pub trickle_ice: bool,
    /// Maximum number of candidate pairs to check
    pub max_candidate_pairs: usize,
}

impl Default for IceConfig {
    fn default() -> Self {
        Self {
            stun_servers: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
                "stun:stun.cloudflare.com:3478".to_string(),
            ],
            turn_servers: vec![],
            gathering_timeout: Duration::from_secs(10),
            connection_timeout: Duration::from_secs(30),
            trickle_ice: true,
            max_candidate_pairs: 100,
        }
    }
}

impl IceConfig {
    /// Create a new ICE config with custom STUN servers.
    pub fn with_stun_servers(stun_servers: Vec<String>) -> Self {
        Self {
            stun_servers,
            ..Default::default()
        }
    }

    /// Add a TURN server for relay fallback.
    pub fn with_turn_server(mut self, url: &str, username: &str, credential: &str) -> Self {
        self.turn_servers.push(TurnServer {
            url: url.to_string(),
            username: username.to_string(),
            credential: credential.to_string(),
        });
        self
    }

    /// Set gathering timeout.
    pub fn with_gathering_timeout(mut self, timeout: Duration) -> Self {
        self.gathering_timeout = timeout;
        self
    }

    /// Set connection timeout.
    pub fn with_connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = timeout;
        self
    }
}

/// ICE gatherer for collecting candidates.
pub struct IceGatherer {
    /// Configuration
    config: IceConfig,
    /// Gathered candidates
    candidates: Arc<Mutex<Vec<IceCandidate>>>,
}

impl IceGatherer {
    /// Create a new ICE gatherer with the given configuration.
    pub fn new(config: IceConfig) -> Self {
        Self {
            config,
            candidates: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Gather ICE candidates from a peer connection.
    ///
    /// This waits for gathering to complete or timeout, returning
    /// all gathered candidates.
    pub async fn gather_candidates(
        &self,
        peer_connection: &RTCPeerConnection,
    ) -> Result<Vec<IceCandidate>, IceError> {
        let candidates = Arc::new(Mutex::new(Vec::new()));
        let candidates_clone = candidates.clone();

        // Set up candidate handler
        peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
            let candidates = candidates_clone.clone();
            Box::pin(async move {
                if let Some(c) = candidate {
                    if let Ok(ice_candidate) = convert_rtc_candidate(&c) {
                        debug!(
                            "Gathered ICE candidate: {} {} {}:{}",
                            ice_candidate.candidate_type,
                            ice_candidate.protocol,
                            ice_candidate.address,
                            ice_candidate.port
                        );
                        let mut locked = candidates.lock().await;
                        locked.push(ice_candidate);
                    }
                }
            })
        }));

        // Wait for gathering complete with timeout
        let gathering_complete = async {
            loop {
                if peer_connection.ice_gathering_state() == RTCIceGatheringState::Complete {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };

        match timeout(self.config.gathering_timeout, gathering_complete).await {
            Ok(_) => {
                info!("ICE gathering completed");
            }
            Err(_) => {
                warn!(
                    "ICE gathering timed out after {:?}",
                    self.config.gathering_timeout
                );
            }
        }

        let gathered = candidates.lock().await;
        if gathered.is_empty() {
            return Err(IceError::NoCandidates);
        }

        info!("Gathered {} ICE candidates", gathered.len());
        Ok(gathered.clone())
    }

    /// Get the current configuration.
    pub fn config(&self) -> &IceConfig {
        &self.config
    }

    /// Set up trickle ICE callback for sending candidates as they're gathered.
    pub fn on_candidate<F>(&self, peer_connection: &RTCPeerConnection, callback: F)
    where
        F: Fn(IceCandidate) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
            let cb = callback.clone();
            Box::pin(async move {
                if let Some(c) = candidate {
                    if let Ok(ice_candidate) = convert_rtc_candidate(&c) {
                        cb(ice_candidate);
                    }
                }
            })
        }));
    }
}

/// Convert WebRTC ICE candidate to our format.
fn convert_rtc_candidate(rtc: &RTCIceCandidate) -> Result<IceCandidate, IceError> {
    let candidate_type = match rtc.typ.to_string().to_lowercase().as_str() {
        "host" => IceCandidateType::Host,
        "srflx" => IceCandidateType::ServerReflexive,
        "prflx" => IceCandidateType::PeerReflexive,
        "relay" => IceCandidateType::Relay,
        other => {
            return Err(IceError::InvalidCandidate(format!(
                "unknown candidate type: {}",
                other
            )))
        }
    };

    // Handle related address - empty string means no related address
    let related_address = if rtc.related_address.is_empty() {
        None
    } else {
        Some(rtc.related_address.clone())
    };

    // Handle related port - 0 typically means no related port
    let related_port = if rtc.related_port == 0 {
        None
    } else {
        Some(rtc.related_port)
    };

    Ok(IceCandidate {
        candidate_type,
        protocol: rtc.protocol.to_string(),
        address: rtc.address.clone(),
        port: rtc.port,
        priority: rtc.priority,
        foundation: rtc.foundation.clone(),
        component: rtc.component,
        related_address,
        related_port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_type_display() {
        assert_eq!(IceCandidateType::Host.to_string(), "host");
        assert_eq!(IceCandidateType::ServerReflexive.to_string(), "srflx");
        assert_eq!(IceCandidateType::PeerReflexive.to_string(), "prflx");
        assert_eq!(IceCandidateType::Relay.to_string(), "relay");
    }

    #[test]
    fn test_candidate_type_from_str() {
        assert_eq!(
            IceCandidateType::from_str("host"),
            Some(IceCandidateType::Host)
        );
        assert_eq!(
            IceCandidateType::from_str("srflx"),
            Some(IceCandidateType::ServerReflexive)
        );
        assert_eq!(
            IceCandidateType::from_str("RELAY"),
            Some(IceCandidateType::Relay)
        );
        assert_eq!(IceCandidateType::from_str("invalid"), None);
    }

    #[test]
    fn test_candidate_priority() {
        // Host candidates should have highest priority
        let host_priority = IceCandidate::calculate_priority(IceCandidateType::Host, 1, 65535);
        let relay_priority = IceCandidate::calculate_priority(IceCandidateType::Relay, 1, 65535);
        assert!(host_priority > relay_priority);
    }

    #[test]
    fn test_sdp_candidate_roundtrip() {
        let candidate = IceCandidate::new(
            IceCandidateType::Host,
            "udp",
            "192.168.1.100",
            12345,
            2130706431,
        );

        let sdp = candidate.to_sdp_attribute();
        let parsed = IceCandidate::from_sdp_attribute(&sdp).unwrap();

        assert_eq!(parsed.candidate_type, candidate.candidate_type);
        assert_eq!(parsed.protocol, candidate.protocol);
        assert_eq!(parsed.address, candidate.address);
        assert_eq!(parsed.port, candidate.port);
    }

    #[test]
    fn test_sdp_candidate_with_related() {
        let sdp = "candidate:abc123 1 udp 1694498815 203.0.113.50 54321 typ srflx raddr 192.168.1.100 rport 12345";
        let parsed = IceCandidate::from_sdp_attribute(sdp).unwrap();

        assert_eq!(parsed.candidate_type, IceCandidateType::ServerReflexive);
        assert_eq!(parsed.address, "203.0.113.50");
        assert_eq!(parsed.port, 54321);
        assert_eq!(parsed.related_address, Some("192.168.1.100".to_string()));
        assert_eq!(parsed.related_port, Some(12345));
    }

    #[test]
    fn test_ice_config_default() {
        let config = IceConfig::default();
        assert_eq!(config.stun_servers.len(), 3);
        assert!(config.turn_servers.is_empty());
        assert!(config.trickle_ice);
        assert_eq!(config.gathering_timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_ice_config_builder() {
        let config = IceConfig::default()
            .with_gathering_timeout(Duration::from_secs(5))
            .with_connection_timeout(Duration::from_secs(15))
            .with_turn_server("turn:example.com:3478", "user", "pass");

        assert_eq!(config.gathering_timeout, Duration::from_secs(5));
        assert_eq!(config.connection_timeout, Duration::from_secs(15));
        assert_eq!(config.turn_servers.len(), 1);
        assert_eq!(config.turn_servers[0].username, "user");
    }

    #[test]
    fn test_ice_connection_state_display() {
        assert_eq!(IceConnectionState::New.to_string(), "new");
        assert_eq!(IceConnectionState::Checking.to_string(), "checking");
        assert_eq!(IceConnectionState::Connected.to_string(), "connected");
        assert_eq!(IceConnectionState::Failed.to_string(), "failed");
    }
}
