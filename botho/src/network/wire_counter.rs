// Copyright (c) 2024 Botho Foundation

//! Wire-level byte counting for the libp2p transport (#550).
//!
//! The application-payload counters from #542/#548/#549 (gossipsub publish/
//! receive and sync request/response codec) count only the *useful* bytes a
//! node exchanges. They deliberately ignore transport framing overhead: the
//! Noise handshake, yamux stream framing, and protocol negotiation. For "is
//! traffic flowing" diagnostics that is exactly right, but it cannot answer
//! "how much bandwidth did this node actually consume" — the question you need
//! for bandwidth-billing a managed rig.
//!
//! This module closes that gap with a thin counting wrapper installed *below*
//! the Noise and yamux upgrades. Because it sits on the raw TCP byte stream,
//! every byte it observes is a true byte-on-wire: Noise handshake bytes, yamux
//! frame headers, and all multiplexed payload. The only thing it cannot see is
//! the kernel-level TCP/IP header, which never reaches user space.
//!
//! ## Design
//!
//! [`CountingStream`] wraps any [`AsyncRead`] + [`AsyncWrite`] stream and feeds
//! every successful read/write into the shared [`NetworkStats`] wire counters.
//! [`with_wire_counting`] applies that wrapper to a raw TCP transport via
//! [`Transport::map`], yielding a transport whose connections are counted but
//! that is otherwise identical to the one libp2p's [`SwarmBuilder::with_tcp`]
//! builds internally.
//!
//! [`SwarmBuilder::with_tcp`]: libp2p::SwarmBuilder::with_tcp

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use futures::{AsyncRead, AsyncWrite};
use libp2p::core::transport::Transport;

use super::discovery::NetworkStats;

/// An [`AsyncRead`] + [`AsyncWrite`] stream that records every byte it reads or
/// writes into a shared [`NetworkStats`] as raw wire traffic (#550).
///
/// The wrapper is intentionally transparent: it forwards every poll to the
/// inner stream unchanged and only increments a counter on the bytes that were
/// actually transferred (`Poll::Ready(Ok(n))`). Errors and pending polls move
/// no bytes and so move no counters.
#[derive(Debug)]
pub struct CountingStream<S> {
    inner: S,
    stats: Arc<NetworkStats>,
}

impl<S> CountingStream<S> {
    /// Wrap `inner`, recording its traffic into `stats`.
    pub fn new(inner: S, stats: Arc<NetworkStats>) -> Self {
        Self { inner, stats }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountingStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(n)) = &poll {
            self.stats.record_wire_received(*n as u64);
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &poll {
            self.stats.record_wire_sent(*n as u64);
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

/// Wrap a raw (pre-upgrade) transport so each connection it produces counts its
/// raw wire bytes into `stats` (#550).
///
/// Apply this to the base TCP transport *before* authenticating with Noise and
/// multiplexing with yamux, so the counting stream sees framing overhead too.
/// The returned transport has the same output stream type wrapped in a
/// [`CountingStream`]; it can be fed straight into
/// `.upgrade(...).authenticate(...).multiplex(...)`.
pub fn with_wire_counting<T>(
    transport: T,
    stats: Arc<NetworkStats>,
) -> impl Transport<
    Output = CountingStream<T::Output>,
    Error = T::Error,
    Dial = impl Send,
    ListenerUpgrade = impl Send,
>
where
    T: Transport,
    T::Output: AsyncRead + AsyncWrite + Unpin,
    T::Dial: Send,
    T::ListenerUpgrade: Send,
{
    transport.map(move |output, _endpoint| CountingStream::new(output, Arc::clone(&stats)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{AsyncReadExt, AsyncWriteExt};

    /// An in-memory duplex stream backed by two byte vectors, used to drive the
    /// counting wrapper without real sockets.
    struct MockStream {
        to_read: std::collections::VecDeque<u8>,
        written: Vec<u8>,
    }

    impl AsyncRead for MockStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<std::io::Result<usize>> {
            let n = self.to_read.len().min(buf.len());
            for slot in buf.iter_mut().take(n) {
                *slot = self.to_read.pop_front().unwrap();
            }
            Poll::Ready(Ok(n))
        }
    }

    impl AsyncWrite for MockStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn counts_bytes_written() {
        let stats = Arc::new(NetworkStats::new());
        let inner = MockStream {
            to_read: Default::default(),
            written: Vec::new(),
        };
        let mut stream = CountingStream::new(inner, Arc::clone(&stats));

        stream.write_all(&[1, 2, 3, 4, 5]).await.unwrap();
        stream.write_all(&[6, 7, 8]).await.unwrap();

        assert_eq!(stats.wire_bytes_sent(), 8);
        assert_eq!(
            stats.wire_bytes_received(),
            0,
            "writes must not touch the receive counter"
        );
    }

    #[tokio::test]
    async fn counts_bytes_read() {
        let stats = Arc::new(NetworkStats::new());
        let inner = MockStream {
            to_read: (0u8..10).collect(),
            written: Vec::new(),
        };
        let mut stream = CountingStream::new(inner, Arc::clone(&stats));

        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [0, 1, 2, 3]);
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [4, 5, 6, 7]);

        assert_eq!(stats.wire_bytes_received(), 8);
        assert_eq!(
            stats.wire_bytes_sent(),
            0,
            "reads must not touch the send counter"
        );
    }

    #[tokio::test]
    async fn counters_are_independent_and_cumulative() {
        let stats = Arc::new(NetworkStats::new());
        let inner = MockStream {
            to_read: (0u8..6).collect(),
            written: Vec::new(),
        };
        let mut stream = CountingStream::new(inner, Arc::clone(&stats));

        stream.write_all(&[9, 9]).await.unwrap();
        let mut buf = [0u8; 6];
        stream.read_exact(&mut buf).await.unwrap();
        stream.write_all(&[9]).await.unwrap();

        assert_eq!(stats.wire_bytes_sent(), 3);
        assert_eq!(stats.wire_bytes_received(), 6);
    }
}
