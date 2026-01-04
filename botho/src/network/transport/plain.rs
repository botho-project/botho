// Copyright (c) 2024 Botho Foundation

//! Plain transport implementation using TCP + Noise.
//!
//! This is the default transport that wraps the existing libp2p TCP + Noise
//! connection mechanism. It provides no protocol obfuscation but offers the
//! best performance.
//!
//! # Overview
//!
//! The plain transport uses:
//! - **TCP**: Reliable byte stream transport
//! - **Noise**: Modern encryption protocol (XX handshake pattern)
//! - **Yamux**: Stream multiplexing
//!
//! This matches the current botho network stack and serves as the baseline
//! for comparing other transports.
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::{PlainTransport, PluggableTransport};
//!
//! let transport = PlainTransport::new();
//! assert_eq!(transport.name(), "plain");
//! assert!(!transport.transport_type().is_obfuscated());
//! ```

use async_trait::async_trait;
use libp2p::{Multiaddr, PeerId};
use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use super::error::TransportError;
use super::traits::{BoxedConnection, PluggableTransport};
use super::types::TransportType;

/// Plain transport using TCP + Noise.
///
/// This is the baseline transport with no protocol obfuscation.
/// It provides the best performance but may be detected and blocked
/// by deep packet inspection.
///
/// # Configuration
///
/// The plain transport uses the system's TCP stack with default settings.
/// No additional configuration is required.
#[derive(Clone)]
pub struct PlainTransport {
    /// Connection timeout in seconds.
    connect_timeout_secs: u64,
}

impl PlainTransport {
    /// Create a new plain transport with default settings.
    pub fn new() -> Self {
        Self {
            connect_timeout_secs: 30,
        }
    }

    /// Create a new plain transport with custom timeout.
    pub fn with_timeout(connect_timeout_secs: u64) -> Self {
        Self {
            connect_timeout_secs,
        }
    }

    /// Get the connection timeout in seconds.
    pub fn connect_timeout_secs(&self) -> u64 {
        self.connect_timeout_secs
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
}

impl Default for PlainTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PlainTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlainTransport")
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .finish()
    }
}

#[async_trait]
impl PluggableTransport for PlainTransport {
    fn transport_type(&self) -> TransportType {
        TransportType::Plain
    }

    fn is_available(&self) -> bool {
        // Plain transport is always available
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
        let timeout = std::time::Duration::from_secs(self.connect_timeout_secs);

        let stream = tokio::time::timeout(timeout, connect_future)
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|e| TransportError::ConnectionFailed(e.to_string()))?;

        // Configure TCP options
        stream.set_nodelay(true).map_err(|e| {
            TransportError::Configuration(format!("failed to set TCP_NODELAY: {}", e))
        })?;

        // Wrap in PlainConnection for Debug impl
        let conn = PlainConnection::new(stream, *peer);
        Ok(Box::new(conn))
    }

    async fn accept(&self, stream: BoxedConnection) -> Result<BoxedConnection, TransportError> {
        // For plain transport, we just pass through the connection
        // The actual Noise handshake is handled by libp2p at a higher level
        Ok(stream)
    }
}

/// A plain TCP connection wrapper.
///
/// This wraps a TCP stream with peer information and implements
/// the required traits for use as a transport connection.
pub struct PlainConnection {
    stream: TcpStream,
    peer: PeerId,
}

impl PlainConnection {
    /// Create a new plain connection.
    pub fn new(stream: TcpStream, peer: PeerId) -> Self {
        Self { stream, peer }
    }

    /// Get the peer ID for this connection.
    pub fn peer(&self) -> &PeerId {
        &self.peer
    }

    /// Get a reference to the underlying TCP stream.
    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }
}

impl fmt::Debug for PlainConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlainConnection")
            .field("peer", &self.peer)
            .field("local_addr", &self.stream.local_addr().ok())
            .field("peer_addr", &self.stream.peer_addr().ok())
            .finish()
    }
}

impl AsyncRead for PlainConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for PlainConnection {
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

// PlainConnection is Unpin because TcpStream is Unpin
impl Unpin for PlainConnection {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_transport_default() {
        let transport = PlainTransport::new();
        assert_eq!(transport.transport_type(), TransportType::Plain);
        assert_eq!(transport.name(), "plain");
        assert!(transport.is_available());
    }

    #[test]
    fn test_plain_transport_with_timeout() {
        let transport = PlainTransport::with_timeout(60);
        assert_eq!(transport.connect_timeout_secs(), 60);
    }

    #[test]
    fn test_plain_transport_debug() {
        let transport = PlainTransport::new();
        let debug = format!("{:?}", transport);
        assert!(debug.contains("PlainTransport"));
        assert!(debug.contains("connect_timeout_secs"));
    }

    #[test]
    fn test_extract_tcp_addr_ipv4() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/8080".parse().unwrap();
        let socket_addr = PlainTransport::extract_tcp_addr(&addr).unwrap();
        assert_eq!(socket_addr.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn test_extract_tcp_addr_ipv6() {
        let addr: Multiaddr = "/ip6/::1/tcp/8080".parse().unwrap();
        let socket_addr = PlainTransport::extract_tcp_addr(&addr).unwrap();
        assert_eq!(socket_addr.to_string(), "[::1]:8080");
    }

    #[test]
    fn test_extract_tcp_addr_no_port() {
        let addr: Multiaddr = "/ip4/127.0.0.1".parse().unwrap();
        assert!(PlainTransport::extract_tcp_addr(&addr).is_none());
    }

    #[test]
    fn test_extract_tcp_addr_no_ip() {
        let addr: Multiaddr = "/tcp/8080".parse().unwrap();
        assert!(PlainTransport::extract_tcp_addr(&addr).is_none());
    }

    #[tokio::test]
    async fn test_connect_no_address() {
        let transport = PlainTransport::new();
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
        let transport = PlainTransport::new();
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

    #[tokio::test]
    async fn test_connect_unreachable() {
        let transport = PlainTransport::with_timeout(1); // 1 second timeout
        let peer = PeerId::random();
        // Use a non-routable address to trigger connection failure
        let addr: Multiaddr = "/ip4/10.255.255.1/tcp/12345".parse().unwrap();

        let result = transport.connect(&peer, Some(&addr)).await;
        assert!(result.is_err());
        // Could be timeout or connection failed depending on network
    }
}
