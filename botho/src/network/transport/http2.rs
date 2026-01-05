// Copyright (c) 2024 Botho Foundation

//! Optional HTTP/2 framing for maximum protocol obfuscation.
//!
//! This module provides HTTP/2 frame wrapping for TLS tunnel traffic.
//! When enabled, the traffic not only looks like HTTPS but also follows
//! the HTTP/2 framing structure, making it indistinguishable from
//! legitimate HTTP/2 traffic to deep packet inspection.
//!
//! # Overview
//!
//! HTTP/2 frames are structured as:
//! ```text
//! +-----------------------------------------------+
//! |                 Length (24)                   |
//! +---------------+---------------+---------------+
//! |   Type (8)    |   Flags (8)   |
//! +-+-------------+---------------+-------------------------------+
//! |R|                 Stream Identifier (31)                      |
//! +=+=============================================================+
//! |                   Frame Payload (0...)                      ...
//! +---------------------------------------------------------------+
//! ```
//!
//! We use DATA frames (type 0x0) to wrap botho protocol data.
//!
//! # Security Properties
//!
//! - Traffic appears as legitimate HTTP/2 data transfer
//! - Frame structure matches RFC 7540 specification
//! - Stream IDs cycle through valid ranges
//! - Padding is used to normalize frame sizes
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::http2::{Http2Wrapper, Http2WrapperConfig};
//!
//! let config = Http2WrapperConfig::default();
//! let wrapper = Http2Wrapper::new(config);
//!
//! // Wrap data in HTTP/2 DATA frame
//! let data = b"hello world";
//! let frame = wrapper.wrap(data);
//!
//! // Unwrap to get original data
//! let original = wrapper.unwrap(&frame).unwrap();
//! assert_eq!(original, data);
//! ```
//!
//! # References
//!
//! - HTTP/2 Frame Format: RFC 7540 Section 4
//! - DATA Frame: RFC 7540 Section 6.1

use std::fmt;

/// Maximum HTTP/2 frame payload size (16KB default, 16MB max per RFC 7540).
pub const MAX_FRAME_SIZE: usize = 16384;

/// HTTP/2 frame header size in bytes.
pub const FRAME_HEADER_SIZE: usize = 9;

/// HTTP/2 DATA frame type.
pub const FRAME_TYPE_DATA: u8 = 0x0;

/// HTTP/2 SETTINGS frame type.
pub const FRAME_TYPE_SETTINGS: u8 = 0x4;

/// HTTP/2 WINDOW_UPDATE frame type.
pub const FRAME_TYPE_WINDOW_UPDATE: u8 = 0x8;

/// HTTP/2 frame flags.
pub mod flags {
    /// END_STREAM flag (indicates final frame of a stream).
    pub const END_STREAM: u8 = 0x1;

    /// PADDED flag (indicates padding is present).
    pub const PADDED: u8 = 0x8;

    /// END_HEADERS flag (for HEADERS frames).
    pub const END_HEADERS: u8 = 0x4;
}

/// Configuration for HTTP/2 frame wrapping.
#[derive(Debug, Clone)]
pub struct Http2WrapperConfig {
    /// Use padding to normalize frame sizes.
    pub use_padding: bool,

    /// Target frame size when padding (must be <= MAX_FRAME_SIZE).
    pub target_frame_size: usize,

    /// Initial stream ID for DATA frames.
    pub initial_stream_id: u32,
}

impl Default for Http2WrapperConfig {
    fn default() -> Self {
        Self {
            use_padding: true,
            target_frame_size: MAX_FRAME_SIZE,
            initial_stream_id: 1, // Client streams use odd numbers
        }
    }
}

impl Http2WrapperConfig {
    /// Create config optimized for obfuscation.
    pub fn high_obfuscation() -> Self {
        Self {
            use_padding: true,
            target_frame_size: MAX_FRAME_SIZE,
            initial_stream_id: 1,
        }
    }

    /// Create config optimized for performance (minimal overhead).
    pub fn low_overhead() -> Self {
        Self {
            use_padding: false,
            target_frame_size: MAX_FRAME_SIZE,
            initial_stream_id: 1,
        }
    }
}

/// HTTP/2 frame wrapper for protocol obfuscation.
///
/// Wraps arbitrary data in HTTP/2 DATA frames, optionally adding
/// padding to normalize frame sizes and prevent traffic analysis.
pub struct Http2Wrapper {
    config: Http2WrapperConfig,

    /// Current stream ID (incremented by 2 for each new stream).
    current_stream_id: u32,

    /// Decoder state for unwrapping frames.
    decoder_buffer: Vec<u8>,
}

impl Http2Wrapper {
    /// Create a new HTTP/2 wrapper with the given configuration.
    pub fn new(config: Http2WrapperConfig) -> Self {
        Self {
            current_stream_id: config.initial_stream_id,
            config,
            decoder_buffer: Vec::new(),
        }
    }

    /// Wrap data as an HTTP/2 DATA frame.
    ///
    /// Returns the complete frame including header.
    pub fn wrap(&mut self, data: &[u8]) -> Vec<u8> {
        // Calculate padding if enabled
        let (padding_length, use_padding_flag) = if self.config.use_padding {
            self.calculate_padding(data.len())
        } else {
            (0, false)
        };

        // Total payload size: [pad_length (1)] + data + padding
        let payload_size = if use_padding_flag {
            1 + data.len() + padding_length
        } else {
            data.len()
        };

        // Build frame
        let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE + payload_size);

        // Length (24 bits)
        frame.push(((payload_size >> 16) & 0xFF) as u8);
        frame.push(((payload_size >> 8) & 0xFF) as u8);
        frame.push((payload_size & 0xFF) as u8);

        // Type (8 bits) - DATA frame
        frame.push(FRAME_TYPE_DATA);

        // Flags (8 bits)
        let flags = if use_padding_flag { flags::PADDED } else { 0 };
        frame.push(flags);

        // Stream ID (31 bits, first bit reserved)
        let stream_id = self.current_stream_id;
        frame.push(((stream_id >> 24) & 0x7F) as u8); // Mask reserved bit
        frame.push(((stream_id >> 16) & 0xFF) as u8);
        frame.push(((stream_id >> 8) & 0xFF) as u8);
        frame.push((stream_id & 0xFF) as u8);

        // Payload
        if use_padding_flag {
            // Pad length (1 byte)
            frame.push(padding_length as u8);
        }

        // Data
        frame.extend_from_slice(data);

        // Padding (random bytes for obfuscation)
        if use_padding_flag && padding_length > 0 {
            let padding: Vec<u8> = (0..padding_length).map(|_| rand::random()).collect();
            frame.extend_from_slice(&padding);
        }

        frame
    }

    /// Unwrap an HTTP/2 DATA frame to get the original data.
    ///
    /// Returns an error if the frame is malformed or not a DATA frame.
    pub fn unwrap(&self, frame: &[u8]) -> Result<Vec<u8>, Http2FrameError> {
        if frame.len() < FRAME_HEADER_SIZE {
            return Err(Http2FrameError::FrameTooShort);
        }

        // Parse header
        let length = ((frame[0] as usize) << 16) | ((frame[1] as usize) << 8) | (frame[2] as usize);

        let frame_type = frame[3];
        let flags = frame[4];

        // We only handle DATA frames
        if frame_type != FRAME_TYPE_DATA {
            return Err(Http2FrameError::NotDataFrame(frame_type));
        }

        // Verify length
        let expected_len = FRAME_HEADER_SIZE + length;
        if frame.len() < expected_len {
            return Err(Http2FrameError::IncompleteFrame {
                expected: expected_len,
                actual: frame.len(),
            });
        }

        // Extract payload
        let payload = &frame[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + length];

        // Handle padding
        if flags & flags::PADDED != 0 {
            if payload.is_empty() {
                return Err(Http2FrameError::InvalidPadding);
            }

            let pad_length = payload[0] as usize;

            // Validate padding length
            if pad_length >= payload.len() {
                return Err(Http2FrameError::InvalidPadding);
            }

            // Data is between pad_length byte and padding
            let data_start = 1;
            let data_end = payload.len() - pad_length;

            Ok(payload[data_start..data_end].to_vec())
        } else {
            Ok(payload.to_vec())
        }
    }

    /// Calculate padding to reach target frame size.
    fn calculate_padding(&self, data_len: usize) -> (usize, bool) {
        // Overhead: header (9) + pad_length byte (1)
        let overhead = FRAME_HEADER_SIZE + 1;
        let current_size = overhead + data_len;

        if current_size >= self.config.target_frame_size {
            // Data is already at or above target, no padding
            (0, false)
        } else {
            let padding = self.config.target_frame_size - current_size;
            // HTTP/2 padding is limited to 255 bytes
            let capped_padding = padding.min(255);
            (capped_padding, true)
        }
    }

    /// Feed data into the decoder buffer for streaming unwrap.
    pub fn feed(&mut self, data: &[u8]) {
        self.decoder_buffer.extend_from_slice(data);
    }

    /// Try to extract the next complete frame from the decoder buffer.
    ///
    /// Returns `Some(data)` if a complete frame was extracted,
    /// `None` if more data is needed.
    pub fn try_decode_next(&mut self) -> Result<Option<Vec<u8>>, Http2FrameError> {
        if self.decoder_buffer.len() < FRAME_HEADER_SIZE {
            return Ok(None);
        }

        // Parse length from header
        let length = ((self.decoder_buffer[0] as usize) << 16)
            | ((self.decoder_buffer[1] as usize) << 8)
            | (self.decoder_buffer[2] as usize);

        let total_frame_size = FRAME_HEADER_SIZE + length;

        if self.decoder_buffer.len() < total_frame_size {
            return Ok(None);
        }

        // Extract complete frame
        let frame: Vec<u8> = self.decoder_buffer.drain(..total_frame_size).collect();
        let data = self.unwrap(&frame)?;
        Ok(Some(data))
    }

    /// Clear the decoder buffer.
    pub fn clear_buffer(&mut self) {
        self.decoder_buffer.clear();
    }

    /// Advance to the next stream ID.
    ///
    /// HTTP/2 client streams use odd numbers, server streams use even.
    pub fn next_stream(&mut self) {
        self.current_stream_id = self.current_stream_id.wrapping_add(2);
        // Keep stream ID in valid range (avoid 0 and stay within 31 bits)
        if self.current_stream_id == 0 || self.current_stream_id > 0x7FFFFFFF {
            self.current_stream_id = self.config.initial_stream_id;
        }
    }

    /// Get the current stream ID.
    pub fn current_stream_id(&self) -> u32 {
        self.current_stream_id
    }

    /// Generate a SETTINGS frame (for connection preface).
    ///
    /// This should be sent at the start of an HTTP/2 connection.
    pub fn settings_frame(&self) -> Vec<u8> {
        // Empty SETTINGS frame (use defaults)
        let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE);

        // Length: 0
        frame.extend_from_slice(&[0, 0, 0]);
        // Type: SETTINGS
        frame.push(FRAME_TYPE_SETTINGS);
        // Flags: 0
        frame.push(0);
        // Stream ID: 0 (connection-level)
        frame.extend_from_slice(&[0, 0, 0, 0]);

        frame
    }

    /// Generate a SETTINGS ACK frame.
    pub fn settings_ack_frame(&self) -> Vec<u8> {
        let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE);

        // Length: 0
        frame.extend_from_slice(&[0, 0, 0]);
        // Type: SETTINGS
        frame.push(FRAME_TYPE_SETTINGS);
        // Flags: ACK (0x1)
        frame.push(0x1);
        // Stream ID: 0
        frame.extend_from_slice(&[0, 0, 0, 0]);

        frame
    }

    /// Generate a WINDOW_UPDATE frame.
    pub fn window_update_frame(&self, stream_id: u32, increment: u32) -> Vec<u8> {
        let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE + 4);

        // Length: 4
        frame.extend_from_slice(&[0, 0, 4]);
        // Type: WINDOW_UPDATE
        frame.push(FRAME_TYPE_WINDOW_UPDATE);
        // Flags: 0
        frame.push(0);
        // Stream ID
        frame.push(((stream_id >> 24) & 0x7F) as u8);
        frame.push(((stream_id >> 16) & 0xFF) as u8);
        frame.push(((stream_id >> 8) & 0xFF) as u8);
        frame.push((stream_id & 0xFF) as u8);
        // Window Size Increment (31 bits)
        frame.push(((increment >> 24) & 0x7F) as u8);
        frame.push(((increment >> 16) & 0xFF) as u8);
        frame.push(((increment >> 8) & 0xFF) as u8);
        frame.push((increment & 0xFF) as u8);

        frame
    }
}

impl Default for Http2Wrapper {
    fn default() -> Self {
        Self::new(Http2WrapperConfig::default())
    }
}

impl fmt::Debug for Http2Wrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Http2Wrapper")
            .field("config", &self.config)
            .field("current_stream_id", &self.current_stream_id)
            .field("decoder_buffer_len", &self.decoder_buffer.len())
            .finish()
    }
}

/// Errors that can occur during HTTP/2 frame operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Http2FrameError {
    /// Frame is too short to contain a valid header.
    FrameTooShort,

    /// Frame is not a DATA frame.
    NotDataFrame(u8),

    /// Frame is incomplete (more data needed).
    IncompleteFrame { expected: usize, actual: usize },

    /// Invalid padding in frame.
    InvalidPadding,

    /// Frame exceeds maximum size.
    FrameTooLarge(usize),
}

impl fmt::Display for Http2FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Http2FrameError::FrameTooShort => {
                write!(f, "frame too short to contain header")
            }
            Http2FrameError::NotDataFrame(t) => {
                write!(f, "expected DATA frame (0x0), got type 0x{:02x}", t)
            }
            Http2FrameError::IncompleteFrame { expected, actual } => {
                write!(
                    f,
                    "incomplete frame: expected {} bytes, got {}",
                    expected, actual
                )
            }
            Http2FrameError::InvalidPadding => {
                write!(f, "invalid padding in frame")
            }
            Http2FrameError::FrameTooLarge(size) => {
                write!(f, "frame size {} exceeds maximum", size)
            }
        }
    }
}

impl std::error::Error for Http2FrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_unwrap_basic() {
        let mut wrapper = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: false,
            ..Default::default()
        });

        let data = b"hello world";
        let frame = wrapper.wrap(data);
        let unwrapped = wrapper.unwrap(&frame).unwrap();

        assert_eq!(unwrapped, data);
    }

    #[test]
    fn test_wrap_unwrap_with_padding() {
        let mut wrapper = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: true,
            target_frame_size: 100,
            ..Default::default()
        });

        let data = b"hello";
        let frame = wrapper.wrap(data);

        // Frame should be padded to target size
        assert_eq!(frame.len(), 100);

        let unwrapped = wrapper.unwrap(&frame).unwrap();
        assert_eq!(unwrapped, data);
    }

    #[test]
    fn test_wrap_unwrap_empty() {
        let mut wrapper = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: false,
            ..Default::default()
        });

        let data: &[u8] = b"";
        let frame = wrapper.wrap(data);
        let unwrapped = wrapper.unwrap(&frame).unwrap();

        assert_eq!(unwrapped, data);
    }

    #[test]
    fn test_frame_structure() {
        let mut wrapper = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: false,
            initial_stream_id: 5,
            ..Default::default()
        });

        let data = b"test";
        let frame = wrapper.wrap(data);

        // Check header
        assert_eq!(frame.len(), FRAME_HEADER_SIZE + 4);

        // Length (24 bits)
        let length = ((frame[0] as usize) << 16) | ((frame[1] as usize) << 8) | (frame[2] as usize);
        assert_eq!(length, 4);

        // Type
        assert_eq!(frame[3], FRAME_TYPE_DATA);

        // Flags
        assert_eq!(frame[4], 0);

        // Stream ID (ignore reserved bit)
        let stream_id = ((frame[5] as u32 & 0x7F) << 24)
            | ((frame[6] as u32) << 16)
            | ((frame[7] as u32) << 8)
            | (frame[8] as u32);
        assert_eq!(stream_id, 5);
    }

    #[test]
    fn test_unwrap_short_frame() {
        let wrapper = Http2Wrapper::default();
        let frame = vec![0u8; 5]; // Too short
        assert!(matches!(
            wrapper.unwrap(&frame),
            Err(Http2FrameError::FrameTooShort)
        ));
    }

    #[test]
    fn test_unwrap_incomplete_frame() {
        let wrapper = Http2Wrapper::default();
        // Header claiming 100 bytes but only providing header
        let mut frame = vec![0, 0, 100]; // Length: 100
        frame.push(FRAME_TYPE_DATA);
        frame.extend_from_slice(&[0, 0, 0, 0, 1]); // Flags + Stream ID

        assert!(matches!(
            wrapper.unwrap(&frame),
            Err(Http2FrameError::IncompleteFrame { .. })
        ));
    }

    #[test]
    fn test_unwrap_wrong_type() {
        let wrapper = Http2Wrapper::default();
        // SETTINGS frame instead of DATA
        let mut frame = vec![0, 0, 0]; // Length: 0
        frame.push(FRAME_TYPE_SETTINGS);
        frame.extend_from_slice(&[0, 0, 0, 0, 0]); // Flags + Stream ID

        assert!(matches!(
            wrapper.unwrap(&frame),
            Err(Http2FrameError::NotDataFrame(FRAME_TYPE_SETTINGS))
        ));
    }

    #[test]
    fn test_streaming_decode() {
        let mut encoder = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: false,
            ..Default::default()
        });
        let mut decoder = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: false,
            ..Default::default()
        });

        // Encode two frames
        let frame1 = encoder.wrap(b"hello");
        let frame2 = encoder.wrap(b"world");

        // Feed partial data
        decoder.feed(&frame1[..5]);
        assert!(decoder.try_decode_next().unwrap().is_none());

        // Feed rest of first frame
        decoder.feed(&frame1[5..]);
        let data1 = decoder.try_decode_next().unwrap().unwrap();
        assert_eq!(data1, b"hello");

        // Feed second frame
        decoder.feed(&frame2);
        let data2 = decoder.try_decode_next().unwrap().unwrap();
        assert_eq!(data2, b"world");
    }

    #[test]
    fn test_next_stream() {
        let mut wrapper = Http2Wrapper::new(Http2WrapperConfig {
            initial_stream_id: 1,
            ..Default::default()
        });

        assert_eq!(wrapper.current_stream_id(), 1);
        wrapper.next_stream();
        assert_eq!(wrapper.current_stream_id(), 3);
        wrapper.next_stream();
        assert_eq!(wrapper.current_stream_id(), 5);
    }

    #[test]
    fn test_settings_frame() {
        let wrapper = Http2Wrapper::default();
        let frame = wrapper.settings_frame();

        assert_eq!(frame.len(), FRAME_HEADER_SIZE);
        assert_eq!(frame[3], FRAME_TYPE_SETTINGS);
        assert_eq!(frame[4], 0); // No flags
    }

    #[test]
    fn test_settings_ack_frame() {
        let wrapper = Http2Wrapper::default();
        let frame = wrapper.settings_ack_frame();

        assert_eq!(frame.len(), FRAME_HEADER_SIZE);
        assert_eq!(frame[3], FRAME_TYPE_SETTINGS);
        assert_eq!(frame[4], 0x1); // ACK flag
    }

    #[test]
    fn test_window_update_frame() {
        let wrapper = Http2Wrapper::default();
        let frame = wrapper.window_update_frame(1, 65535);

        assert_eq!(frame.len(), FRAME_HEADER_SIZE + 4);
        assert_eq!(frame[3], FRAME_TYPE_WINDOW_UPDATE);

        // Check increment value
        let increment = ((frame[9] as u32 & 0x7F) << 24)
            | ((frame[10] as u32) << 16)
            | ((frame[11] as u32) << 8)
            | (frame[12] as u32);
        assert_eq!(increment, 65535);
    }

    #[test]
    fn test_error_display() {
        let err = Http2FrameError::FrameTooShort;
        assert!(!err.to_string().is_empty());

        let err = Http2FrameError::NotDataFrame(0x04);
        assert!(err.to_string().contains("0x04"));

        let err = Http2FrameError::IncompleteFrame {
            expected: 100,
            actual: 50,
        };
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("50"));
    }

    #[test]
    fn test_config_presets() {
        let high = Http2WrapperConfig::high_obfuscation();
        assert!(high.use_padding);

        let low = Http2WrapperConfig::low_overhead();
        assert!(!low.use_padding);
    }

    #[test]
    fn test_large_data() {
        let mut wrapper = Http2Wrapper::new(Http2WrapperConfig {
            use_padding: false,
            ..Default::default()
        });

        // Test with data larger than typical
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let frame = wrapper.wrap(&data);
        let unwrapped = wrapper.unwrap(&frame).unwrap();

        assert_eq!(unwrapped, data);
    }
}
