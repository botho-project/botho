// Copyright (c) 2024 Botho Foundation

//! Transport manager for intelligent transport selection.
//!
//! This module provides the `TransportSelector` which manages transport
//! selection, connection attempts with fallback, and metrics tracking.
//!
//! # Architecture
//!
//! The transport selector sits between the application and individual
//! transports, providing:
//!
//! - Intelligent transport selection based on capabilities and metrics
//! - Automatic fallback on connection failures
//! - Metrics tracking for improving selection over time
//! - Integration with privacy configuration
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::manager::TransportSelector;
//! use botho::network::transport::config::TransportConfig;
//!
//! // Create selector with full transport support
//! let config = TransportConfig::full();
//! let selector = TransportSelector::new(config);
//!
//! // Select best transport for a peer
//! let transport = selector.select_for_peer(&peer_info);
//!
//! // Connect with automatic fallback
//! let connection = selector.connect_with_fallback(&peer_id, &addr).await?;
//! ```

use std::sync::{Arc, RwLock};

use libp2p::{Multiaddr, PeerId};

use super::{
    capabilities::{NatType, TransportCapabilities, TransportType as CapabilityTransportType},
    config::{TransportConfig, TransportPreference},
    error::TransportError,
    metrics::{ConnectResult, TransportMetrics},
    plain::PlainTransport,
    traits::{BoxedConnection, PluggableTransport},
    types::TransportType,
};

/// Peer information for transport selection.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Peer's transport capabilities.
    pub capabilities: TransportCapabilities,
    /// Peer's multiaddr (for connection).
    pub addr: Option<Multiaddr>,
}

impl PeerInfo {
    /// Create peer info with capabilities.
    pub fn with_capabilities(capabilities: TransportCapabilities) -> Self {
        Self {
            capabilities,
            addr: None,
        }
    }

    /// Create peer info from capabilities string suffix.
    pub fn from_capabilities_suffix(suffix: &str) -> Option<Self> {
        TransportCapabilities::from_multiaddr_suffix(suffix).map(Self::with_capabilities)
    }

    /// Add address to peer info.
    pub fn with_addr(mut self, addr: Multiaddr) -> Self {
        self.addr = Some(addr);
        self
    }
}

/// Result of a connection attempt with transport information.
#[derive(Debug)]
pub struct ConnectionResult {
    /// The established connection.
    pub connection: BoxedConnection,
    /// The transport type used.
    pub transport_type: TransportType,
    /// Number of fallback attempts made.
    pub fallback_attempts: u32,
}

/// Transport selector for intelligent transport selection.
///
/// Manages available transports, selects the best transport for each
/// connection based on capabilities, preferences, and metrics, and
/// handles fallback on connection failures.
#[derive(Debug)]
pub struct TransportSelector {
    /// Configuration for transport selection.
    config: TransportConfig,
    /// Our transport capabilities to advertise.
    capabilities: TransportCapabilities,
    /// Transport metrics for selection improvement.
    metrics: Arc<RwLock<TransportMetrics>>,
    /// Available transports.
    transports: Vec<Arc<dyn PluggableTransport>>,
}

impl TransportSelector {
    /// Create a new transport selector with the given configuration.
    pub fn new(config: TransportConfig) -> Self {
        Self::with_nat_type(config, NatType::Unknown)
    }

    /// Create a new transport selector with known NAT type.
    pub fn with_nat_type(config: TransportConfig, nat_type: NatType) -> Self {
        let transports = Self::create_transports(&config);
        let capabilities = Self::create_capabilities(&config, nat_type);

        Self {
            config,
            capabilities,
            metrics: Arc::new(RwLock::new(TransportMetrics::new())),
            transports,
        }
    }

    /// Create available transports based on configuration.
    fn create_transports(config: &TransportConfig) -> Vec<Arc<dyn PluggableTransport>> {
        let mut transports: Vec<Arc<dyn PluggableTransport>> = Vec::new();

        // Plain transport is always available
        transports.push(Arc::new(PlainTransport::with_timeout(
            config.connect_timeout_secs,
        )));

        // WebRTC transport will be added here when it implements PluggableTransport
        // (pending issue #203: Implement WebRTC data channel transport)
        // if config.enable_webrtc {
        //     transports.push(Arc::new(WebRtcTransport::with_defaults()));
        // }

        // TLS tunnel transport will be added here when implemented
        // if config.enable_tls_tunnel {
        //     transports.push(Arc::new(TlsTunnelTransport::new(config.tls_config)));
        // }

        transports
    }

    /// Create capabilities based on configuration.
    fn create_capabilities(config: &TransportConfig, nat_type: NatType) -> TransportCapabilities {
        let mut supported = vec![CapabilityTransportType::Plain];

        if config.enable_webrtc {
            supported.insert(0, CapabilityTransportType::WebRTC);
        }

        if config.enable_tls_tunnel {
            supported.insert(
                if config.enable_webrtc { 1 } else { 0 },
                CapabilityTransportType::TlsTunnel,
            );
        }

        let preferred = match config.preference {
            TransportPreference::Privacy => {
                if config.enable_webrtc {
                    CapabilityTransportType::WebRTC
                } else if config.enable_tls_tunnel {
                    CapabilityTransportType::TlsTunnel
                } else {
                    CapabilityTransportType::Plain
                }
            }
            TransportPreference::Performance => CapabilityTransportType::Plain,
            TransportPreference::Compatibility => {
                if config.enable_webrtc {
                    CapabilityTransportType::WebRTC
                } else {
                    CapabilityTransportType::Plain
                }
            }
            TransportPreference::Specific(t) => Self::convert_transport_type(t),
        };

        TransportCapabilities::new(supported, preferred, nat_type)
    }

    /// Convert between transport type enums.
    fn convert_transport_type(t: TransportType) -> CapabilityTransportType {
        match t {
            TransportType::Plain => CapabilityTransportType::Plain,
            TransportType::WebRTC => CapabilityTransportType::WebRTC,
            TransportType::TlsTunnel => CapabilityTransportType::TlsTunnel,
        }
    }

    /// Convert from capability transport type.
    fn from_capability_type(t: CapabilityTransportType) -> TransportType {
        match t {
            CapabilityTransportType::Plain => TransportType::Plain,
            CapabilityTransportType::WebRTC => TransportType::WebRTC,
            CapabilityTransportType::TlsTunnel => TransportType::TlsTunnel,
        }
    }

    /// Get our transport capabilities for advertising.
    pub fn capabilities(&self) -> &TransportCapabilities {
        &self.capabilities
    }

    /// Get the capabilities as a multiaddr suffix string.
    pub fn capabilities_suffix(&self) -> String {
        self.capabilities.to_multiaddr_suffix()
    }

    /// Get the configuration.
    pub fn config(&self) -> &TransportConfig {
        &self.config
    }

    /// Get the transport metrics.
    pub fn metrics(&self) -> Arc<RwLock<TransportMetrics>> {
        self.metrics.clone()
    }

    /// Select the best transport for a peer.
    ///
    /// Uses capability-based selection enhanced with metrics data.
    pub fn select_for_peer(&self, peer: &PeerInfo) -> TransportType {
        // Get the best common transport based on capabilities
        let capability_best = self
            .capabilities
            .best_common(&peer.capabilities)
            .unwrap_or(CapabilityTransportType::Plain);

        let base_transport = Self::from_capability_type(capability_best);

        // If metrics are disabled, return the capability-based selection
        if !self.config.enable_metrics {
            return base_transport;
        }

        // Get available transports that both peers support
        let available: Vec<TransportType> = self
            .config
            .enabled_transports()
            .into_iter()
            .filter(|&t| peer.capabilities.supports(Self::convert_transport_type(t)))
            .collect();

        if available.is_empty() {
            return TransportType::Plain;
        }

        // Use metrics to adjust selection
        let metrics = self.metrics.read().unwrap();

        // Get metrics recommendation
        if let Some(recommended) = metrics.recommend(&available) {
            // Check if metrics-recommended differs from capability-recommended
            if recommended != base_transport {
                // Only override if metrics show significant difference
                let base_rate = metrics.success_rate(base_transport);
                let recommended_rate = metrics.success_rate(recommended);

                // Switch if recommended has notably better success rate
                if recommended_rate > base_rate + 0.15 {
                    return recommended;
                }
            }
        }

        // Apply preference scoring
        let mut best: Option<(TransportType, i32)> = None;

        for transport in &available {
            let mut score: i32 = 0;

            // Base preference score
            score += self.config.preference.score_for(*transport);

            // Metrics-based score adjustment
            let success_rate = metrics.success_rate(*transport);
            score += (success_rate * 30.0) as i32;

            // Penalty for transport that should be avoided
            if let Some(stats) = metrics.get_stats(*transport) {
                if stats.should_avoid() {
                    score -= 50;
                }
            }

            match &best {
                Some((_, best_score)) if score > *best_score => {
                    best = Some((*transport, score));
                }
                None => {
                    best = Some((*transport, score));
                }
                _ => {}
            }
        }

        best.map(|(t, _)| t).unwrap_or(TransportType::Plain)
    }

    /// Get the transport implementation for a transport type.
    fn get_transport(&self, transport_type: TransportType) -> Option<&Arc<dyn PluggableTransport>> {
        self.transports
            .iter()
            .find(|t| t.transport_type() == transport_type)
    }

    /// Get transports in fallback order for a peer.
    fn get_fallback_order(&self, peer: &PeerInfo) -> Vec<TransportType> {
        let primary = self.select_for_peer(peer);
        let mut order = vec![primary];

        // Add other enabled transports as fallbacks
        for transport in self.config.enabled_transports() {
            if transport != primary && !order.contains(&transport) {
                // Check if peer supports this transport
                if peer
                    .capabilities
                    .supports(Self::convert_transport_type(transport))
                {
                    order.push(transport);
                }
            }
        }

        // Always include plain as last resort
        if !order.contains(&TransportType::Plain) {
            order.push(TransportType::Plain);
        }

        order
    }

    /// Connect to a peer using the best transport.
    ///
    /// Attempts connection with the selected transport.
    pub async fn connect(
        &self,
        peer_id: &PeerId,
        peer: &PeerInfo,
    ) -> Result<ConnectionResult, TransportError> {
        let transport_type = self.select_for_peer(peer);
        let transport = self.get_transport(transport_type).ok_or_else(|| {
            TransportError::Configuration(format!(
                "transport {} not available",
                transport_type.name()
            ))
        })?;

        let start = std::time::Instant::now();
        let result = transport.connect(peer_id, peer.addr.as_ref()).await;

        // Record metrics
        {
            let mut metrics = self.metrics.write().unwrap();
            match &result {
                Ok(_) => {
                    metrics.record(transport_type, ConnectResult::success(start.elapsed()));
                }
                Err(TransportError::Timeout) => {
                    metrics.record(transport_type, ConnectResult::timeout());
                }
                Err(e) => {
                    metrics.record(transport_type, ConnectResult::failure(e.to_string()));
                }
            }
        }

        result.map(|connection| ConnectionResult {
            connection,
            transport_type,
            fallback_attempts: 0,
        })
    }

    /// Connect to a peer with automatic fallback on failure.
    ///
    /// Tries the best transport first, then falls back to other transports
    /// if the connection fails and fallback is enabled.
    pub async fn connect_with_fallback(
        &self,
        peer_id: &PeerId,
        peer: &PeerInfo,
    ) -> Result<ConnectionResult, TransportError> {
        if !self.config.enable_fallback {
            return self.connect(peer_id, peer).await;
        }

        let fallback_order = self.get_fallback_order(peer);
        let max_attempts = self
            .config
            .max_fallback_attempts
            .min(fallback_order.len() as u32);

        let mut last_error = None;
        let mut attempts = 0;

        for transport_type in fallback_order.iter().take(max_attempts as usize) {
            let transport = match self.get_transport(*transport_type) {
                Some(t) => t,
                None => continue,
            };

            let start = std::time::Instant::now();
            match transport.connect(peer_id, peer.addr.as_ref()).await {
                Ok(connection) => {
                    // Record success
                    {
                        let mut metrics = self.metrics.write().unwrap();
                        metrics.record(*transport_type, ConnectResult::success(start.elapsed()));
                    }

                    return Ok(ConnectionResult {
                        connection,
                        transport_type: *transport_type,
                        fallback_attempts: attempts,
                    });
                }
                Err(e) => {
                    // Record failure
                    {
                        let mut metrics = self.metrics.write().unwrap();
                        match &e {
                            TransportError::Timeout => {
                                metrics.record(*transport_type, ConnectResult::timeout());
                            }
                            _ => {
                                metrics
                                    .record(*transport_type, ConnectResult::failure(e.to_string()));
                            }
                        }
                    }

                    last_error = Some(e);
                    attempts += 1;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            TransportError::ConnectionFailed("all transports failed".to_string())
        }))
    }

    /// Update NAT type after detection.
    ///
    /// This should be called when STUN completes NAT detection.
    pub fn update_nat_type(&mut self, nat_type: NatType) {
        self.capabilities = Self::create_capabilities(&self.config, nat_type);
    }

    /// Reset metrics for all transports.
    pub fn reset_metrics(&self) {
        let mut metrics = self.metrics.write().unwrap();
        metrics.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_peer_info() -> PeerInfo {
        PeerInfo::with_capabilities(TransportCapabilities::full(NatType::Open))
    }

    fn plain_only_peer_info() -> PeerInfo {
        PeerInfo::with_capabilities(TransportCapabilities::plain_only())
    }

    #[test]
    fn test_transport_selector_new() {
        let config = TransportConfig::default();
        let selector = TransportSelector::new(config);

        assert!(!selector.transports.is_empty());
        assert!(selector.get_transport(TransportType::Plain).is_some());
    }

    #[test]
    fn test_transport_selector_with_full_config() {
        let config = TransportConfig::full();
        let selector = TransportSelector::new(config);

        assert!(selector
            .capabilities
            .supports(CapabilityTransportType::Plain));
        assert!(selector
            .capabilities
            .supports(CapabilityTransportType::WebRTC));
    }

    #[test]
    fn test_select_for_peer_plain_only() {
        let config = TransportConfig::default();
        let selector = TransportSelector::new(config);

        let peer = default_peer_info();
        let selected = selector.select_for_peer(&peer);

        assert_eq!(selected, TransportType::Plain);
    }

    #[test]
    fn test_select_for_peer_common_transport() {
        let config = TransportConfig::builder()
            .preference(TransportPreference::Privacy)
            .enable_webrtc(true)
            .build();
        let selector = TransportSelector::with_nat_type(config, NatType::Open);

        let peer = default_peer_info();
        let selected = selector.select_for_peer(&peer);

        // Should select WebRTC since both support it and privacy is preferred
        assert_eq!(selected, TransportType::WebRTC);
    }

    #[test]
    fn test_select_for_peer_falls_back_to_common() {
        let config = TransportConfig::builder()
            .preference(TransportPreference::Privacy)
            .enable_webrtc(true)
            .build();
        let selector = TransportSelector::with_nat_type(config, NatType::Open);

        // Peer only supports plain
        let peer = plain_only_peer_info();
        let selected = selector.select_for_peer(&peer);

        assert_eq!(selected, TransportType::Plain);
    }

    #[test]
    fn test_capabilities_suffix() {
        let config = TransportConfig::builder().enable_webrtc(true).build();
        let selector = TransportSelector::with_nat_type(config, NatType::Open);

        let suffix = selector.capabilities_suffix();
        assert!(suffix.starts_with("/transport-caps/"));
        assert!(suffix.contains("webrtc"));
    }

    #[test]
    fn test_get_fallback_order() {
        let config = TransportConfig::builder()
            .preference(TransportPreference::Privacy)
            .enable_webrtc(true)
            .enable_tls_tunnel(true)
            .build();
        let selector = TransportSelector::with_nat_type(config, NatType::Open);

        let peer = default_peer_info();
        let order = selector.get_fallback_order(&peer);

        // Should have WebRTC first (privacy preference), then TLS, then Plain
        assert!(!order.is_empty());
        assert!(order.contains(&TransportType::Plain)); // Plain always included
    }

    #[test]
    fn test_update_nat_type() {
        let config = TransportConfig::default();
        let mut selector = TransportSelector::new(config);

        assert_eq!(selector.capabilities.nat_type, NatType::Unknown);

        selector.update_nat_type(NatType::FullCone);
        assert_eq!(selector.capabilities.nat_type, NatType::FullCone);
    }

    #[test]
    fn test_reset_metrics() {
        let config = TransportConfig::default();
        let selector = TransportSelector::new(config);

        // Record some metrics
        {
            let mut metrics = selector.metrics.write().unwrap();
            metrics.record(
                TransportType::Plain,
                ConnectResult::success(std::time::Duration::from_millis(100)),
            );
        }

        // Verify metrics exist
        {
            let metrics = selector.metrics.read().unwrap();
            assert!(metrics.get_stats(TransportType::Plain).is_some());
        }

        // Reset and verify cleared
        selector.reset_metrics();
        {
            let metrics = selector.metrics.read().unwrap();
            assert!(metrics.get_stats(TransportType::Plain).is_none());
        }
    }

    #[test]
    fn test_peer_info_from_suffix() {
        let suffix = "/transport-caps/1/webrtc,plain/open";
        let peer = PeerInfo::from_capabilities_suffix(suffix);

        assert!(peer.is_some());
        let peer = peer.unwrap();
        assert!(peer.capabilities.supports(CapabilityTransportType::WebRTC));
        assert!(peer.capabilities.supports(CapabilityTransportType::Plain));
    }

    #[test]
    fn test_connection_result() {
        let config = TransportConfig::default();
        let selector = TransportSelector::new(config);

        // Verify transport is available
        assert!(selector.get_transport(TransportType::Plain).is_some());
    }
}
