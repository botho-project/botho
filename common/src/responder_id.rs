// Copyright (c) 2018-2022 The Botho Foundation

//! The Responder ID type

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::{
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};
use displaydoc::Display;
use bth_crypto_digestible::Digestible;
use prost::{
    bytes::{Buf, BufMut},
    encoding, Message,
};
use serde::{Deserialize, Serialize};

/// Potential parse errors
#[derive(Debug, Display, Eq, Ord, PartialOrd, PartialEq, Clone)]
pub enum ResponderIdParseError {
    /// Failure from Utf8 for {0:0x?}
    FromUtf8Error(Vec<u8>),
    /// Invalid Format for {0}
    InvalidFormat(String),
}

#[cfg(feature = "std")]
impl std::error::Error for ResponderIdParseError {}

/// Node unique identifier.
#[derive(
    Clone, Default, Debug, Eq, Serialize, Deserialize, PartialEq, PartialOrd, Ord, Hash, Digestible,
)]
pub struct ResponderId(#[digestible(never_omit)] pub String);

impl Display for ResponderId {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ResponderId {
    type Err = ResponderIdParseError;

    fn from_str(src: &str) -> Result<ResponderId, Self::Err> {
        // ResponderId is expected to be host:port, so at least ensure we have a single
        // colon as a small sanity test.
        if !src.contains(':') {
            return Err(ResponderIdParseError::InvalidFormat(src.to_string()));
        }

        Ok(Self(src.to_string()))
    }
}

// This is needed for SCPNetworkState's NetworkState implementation.
impl AsRef<ResponderId> for ResponderId {
    fn as_ref(&self) -> &Self {
        self
    }
}

// Encode ResponderId as a proto string
impl Message for ResponderId {
    fn encode_raw<B>(&self, buf: &mut B)
    where
        B: BufMut,
        Self: Sized,
    {
        String::encode_raw(&self.0, buf)
    }

    fn merge_field<B>(
        &mut self,
        tag: u32,
        wire_type: encoding::WireType,
        buf: &mut B,
        ctx: encoding::DecodeContext,
    ) -> Result<(), prost::DecodeError>
    where
        B: Buf,
        Self: Sized,
    {
        String::merge_field(&mut self.0, tag, wire_type, buf, ctx)
    }

    fn encoded_len(&self) -> usize {
        String::encoded_len(&self.0)
    }

    fn clear(&mut self) {
        self.0.clear()
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use super::*;
    use alloc::{format, vec, vec::Vec};

    #[test]
    fn test_responder_id_from_str_valid() {
        let id = ResponderId::from_str("localhost:8080").unwrap();
        assert_eq!(id.0, "localhost:8080");
    }

    #[test]
    fn test_responder_id_from_str_with_ip() {
        let id = ResponderId::from_str("192.168.1.1:3000").unwrap();
        assert_eq!(id.0, "192.168.1.1:3000");
    }

    #[test]
    fn test_responder_id_from_str_with_ipv6() {
        let id = ResponderId::from_str("[::1]:8080").unwrap();
        assert_eq!(id.0, "[::1]:8080");
    }

    #[test]
    fn test_responder_id_from_str_invalid_no_colon() {
        let result = ResponderId::from_str("localhost");
        assert!(result.is_err());
        match result.unwrap_err() {
            ResponderIdParseError::InvalidFormat(s) => assert_eq!(s, "localhost"),
            _ => panic!("Expected InvalidFormat error"),
        }
    }

    #[test]
    fn test_responder_id_display() {
        let id = ResponderId("node1.example.com:443".to_string());
        assert_eq!(format!("{}", id), "node1.example.com:443");
    }

    #[test]
    fn test_responder_id_default() {
        let id = ResponderId::default();
        assert_eq!(id.0, "");
    }

    #[test]
    fn test_responder_id_equality() {
        let id1 = ResponderId::from_str("host:1234").unwrap();
        let id2 = ResponderId::from_str("host:1234").unwrap();
        let id3 = ResponderId::from_str("other:1234").unwrap();

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_responder_id_ordering() {
        let id_a = ResponderId::from_str("a:1").unwrap();
        let id_b = ResponderId::from_str("b:1").unwrap();
        let id_c = ResponderId::from_str("c:1").unwrap();

        assert!(id_a < id_b);
        assert!(id_b < id_c);
        assert!(id_a < id_c);
    }

    #[test]
    fn test_responder_id_hash() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let id1 = ResponderId::from_str("host:1234").unwrap();
        let id2 = ResponderId::from_str("host:1234").unwrap();

        let mut hasher1 = DefaultHasher::new();
        let mut hasher2 = DefaultHasher::new();

        id1.hash(&mut hasher1);
        id2.hash(&mut hasher2);

        assert_eq!(hasher1.finish(), hasher2.finish());
    }

    #[test]
    fn test_responder_id_clone() {
        let id1 = ResponderId::from_str("host:1234").unwrap();
        let id2 = id1.clone();

        assert_eq!(id1, id2);
    }

    #[test]
    fn test_responder_id_as_ref() {
        let id = ResponderId::from_str("host:1234").unwrap();
        let id_ref: &ResponderId = id.as_ref();

        assert_eq!(id_ref.0, "host:1234");
    }

    #[test]
    fn test_responder_id_prost_message_encode_decode() {
        let original = ResponderId::from_str("node.example.com:8443").unwrap();

        // Encode
        let mut buf = Vec::new();
        original.encode_raw(&mut buf);

        // The encoded length should be consistent
        assert_eq!(buf.len(), original.encoded_len());
    }

    #[test]
    fn test_responder_id_clear() {
        let mut id = ResponderId::from_str("host:1234").unwrap();
        id.clear();
        assert_eq!(id.0, "");
    }

    #[test]
    fn test_responder_id_serde_roundtrip() {
        let original = ResponderId::from_str("serde.test:9999").unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: ResponderId = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_responder_id_parse_error_display() {
        let err = ResponderIdParseError::InvalidFormat("bad".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Invalid Format"));
        assert!(msg.contains("bad"));
    }

    #[test]
    fn test_responder_id_parse_error_from_utf8_display() {
        let err = ResponderIdParseError::FromUtf8Error(vec![0xFF, 0xFE]);
        let msg = format!("{}", err);
        assert!(msg.contains("Utf8"));
    }
}
