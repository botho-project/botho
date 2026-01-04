// Copyright (c) 2024 Botho Foundation

//! Message padding for traffic analysis resistance.
//!
//! This module implements Phase 2.1 of the traffic privacy roadmap: padding
//! messages to fixed bucket sizes to prevent size-based traffic analysis.
//!
//! # Design
//!
//! All messages are padded to one of five standard bucket sizes:
//! - 512 bytes: Tiny messages (pings, acks)
//! - 2,048 bytes: Small messages (typical transactions)
//! - 8,192 bytes: Medium messages (multi-input transactions)
//! - 32,768 bytes: Large messages (block headers)
//! - 131,072 bytes: XLarge messages (block bodies, 128 KB)
//!
//! # Message Format
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │ Length (2 bytes, little-endian) │ Payload │ Padding │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! - Length: Original payload size as u16 (max 65,535 bytes)
//! - Payload: The original message bytes
//! - Padding: Random bytes filling to the bucket size
//!
//! # Security
//!
//! - Padding uses cryptographically random bytes (not zeros) to prevent
//!   distinguishing padded regions via compression or entropy analysis
//! - All messages of similar type have identical wire size
//! - Combined with onion encryption, observers cannot determine message content
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::padding::{pad_to_bucket, unpad, SIZE_BUCKETS};
//!
//! let message = b"Hello, world!";
//! let padded = pad_to_bucket(message);
//!
//! // Padded to smallest bucket (512 bytes)
//! assert_eq!(padded.len(), 512);
//!
//! // Unpad to recover original
//! let recovered = unpad(&padded).unwrap();
//! assert_eq!(recovered, message);
//! ```

use rand::RngCore;
use thiserror::Error;

/// Length header size in bytes (u16 little-endian).
pub const LENGTH_HEADER_SIZE: usize = 2;

/// Maximum payload size that can be represented by u16 length header.
pub const MAX_PAYLOAD_SIZE: usize = u16::MAX as usize;

/// Standard message size buckets in bytes.
///
/// These sizes are chosen to:
/// - Cover the range of typical botho message sizes
/// - Minimize wasted bandwidth while maximizing anonymity set
/// - Align with common MTU boundaries where practical
pub const SIZE_BUCKETS: [usize; 5] = [
    512,    // Tiny: pings, acks, small control messages
    2048,   // Small: typical single-input transactions
    8192,   // Medium: multi-input transactions, small blocks
    32768,  // Large: block headers, batched transactions
    131072, // XLarge: block bodies (128 KB)
];

/// Errors that can occur during padding/unpadding operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PaddingError {
    /// Padded message is too short to contain length header.
    #[error("padded message too short: got {0} bytes, need at least {LENGTH_HEADER_SIZE}")]
    TooShort(usize),

    /// Length header indicates more bytes than available.
    #[error("invalid length header: claims {claimed} bytes but only {available} available")]
    InvalidLength { claimed: usize, available: usize },

    /// Total message size is not a valid bucket size.
    #[error("invalid bucket size: {0} bytes is not a standard bucket")]
    InvalidBucket(usize),

    /// Payload exceeds maximum size representable by length header.
    #[error("payload too large: {0} bytes exceeds maximum {MAX_PAYLOAD_SIZE}")]
    PayloadTooLarge(usize),
}

/// Pad a message to the next bucket size.
///
/// The message is padded to the smallest bucket that can contain the payload
/// plus a 2-byte length header. Padding bytes are cryptographically random.
///
/// # Arguments
///
/// * `payload` - The original message bytes
///
/// # Returns
///
/// A new vector containing the padded message.
///
/// # Panics
///
/// Panics if the payload is larger than [`MAX_PAYLOAD_SIZE`] (65,535 bytes).
/// For payloads that may exceed this, use [`try_pad_to_bucket`] instead.
///
/// # Example
///
/// ```
/// use botho::network::privacy::padding::pad_to_bucket;
///
/// let msg = b"small message";
/// let padded = pad_to_bucket(msg);
/// assert_eq!(padded.len(), 512); // Smallest bucket
/// ```
pub fn pad_to_bucket(payload: &[u8]) -> Vec<u8> {
    try_pad_to_bucket(payload).expect("payload exceeds maximum size")
}

/// Try to pad a message to the next bucket size.
///
/// Like [`pad_to_bucket`], but returns an error instead of panicking for
/// oversized payloads.
///
/// # Errors
///
/// Returns [`PaddingError::PayloadTooLarge`] if payload exceeds 65,535 bytes.
pub fn try_pad_to_bucket(payload: &[u8]) -> Result<Vec<u8>, PaddingError> {
    if payload.len() > MAX_PAYLOAD_SIZE {
        return Err(PaddingError::PayloadTooLarge(payload.len()));
    }

    let needed = payload.len() + LENGTH_HEADER_SIZE;
    let bucket_size = select_bucket(needed);

    let mut padded = Vec::with_capacity(bucket_size);

    // Write length header (little-endian u16)
    let len_bytes = (payload.len() as u16).to_le_bytes();
    padded.extend_from_slice(&len_bytes);

    // Write payload
    padded.extend_from_slice(payload);

    // Fill remaining space with random bytes
    let padding_len = bucket_size - padded.len();
    if padding_len > 0 {
        let mut rng = rand::thread_rng();
        let start = padded.len();
        padded.resize(bucket_size, 0);
        rng.fill_bytes(&mut padded[start..]);
    }

    Ok(padded)
}

/// Select the smallest bucket that can hold the given size.
///
/// # Arguments
///
/// * `needed` - The minimum size needed (payload + header)
///
/// # Returns
///
/// The bucket size to use. If `needed` exceeds all buckets, returns the
/// largest bucket (messages will need to be split by caller).
fn select_bucket(needed: usize) -> usize {
    for &bucket in &SIZE_BUCKETS {
        if bucket >= needed {
            return bucket;
        }
    }
    // Fall back to largest bucket for oversized messages
    SIZE_BUCKETS[SIZE_BUCKETS.len() - 1]
}

/// Remove padding and extract the original payload.
///
/// # Arguments
///
/// * `padded` - The padded message
///
/// # Returns
///
/// A slice containing the original payload bytes.
///
/// # Errors
///
/// - [`PaddingError::TooShort`] if message is shorter than length header
/// - [`PaddingError::InvalidLength`] if length header exceeds available data
/// - [`PaddingError::InvalidBucket`] if total size is not a valid bucket
///
/// # Example
///
/// ```
/// use botho::network::privacy::padding::{pad_to_bucket, unpad};
///
/// let original = b"test message";
/// let padded = pad_to_bucket(original);
/// let recovered = unpad(&padded).unwrap();
/// assert_eq!(recovered, original);
/// ```
pub fn unpad(padded: &[u8]) -> Result<&[u8], PaddingError> {
    // Validate minimum size
    if padded.len() < LENGTH_HEADER_SIZE {
        return Err(PaddingError::TooShort(padded.len()));
    }

    // Validate bucket size
    if !is_valid_bucket(padded.len()) {
        return Err(PaddingError::InvalidBucket(padded.len()));
    }

    // Extract length header
    let len = u16::from_le_bytes([padded[0], padded[1]]) as usize;

    // Validate length is consistent with buffer
    let available = padded.len() - LENGTH_HEADER_SIZE;
    if len > available {
        return Err(PaddingError::InvalidLength {
            claimed: len,
            available,
        });
    }

    // Return slice to original payload
    Ok(&padded[LENGTH_HEADER_SIZE..LENGTH_HEADER_SIZE + len])
}

/// Check if a size is a valid bucket size.
#[inline]
pub fn is_valid_bucket(size: usize) -> bool {
    SIZE_BUCKETS.contains(&size)
}

/// Get the bucket size for a given payload.
///
/// Returns the bucket size that would be used for a payload of the given
/// length.
///
/// # Example
///
/// ```
/// use botho::network::privacy::padding::bucket_for_payload;
///
/// assert_eq!(bucket_for_payload(100), 512);
/// assert_eq!(bucket_for_payload(1000), 2048);
/// ```
pub fn bucket_for_payload(payload_len: usize) -> usize {
    select_bucket(payload_len + LENGTH_HEADER_SIZE)
}

/// Calculate padding overhead for a given payload size.
///
/// Returns the number of bytes that will be wasted as padding.
///
/// # Example
///
/// ```
/// use botho::network::privacy::padding::padding_overhead;
///
/// // 100-byte payload padded to 512 bytes = 410 bytes overhead
/// assert_eq!(padding_overhead(100), 410);
/// ```
pub fn padding_overhead(payload_len: usize) -> usize {
    let bucket = bucket_for_payload(payload_len);
    bucket - payload_len - LENGTH_HEADER_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pad_empty_payload() {
        let payload: &[u8] = &[];
        let padded = pad_to_bucket(payload);

        assert_eq!(padded.len(), SIZE_BUCKETS[0]); // Smallest bucket
        assert_eq!(padded[0], 0); // Length = 0
        assert_eq!(padded[1], 0);

        let recovered = unpad(&padded).unwrap();
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_pad_small_payload() {
        let payload = b"hello, world!";
        let padded = pad_to_bucket(payload);

        assert_eq!(padded.len(), SIZE_BUCKETS[0]); // 512 bytes

        let recovered = unpad(&padded).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn test_pad_exact_bucket_boundary() {
        // Payload that exactly fills smallest bucket minus header
        let payload_len = SIZE_BUCKETS[0] - LENGTH_HEADER_SIZE;
        let payload: Vec<u8> = (0..payload_len).map(|i| (i % 256) as u8).collect();

        let padded = pad_to_bucket(&payload);
        assert_eq!(padded.len(), SIZE_BUCKETS[0]);

        let recovered = unpad(&padded).unwrap();
        assert_eq!(recovered, payload.as_slice());
    }

    #[test]
    fn test_pad_just_over_bucket_boundary() {
        // Payload that just exceeds smallest bucket
        let payload_len = SIZE_BUCKETS[0] - LENGTH_HEADER_SIZE + 1;
        let payload: Vec<u8> = (0..payload_len).map(|i| (i % 256) as u8).collect();

        let padded = pad_to_bucket(&payload);
        assert_eq!(padded.len(), SIZE_BUCKETS[1]); // Bumps to next bucket

        let recovered = unpad(&padded).unwrap();
        assert_eq!(recovered, payload.as_slice());
    }

    #[test]
    fn test_all_bucket_sizes() {
        // Test each bucket with a payload sized to fit in that specific bucket
        // Note: XLarge bucket (131072) can't be fully filled due to u16 length limit
        let test_cases = [
            (SIZE_BUCKETS[0] - LENGTH_HEADER_SIZE - 10, SIZE_BUCKETS[0]), // Tiny
            (SIZE_BUCKETS[1] - LENGTH_HEADER_SIZE - 10, SIZE_BUCKETS[1]), // Small
            (SIZE_BUCKETS[2] - LENGTH_HEADER_SIZE - 10, SIZE_BUCKETS[2]), // Medium
            (SIZE_BUCKETS[3] - LENGTH_HEADER_SIZE - 10, SIZE_BUCKETS[3]), // Large
            (MAX_PAYLOAD_SIZE, SIZE_BUCKETS[4]),                          // XLarge (max u16)
        ];

        for (payload_len, expected_bucket) in test_cases {
            let payload: Vec<u8> = (0..payload_len).map(|i| (i % 256) as u8).collect();

            let padded = pad_to_bucket(&payload);
            assert_eq!(
                padded.len(),
                expected_bucket,
                "payload len {} should use bucket {}",
                payload_len,
                expected_bucket
            );

            let recovered = unpad(&padded).unwrap();
            assert_eq!(recovered, payload.as_slice());
        }
    }

    #[test]
    fn test_padding_is_random() {
        let payload = b"test";

        let padded1 = pad_to_bucket(payload);
        let padded2 = pad_to_bucket(payload);

        // Header and payload should be identical
        assert_eq!(&padded1[..2 + payload.len()], &padded2[..2 + payload.len()]);

        // Padding should differ (with overwhelming probability)
        let padding1 = &padded1[2 + payload.len()..];
        let padding2 = &padded2[2 + payload.len()..];
        assert_ne!(padding1, padding2);
    }

    #[test]
    fn test_unpad_too_short() {
        let short = [0u8; 1];
        let result = unpad(&short);
        assert_eq!(result, Err(PaddingError::TooShort(1)));
    }

    #[test]
    fn test_unpad_invalid_bucket() {
        // 100 bytes is not a valid bucket
        let invalid: Vec<u8> = vec![0; 100];
        let result = unpad(&invalid);
        assert_eq!(result, Err(PaddingError::InvalidBucket(100)));
    }

    #[test]
    fn test_unpad_invalid_length() {
        // Create a bucket-sized buffer with invalid length header
        let mut bad = vec![0u8; SIZE_BUCKETS[0]];
        // Claim length is larger than available space
        let fake_len = (SIZE_BUCKETS[0] as u16).to_le_bytes();
        bad[0] = fake_len[0];
        bad[1] = fake_len[1];

        let result = unpad(&bad);
        assert!(matches!(result, Err(PaddingError::InvalidLength { .. })));
    }

    #[test]
    fn test_bucket_selection() {
        assert_eq!(select_bucket(1), 512);
        assert_eq!(select_bucket(512), 512);
        assert_eq!(select_bucket(513), 2048);
        assert_eq!(select_bucket(2048), 2048);
        assert_eq!(select_bucket(2049), 8192);
        assert_eq!(select_bucket(8192), 8192);
        assert_eq!(select_bucket(8193), 32768);
        assert_eq!(select_bucket(32768), 32768);
        assert_eq!(select_bucket(32769), 131072);
        assert_eq!(select_bucket(131072), 131072);
        // Oversized falls back to largest
        assert_eq!(select_bucket(200000), 131072);
    }

    #[test]
    fn test_is_valid_bucket() {
        assert!(is_valid_bucket(512));
        assert!(is_valid_bucket(2048));
        assert!(is_valid_bucket(8192));
        assert!(is_valid_bucket(32768));
        assert!(is_valid_bucket(131072));

        assert!(!is_valid_bucket(0));
        assert!(!is_valid_bucket(100));
        assert!(!is_valid_bucket(1000));
        assert!(!is_valid_bucket(1024)); // Common size but not a bucket
    }

    #[test]
    fn test_bucket_for_payload() {
        assert_eq!(bucket_for_payload(0), 512);
        assert_eq!(bucket_for_payload(100), 512);
        assert_eq!(bucket_for_payload(510), 512);
        assert_eq!(bucket_for_payload(511), 2048); // 511 + 2 = 513 > 512
        assert_eq!(bucket_for_payload(2000), 2048);
    }

    #[test]
    fn test_padding_overhead() {
        // 100 byte payload -> 512 bucket -> 512 - 100 - 2 = 410 overhead
        assert_eq!(padding_overhead(100), 410);

        // Payload that exactly fills bucket
        assert_eq!(padding_overhead(510), 0);
    }

    #[test]
    fn test_payload_too_large() {
        let huge: Vec<u8> = vec![0; MAX_PAYLOAD_SIZE + 1];
        let result = try_pad_to_bucket(&huge);
        assert!(matches!(result, Err(PaddingError::PayloadTooLarge(_))));
    }

    #[test]
    fn test_roundtrip_various_sizes() {
        let test_sizes = [0, 1, 10, 100, 500, 510, 511, 1000, 2000, 5000, 10000, 30000];

        for size in test_sizes {
            let payload: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let padded = pad_to_bucket(&payload);
            let recovered = unpad(&padded).unwrap();
            assert_eq!(
                recovered,
                payload.as_slice(),
                "roundtrip failed for size {}",
                size
            );
        }
    }

    #[test]
    fn test_length_header_endianness() {
        // Test that we correctly use little-endian
        let payload: Vec<u8> = vec![0xAB; 0x0102]; // 258 bytes
        let padded = pad_to_bucket(&payload);

        // Little-endian: 0x0102 -> [0x02, 0x01]
        assert_eq!(padded[0], 0x02);
        assert_eq!(padded[1], 0x01);
    }
}
