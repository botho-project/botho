// Copyright (c) 2024 Botho Foundation

//! Constant-rate transmitter for traffic normalization (Phase 2).
//!
//! This module implements the constant-rate transmitter described in Section
//! 2.2 of the traffic privacy roadmap. It provides traffic normalization by
//! sending messages at a fixed rate, optionally generating cover traffic when
//! the queue is empty.
//!
//! # Overview
//!
//! The `ConstantRateTransmitter` queues outgoing messages and sends them at a
//! constant rate (e.g., 2 messages per second). This prevents timing analysis
//! attacks that could correlate user activity with network traffic patterns.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                 CONSTANT RATE TRANSMITTER                            │
//! │                                                                     │
//! │   User creates transaction                                          │
//! │       │                                                             │
//! │       ▼                                                             │
//! │   enqueue(msg)                                                      │
//! │       │                                                             │
//! │       ▼                                                             │
//! │   ┌─────────────────────────────┐                                   │
//! │   │         FIFO Queue          │ ← max_queue_depth                 │
//! │   │  [msg1] [msg2] [msg3] ...   │                                   │
//! │   └──────────────┬──────────────┘                                   │
//! │                  │                                                  │
//! │                  │ ← Timer fires at constant rate                   │
//! │                  ▼                                                  │
//! │   ┌────────────────────────────────┐                                │
//! │   │           tick()                │                                │
//! │   │   Queue has message? → Send it │                                │
//! │   │   Queue empty & cover? → Cover │                                │
//! │   └────────────────────────────────┘                                │
//! │                                                                     │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use botho::network::privacy::transmitter::{
//!     ConstantRateConfig, ConstantRateTransmitter, OutgoingMessage,
//! };
//! use std::time::Duration;
//! use tokio::time::interval;
//!
//! // Create transmitter with default config (2 msg/sec, cover traffic enabled)
//! let mut transmitter = ConstantRateTransmitter::new(ConstantRateConfig::default());
//!
//! // Enqueue a message
//! let msg = OutgoingMessage::Transaction(tx_data);
//! transmitter.enqueue(msg);
//!
//! // In async context, run the tick loop
//! let tick_interval = Duration::from_secs_f64(1.0 / transmitter.config().messages_per_second);
//! let mut timer = interval(tick_interval);
//!
//! loop {
//!     timer.tick().await;
//!     if let Some(message) = transmitter.tick() {
//!         // Send message through circuit
//!     }
//! }
//! ```
//!
//! # Security Properties
//!
//! - Fixed transmission rate prevents traffic timing analysis
//! - Cover traffic makes real messages indistinguishable from noise
//! - Queue depth limit prevents memory exhaustion attacks
//! - FIFO ordering preserves transaction submission order

use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};

/// Default messages per second (1 message every 500ms).
pub const DEFAULT_MESSAGES_PER_SECOND: f64 = 2.0;

/// Default maximum queue depth.
pub const DEFAULT_MAX_QUEUE_DEPTH: usize = 100;

/// Configuration for the constant-rate transmitter.
///
/// # Example
///
/// ```
/// use botho::network::privacy::transmitter::ConstantRateConfig;
///
/// // Default: 2 msg/sec, cover traffic enabled, 100 max queue
/// let default_config = ConstantRateConfig::default();
///
/// // Custom: faster rate, no cover traffic
/// let custom_config = ConstantRateConfig {
///     messages_per_second: 5.0,
///     cover_traffic: false,
///     max_queue_depth: 50,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstantRateConfig {
    /// Target messages per second (default: 2.0 = 500ms interval).
    ///
    /// Higher values provide faster message delivery but increase bandwidth.
    /// Lower values reduce bandwidth but may cause queue buildup.
    pub messages_per_second: f64,

    /// Generate cover traffic when queue is empty.
    ///
    /// When enabled, the transmitter sends cover messages at the same rate
    /// as real messages, making traffic patterns uniform regardless of
    /// user activity.
    pub cover_traffic: bool,

    /// Maximum queue depth before dropping old messages.
    ///
    /// When the queue reaches this limit, the oldest messages are dropped
    /// to make room for new ones. This prevents memory exhaustion but may
    /// cause transaction loss under heavy load.
    pub max_queue_depth: usize,
}

impl Default for ConstantRateConfig {
    fn default() -> Self {
        Self {
            messages_per_second: DEFAULT_MESSAGES_PER_SECOND,
            cover_traffic: true,
            max_queue_depth: DEFAULT_MAX_QUEUE_DEPTH,
        }
    }
}

impl ConstantRateConfig {
    /// Create a new configuration with the specified parameters.
    pub fn new(messages_per_second: f64, cover_traffic: bool, max_queue_depth: usize) -> Self {
        Self {
            messages_per_second,
            cover_traffic,
            max_queue_depth,
        }
    }

    /// Calculate the interval between messages.
    pub fn tick_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.messages_per_second)
    }
}

/// Message type marker for transmitter messages.
///
/// Distinguishes between real user messages and cover traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransmitterMessageType {
    /// Real transaction data.
    Transaction,
    /// Cover traffic (silently dropped by exit).
    Cover,
}

/// An outgoing message to be transmitted.
///
/// This wraps the actual message payload with metadata for transmission.
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    /// The message type.
    pub msg_type: TransmitterMessageType,
    /// The serialized message payload.
    pub payload: Vec<u8>,
}

impl OutgoingMessage {
    /// Create a new transaction message.
    pub fn transaction(payload: Vec<u8>) -> Self {
        Self {
            msg_type: TransmitterMessageType::Transaction,
            payload,
        }
    }

    /// Create a new cover message.
    pub fn cover(payload: Vec<u8>) -> Self {
        Self {
            msg_type: TransmitterMessageType::Cover,
            payload,
        }
    }

    /// Check if this is a cover message.
    pub fn is_cover(&self) -> bool {
        self.msg_type == TransmitterMessageType::Cover
    }
}

/// A queued message with metadata.
#[derive(Debug, Clone)]
struct QueuedMessage {
    /// The message to send.
    message: OutgoingMessage,
    /// When the message was queued.
    ///
    /// Reserved for future use: queue latency metrics, message expiry.
    #[allow(dead_code)]
    queued_at: Instant,
}

/// Metrics for the constant-rate transmitter.
#[derive(Debug, Default)]
pub struct TransmitterMetrics {
    /// Total messages sent (real + cover).
    pub messages_sent: AtomicU64,
    /// Real messages sent.
    pub real_messages_sent: AtomicU64,
    /// Cover messages sent.
    pub cover_messages_sent: AtomicU64,
    /// Messages dropped due to queue overflow.
    pub messages_dropped: AtomicU64,
    /// Ticks where no message was sent (queue empty, cover disabled).
    pub empty_ticks: AtomicU64,
}

impl TransmitterMetrics {
    /// Create new metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a snapshot of current metrics.
    pub fn snapshot(&self) -> TransmitterMetricsSnapshot {
        TransmitterMetricsSnapshot {
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            real_messages_sent: self.real_messages_sent.load(Ordering::Relaxed),
            cover_messages_sent: self.cover_messages_sent.load(Ordering::Relaxed),
            messages_dropped: self.messages_dropped.load(Ordering::Relaxed),
            empty_ticks: self.empty_ticks.load(Ordering::Relaxed),
        }
    }

    fn inc_messages_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_real_sent(&self) {
        self.real_messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_cover_sent(&self) {
        self.cover_messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_dropped(&self) {
        self.messages_dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_empty_ticks(&self) {
        self.empty_ticks.fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot of transmitter metrics (for RPC/monitoring).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransmitterMetricsSnapshot {
    /// Total messages sent (real + cover).
    pub messages_sent: u64,
    /// Real messages sent.
    pub real_messages_sent: u64,
    /// Cover messages sent.
    pub cover_messages_sent: u64,
    /// Messages dropped due to queue overflow.
    pub messages_dropped: u64,
    /// Ticks where no message was sent.
    pub empty_ticks: u64,
}

/// Constant-rate transmitter for traffic normalization.
///
/// Queues messages and sends them at a fixed rate, optionally generating
/// cover traffic to normalize traffic patterns.
#[derive(Debug)]
pub struct ConstantRateTransmitter {
    /// Configuration.
    config: ConstantRateConfig,
    /// FIFO message queue.
    queue: VecDeque<QueuedMessage>,
    /// Last send time (for rate limiting).
    last_send: Option<Instant>,
    /// Metrics.
    metrics: TransmitterMetrics,
}

impl ConstantRateTransmitter {
    /// Create a new transmitter with the given configuration.
    pub fn new(config: ConstantRateConfig) -> Self {
        Self {
            config,
            queue: VecDeque::new(),
            last_send: None,
            metrics: TransmitterMetrics::new(),
        }
    }

    /// Get the transmitter's configuration.
    pub fn config(&self) -> &ConstantRateConfig {
        &self.config
    }

    /// Get the transmitter's metrics.
    pub fn metrics(&self) -> &TransmitterMetrics {
        &self.metrics
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue.len()
    }

    /// Check if the queue is empty.
    pub fn is_queue_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Add a message to the queue.
    ///
    /// If the queue is at capacity, the oldest message is dropped to make room.
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to enqueue.
    pub fn enqueue(&mut self, msg: OutgoingMessage) {
        // If queue is full, drop oldest message
        if self.queue.len() >= self.config.max_queue_depth {
            self.queue.pop_front();
            self.metrics.inc_dropped();
        }

        self.queue.push_back(QueuedMessage {
            message: msg,
            queued_at: Instant::now(),
        });
    }

    /// Timer callback - returns the next message to send.
    ///
    /// This method should be called on every timer tick. It returns:
    /// - `Some(message)` if there's a message to send (real or cover)
    /// - `None` if no message should be sent (queue empty and cover disabled)
    ///
    /// # Rate Limiting
    ///
    /// The transmitter tracks the last send time and only returns a message
    /// if enough time has passed based on the configured rate. If called too
    /// soon, it returns `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tokio::time::{interval, Duration};
    ///
    /// let mut transmitter = ConstantRateTransmitter::new(config);
    /// let tick_interval = transmitter.config().tick_interval();
    /// let mut timer = interval(tick_interval);
    ///
    /// loop {
    ///     timer.tick().await;
    ///     if let Some(msg) = transmitter.tick() {
    ///         send_via_circuit(msg).await;
    ///     }
    /// }
    /// ```
    pub fn tick(&mut self) -> Option<OutgoingMessage> {
        let now = Instant::now();
        let interval = self.config.tick_interval();

        // Check if enough time has passed since last send
        if let Some(last) = self.last_send {
            if now.duration_since(last) < interval {
                return None;
            }
        }

        self.last_send = Some(now);

        // Try to send a real message from the queue
        if let Some(queued) = self.queue.pop_front() {
            self.metrics.inc_messages_sent();
            self.metrics.inc_real_sent();
            return Some(queued.message);
        }

        // Queue is empty - send cover traffic if enabled
        if self.config.cover_traffic {
            let cover = generate_cover_message();
            self.metrics.inc_messages_sent();
            self.metrics.inc_cover_sent();
            return Some(cover);
        }

        // No message to send
        self.metrics.inc_empty_ticks();
        None
    }

    /// Reset the transmitter state (for testing).
    #[cfg(test)]
    fn reset(&mut self) {
        self.queue.clear();
        self.last_send = None;
    }
}

impl Default for ConstantRateTransmitter {
    fn default() -> Self {
        Self::new(ConstantRateConfig::default())
    }
}

/// Generate a cover message that looks like a typical transaction.
///
/// Cover messages have random payloads sized to match the typical
/// transaction size distribution, making them indistinguishable from
/// real messages to observers.
fn generate_cover_message() -> OutgoingMessage {
    let mut rng = rand::thread_rng();

    // Match typical transaction size distribution (200-600 bytes)
    let size = rng.gen_range(200..600);
    let mut payload = vec![0u8; size];
    rng.fill_bytes(&mut payload);

    OutgoingMessage::cover(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    #[test]
    fn test_config_default() {
        let config = ConstantRateConfig::default();

        assert_eq!(config.messages_per_second, 2.0);
        assert!(config.cover_traffic);
        assert_eq!(config.max_queue_depth, 100);
    }

    #[test]
    fn test_config_tick_interval() {
        let config = ConstantRateConfig::default();
        let interval = config.tick_interval();

        // 2 msg/sec = 500ms interval
        assert_eq!(interval, Duration::from_millis(500));

        let fast_config = ConstantRateConfig::new(10.0, true, 100);
        let fast_interval = fast_config.tick_interval();

        // 10 msg/sec = 100ms interval
        assert_eq!(fast_interval, Duration::from_millis(100));
    }

    #[test]
    fn test_transmitter_creation() {
        let transmitter = ConstantRateTransmitter::default();

        assert!(transmitter.is_queue_empty());
        assert_eq!(transmitter.queue_depth(), 0);
    }

    #[test]
    fn test_enqueue_basic() {
        let mut transmitter = ConstantRateTransmitter::default();

        let msg = OutgoingMessage::transaction(vec![1, 2, 3]);
        transmitter.enqueue(msg);

        assert!(!transmitter.is_queue_empty());
        assert_eq!(transmitter.queue_depth(), 1);
    }

    #[test]
    fn test_enqueue_fifo_order() {
        let mut transmitter = ConstantRateTransmitter::default();

        transmitter.enqueue(OutgoingMessage::transaction(vec![1]));
        transmitter.enqueue(OutgoingMessage::transaction(vec![2]));
        transmitter.enqueue(OutgoingMessage::transaction(vec![3]));

        // First tick should return first message
        let msg1 = transmitter.tick().unwrap();
        assert_eq!(msg1.payload, vec![1]);

        // Need to wait for rate limit
        thread::sleep(Duration::from_millis(550));

        let msg2 = transmitter.tick().unwrap();
        assert_eq!(msg2.payload, vec![2]);
    }

    #[test]
    fn test_queue_overflow_drops_oldest() {
        let config = ConstantRateConfig::new(2.0, false, 3);
        let mut transmitter = ConstantRateTransmitter::new(config);

        transmitter.enqueue(OutgoingMessage::transaction(vec![1]));
        transmitter.enqueue(OutgoingMessage::transaction(vec![2]));
        transmitter.enqueue(OutgoingMessage::transaction(vec![3]));

        assert_eq!(transmitter.queue_depth(), 3);
        assert_eq!(transmitter.metrics().snapshot().messages_dropped, 0);

        // This should drop message [1]
        transmitter.enqueue(OutgoingMessage::transaction(vec![4]));

        assert_eq!(transmitter.queue_depth(), 3);
        assert_eq!(transmitter.metrics().snapshot().messages_dropped, 1);

        // Verify [1] was dropped - first message should be [2]
        let msg = transmitter.tick().unwrap();
        assert_eq!(msg.payload, vec![2]);
    }

    #[test]
    fn test_tick_rate_limiting() {
        let mut transmitter = ConstantRateTransmitter::default();

        transmitter.enqueue(OutgoingMessage::transaction(vec![1]));
        transmitter.enqueue(OutgoingMessage::transaction(vec![2]));

        // First tick should return message
        let msg1 = transmitter.tick();
        assert!(msg1.is_some());

        // Immediate second tick should return None (rate limited)
        let msg2 = transmitter.tick();
        assert!(msg2.is_none());

        // After waiting, should return message
        thread::sleep(Duration::from_millis(550));
        let msg3 = transmitter.tick();
        assert!(msg3.is_some());
    }

    #[test]
    fn test_cover_traffic_enabled() {
        let config = ConstantRateConfig::new(2.0, true, 100);
        let mut transmitter = ConstantRateTransmitter::new(config);

        // Queue is empty, should get cover traffic
        let msg = transmitter.tick();
        assert!(msg.is_some());
        assert!(msg.unwrap().is_cover());

        let snapshot = transmitter.metrics().snapshot();
        assert_eq!(snapshot.cover_messages_sent, 1);
        assert_eq!(snapshot.real_messages_sent, 0);
    }

    #[test]
    fn test_cover_traffic_disabled() {
        let config = ConstantRateConfig::new(2.0, false, 100);
        let mut transmitter = ConstantRateTransmitter::new(config);

        // Queue is empty, no cover traffic, should get None
        let msg = transmitter.tick();
        assert!(msg.is_none());

        let snapshot = transmitter.metrics().snapshot();
        assert_eq!(snapshot.empty_ticks, 1);
        assert_eq!(snapshot.cover_messages_sent, 0);
    }

    #[test]
    fn test_real_message_preferred_over_cover() {
        let config = ConstantRateConfig::new(2.0, true, 100);
        let mut transmitter = ConstantRateTransmitter::new(config);

        // Add a real message
        transmitter.enqueue(OutgoingMessage::transaction(vec![42]));

        // Should get real message, not cover
        let msg = transmitter.tick();
        assert!(msg.is_some());
        assert!(!msg.as_ref().unwrap().is_cover());
        assert_eq!(msg.unwrap().payload, vec![42]);

        let snapshot = transmitter.metrics().snapshot();
        assert_eq!(snapshot.real_messages_sent, 1);
        assert_eq!(snapshot.cover_messages_sent, 0);
    }

    #[test]
    fn test_cover_message_size_distribution() {
        // Generate several cover messages and verify size is in expected range
        for _ in 0..100 {
            let cover = generate_cover_message();
            assert!(cover.payload.len() >= 200);
            assert!(cover.payload.len() < 600);
            assert!(cover.is_cover());
        }
    }

    #[test]
    fn test_metrics_tracking() {
        let mut transmitter = ConstantRateTransmitter::default();

        // Send a real message
        transmitter.enqueue(OutgoingMessage::transaction(vec![1]));
        transmitter.tick();

        // Wait and send cover
        thread::sleep(Duration::from_millis(550));
        transmitter.tick();

        let snapshot = transmitter.metrics().snapshot();
        assert_eq!(snapshot.messages_sent, 2);
        assert_eq!(snapshot.real_messages_sent, 1);
        assert_eq!(snapshot.cover_messages_sent, 1);
    }

    #[test]
    fn test_outgoing_message_types() {
        let tx_msg = OutgoingMessage::transaction(vec![1, 2, 3]);
        assert!(!tx_msg.is_cover());
        assert_eq!(tx_msg.msg_type, TransmitterMessageType::Transaction);

        let cover_msg = OutgoingMessage::cover(vec![4, 5, 6]);
        assert!(cover_msg.is_cover());
        assert_eq!(cover_msg.msg_type, TransmitterMessageType::Cover);
    }

    #[test]
    fn test_constant_rate_regardless_of_input() {
        // This test verifies the core property: message rate is constant
        // regardless of how fast messages are enqueued.
        let config = ConstantRateConfig::new(10.0, false, 100); // 100ms interval
        let mut transmitter = ConstantRateTransmitter::new(config);

        // Enqueue many messages rapidly
        for i in 0..50 {
            transmitter.enqueue(OutgoingMessage::transaction(vec![i as u8]));
        }

        // Even with 50 messages queued, rate should be limited
        let mut sent_count = 0;
        let start = Instant::now();

        // Try to tick 5 times immediately
        for _ in 0..5 {
            if transmitter.tick().is_some() {
                sent_count += 1;
            }
        }

        // Only 1 should have been sent (rate limited)
        assert_eq!(sent_count, 1);

        // Wait and try again
        thread::sleep(Duration::from_millis(100));

        if transmitter.tick().is_some() {
            sent_count += 1;
        }

        assert_eq!(sent_count, 2);

        // Verify time-based rate limiting
        let elapsed = start.elapsed();
        // Should take at least ~100ms to send 2 messages at 10/sec rate
        assert!(elapsed >= Duration::from_millis(90));
    }

    #[test]
    fn test_queue_depth_with_mixed_operations() {
        let config = ConstantRateConfig::new(2.0, false, 5);
        let mut transmitter = ConstantRateTransmitter::new(config);

        // Fill queue
        for i in 0..5 {
            transmitter.enqueue(OutgoingMessage::transaction(vec![i]));
        }
        assert_eq!(transmitter.queue_depth(), 5);

        // Dequeue one
        transmitter.tick();
        assert_eq!(transmitter.queue_depth(), 4);

        // Wait for rate limit
        thread::sleep(Duration::from_millis(550));

        // Add more (within limit)
        transmitter.enqueue(OutgoingMessage::transaction(vec![5]));
        assert_eq!(transmitter.queue_depth(), 5);

        // Add one more (should drop oldest)
        transmitter.enqueue(OutgoingMessage::transaction(vec![6]));
        assert_eq!(transmitter.queue_depth(), 5);
        assert_eq!(transmitter.metrics().snapshot().messages_dropped, 1);
    }
}
