// Copyright (c) 2024 Botho Foundation

//! Pluggable transport trait and related abstractions.
//!
//! This module defines the core [`PluggableTransport`] trait that all
//! transport implementations must satisfy. It provides a unified interface
//! for establishing connections regardless of the underlying protocol.
//!
//! # Architecture
//!
//! The transport layer sits between the application (gossipsub) and the
//! network. Each transport wraps raw connections with its own encryption
//! and framing:
//!
//! ```text
//! ┌─────────────────┐
//! │   Application   │
//! │   (Gossipsub)   │
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │  PluggableTransport │  ← This trait
//! │  (Plain/WebRTC/TLS) │
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │    Network      │
//! │   (TCP/UDP)     │
//! └─────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::{PluggableTransport, TransportType};
//!
//! async fn connect_to_peer(transport: &dyn PluggableTransport, peer: &PeerId) {
//!     match transport.connect(peer).await {
//!         Ok(conn) => {
//!             // Use conn for reading/writing
//!         }
//!         Err(e) => {
//!             eprintln!("Connection failed: {}", e);
//!         }
//!     }
//! }
//! ```

use async_trait::async_trait;
use libp2p::PeerId;
use std::fmt::Debug;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::error::TransportError;
use super::types::TransportType;

/// A connection that can be read from and written to asynchronously.
///
/// This trait combines [`AsyncRead`] and [`AsyncWrite`] with [`Send`] and [`Sync`]
/// bounds required for use across async tasks. All transport connections must
/// implement this trait.
pub trait TransportConnection: AsyncRead + AsyncWrite + Send + Sync + Unpin + Debug {}

/// Blanket implementation for any type that satisfies the bounds.
impl<T> TransportConnection for T where T: AsyncRead + AsyncWrite + Send + Sync + Unpin + Debug {}

/// A boxed transport connection for dynamic dispatch.
pub type BoxedConnection = Box<dyn TransportConnection>;

/// Pluggable transport interface for protocol obfuscation.
///
/// This trait defines the interface that all transport implementations must
/// satisfy. Each transport provides a different way to establish connections
/// with peers, with different trade-offs for:
///
/// - **Performance**: Connection setup time, throughput
/// - **Compatibility**: NAT traversal, firewall penetration
/// - **Obfuscation**: Resistance to deep packet inspection
///
/// # Implementors
///
/// - [`PlainTransport`]: Standard TCP + Noise (default)
/// - `WebRtcTransport`: WebRTC data channels (Phase 3)
/// - `TlsTunnelTransport`: TLS tunnel (Phase 3)
///
/// # Thread Safety
///
/// All transport implementations must be `Send + Sync` to allow use across
/// async tasks and threads.
#[async_trait]
pub trait PluggableTransport: Send + Sync + Debug {
    /// Get the transport type identifier.
    fn transport_type(&self) -> TransportType;

    /// Get the human-readable name of this transport.
    fn name(&self) -> &'static str {
        self.transport_type().name()
    }

    /// Check if this transport is available and ready to use.
    ///
    /// This may check for required dependencies, network conditions,
    /// or configuration. Returns `true` if the transport can be used.
    fn is_available(&self) -> bool {
        true
    }

    /// Establish an outbound connection to a peer.
    ///
    /// This creates a new connection to the specified peer using this
    /// transport's protocol. The returned connection can be used for
    /// bidirectional communication.
    ///
    /// # Arguments
    ///
    /// * `peer` - The peer ID to connect to
    /// * `addr` - Optional multiaddr hint for the peer's address
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established, such as:
    /// - Network unreachable
    /// - Peer not found
    /// - Handshake failure
    /// - Timeout
    async fn connect(
        &self,
        peer: &PeerId,
        addr: Option<&libp2p::Multiaddr>,
    ) -> Result<BoxedConnection, TransportError>;

    /// Accept an inbound connection.
    ///
    /// This wraps an existing raw connection (e.g., from a TCP listener)
    /// with this transport's protocol layer.
    ///
    /// # Arguments
    ///
    /// * `stream` - The raw incoming connection
    ///
    /// # Errors
    ///
    /// Returns an error if the handshake fails or the connection is invalid.
    async fn accept(&self, stream: BoxedConnection) -> Result<BoxedConnection, TransportError>;
}

/// A wrapper around a boxed connection that implements the standard I/O traits.
///
/// This allows using a `BoxedConnection` with APIs that expect concrete types
/// rather than trait objects.
#[derive(Debug)]
pub struct ConnectionWrapper {
    inner: BoxedConnection,
}

impl ConnectionWrapper {
    /// Create a new connection wrapper.
    pub fn new(conn: BoxedConnection) -> Self {
        Self { inner: conn }
    }

    /// Get a reference to the inner connection.
    pub fn inner(&self) -> &BoxedConnection {
        &self.inner
    }

    /// Get a mutable reference to the inner connection.
    pub fn inner_mut(&mut self) -> &mut BoxedConnection {
        &mut self.inner
    }

    /// Consume the wrapper and return the inner connection.
    pub fn into_inner(self) -> BoxedConnection {
        self.inner
    }
}

impl AsyncRead for ConnectionWrapper {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for ConnectionWrapper {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A simple in-memory connection for testing.
    #[derive(Debug)]
    struct MockConnection {
        read_buf: Cursor<Vec<u8>>,
        write_buf: Vec<u8>,
    }

    impl MockConnection {
        fn new(data: Vec<u8>) -> Self {
            Self {
                read_buf: Cursor::new(data),
                write_buf: Vec::new(),
            }
        }

        fn written(&self) -> &[u8] {
            &self.write_buf
        }
    }

    impl AsyncRead for MockConnection {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let pos = self.read_buf.position() as usize;
            let data = self.read_buf.get_ref();
            let remaining = &data[pos..];
            let to_read = std::cmp::min(remaining.len(), buf.remaining());
            buf.put_slice(&remaining[..to_read]);
            self.read_buf.set_position((pos + to_read) as u64);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for MockConnection {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.write_buf.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn test_connection_wrapper_read() {
        let mock = MockConnection::new(b"hello world".to_vec());
        let mut wrapper = ConnectionWrapper::new(Box::new(mock));

        let mut buf = [0u8; 5];
        let n = wrapper.read(&mut buf).await.unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn test_connection_wrapper_write() {
        let mock = MockConnection::new(vec![]);
        let mut wrapper = ConnectionWrapper::new(Box::new(mock));

        wrapper.write_all(b"test data").await.unwrap();
        wrapper.flush().await.unwrap();

        // The write completed successfully - that's what we're testing
    }

    // Test that MockConnection implements TransportConnection
    #[test]
    fn test_mock_is_transport_connection() {
        fn assert_transport_connection<T: TransportConnection>() {}
        assert_transport_connection::<MockConnection>();
    }
}
