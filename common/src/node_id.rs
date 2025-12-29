// Copyright (c) 2018-2022 The Botho Foundation

//! The Node ID type

use crate::responder_id::ResponderId;
use core::{
    cmp::Ordering,
    fmt::{Debug, Display, Formatter, Result as FmtResult},
    hash::{Hash, Hasher},
};
use displaydoc::Display;
use bth_crypto_digestible::Digestible;
use bth_crypto_keys::{Ed25519Public, KeyError};
use prost::Message;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Deserialize, Display, Hash, Eq, Ord, PartialEq, PartialOrd, Serialize,
)]
pub enum NodeIDError {
    /// Could not create NodeID due to serialization failure
    Deserialization,
    /// The input length was too short or not right (padding)
    InvalidInputLength,
    /// The output buffer was too short for the data
    InvalidOutputLength,
    /// The input data contained invalid characters
    InvalidInput,
    /// Could not parse public key for NodeID
    KeyParseError,
}

impl From<KeyError> for NodeIDError {
    fn from(_src: KeyError) -> Self {
        NodeIDError::KeyParseError
    }
}

/// Node unique identifier containing a responder_id as well as a unique public
/// key
#[derive(Clone, Deserialize, Digestible, Message, Serialize)]
pub struct NodeID {
    /// The Responder ID for this node
    #[prost(message, required, tag = 1)]
    pub responder_id: ResponderId,
    /// The public message-signing key for this node
    #[prost(message, required, tag = 2)]
    pub public_key: Ed25519Public,
}

impl Display for NodeID {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "{}:{}", self.responder_id, self.public_key)
    }
}

impl Hash for NodeID {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.public_key.hash(hasher);
    }
}

impl PartialEq for NodeID {
    fn eq(&self, other: &Self) -> bool {
        self.public_key == other.public_key
    }
}

impl Eq for NodeID {}

impl PartialOrd for NodeID {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NodeID {
    fn cmp(&self, other: &Self) -> Ordering {
        self.public_key.cmp(&other.public_key)
    }
}

impl From<&NodeID> for ResponderId {
    fn from(src: &NodeID) -> Self {
        src.responder_id.clone()
    }
}

// This is needed for SCPNetworkState's NetworkState implementation.
impl AsRef<ResponderId> for NodeID {
    fn as_ref(&self) -> &ResponderId {
        &self.responder_id
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use super::*;
    use alloc::{format, vec::Vec};
    use bth_crypto_keys::Ed25519Private;
    use core::str::FromStr;

    /// Helper to create a test NodeID with deterministic keys
    fn make_test_node_id(responder: &str, key_seed: u8) -> NodeID {
        let responder_id = ResponderId::from_str(responder).unwrap();
        // Create deterministic Ed25519 public key from seed
        let mut key_bytes = [0u8; 32];
        key_bytes[0] = key_seed;
        // Create private key from bytes, then derive public key
        let private_key = Ed25519Private::try_from(&key_bytes[..]).unwrap();
        let public_key = Ed25519Public::from(&private_key);

        NodeID {
            responder_id,
            public_key,
        }
    }

    #[test]
    fn test_node_id_display() {
        let node = make_test_node_id("node1:8080", 1);
        let display = format!("{}", node);
        assert!(display.starts_with("node1:8080:"));
    }

    #[test]
    fn test_node_id_equality_by_public_key() {
        // Two NodeIDs with the same public key are equal, regardless of responder_id
        let node1 = make_test_node_id("host1:1111", 42);
        let node2 = make_test_node_id("host2:2222", 42);
        let node3 = make_test_node_id("host1:1111", 99);

        // Same public key seed => equal
        assert_eq!(node1, node2);
        // Different public key seed => not equal
        assert_ne!(node1, node3);
    }

    #[test]
    fn test_node_id_hash_by_public_key() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let node1 = make_test_node_id("host1:1111", 42);
        let node2 = make_test_node_id("host2:2222", 42);

        let mut hasher1 = DefaultHasher::new();
        let mut hasher2 = DefaultHasher::new();

        node1.hash(&mut hasher1);
        node2.hash(&mut hasher2);

        // Same public key => same hash
        assert_eq!(hasher1.finish(), hasher2.finish());
    }

    #[test]
    fn test_node_id_ordering_by_public_key() {
        let node1 = make_test_node_id("z:9999", 1);
        let node2 = make_test_node_id("a:1111", 2);

        // Ordering is by public key, not by responder_id
        // Different seeds produce different keys, ordering depends on key bytes
        assert_ne!(node1.cmp(&node2), Ordering::Equal);
    }

    #[test]
    fn test_node_id_partial_ord_consistent_with_ord() {
        let node1 = make_test_node_id("host:1", 10);
        let node2 = make_test_node_id("host:2", 20);

        assert_eq!(node1.partial_cmp(&node2), Some(node1.cmp(&node2)));
    }

    #[test]
    fn test_node_id_from_to_responder_id() {
        let node = make_test_node_id("original:1234", 5);
        let responder: ResponderId = (&node).into();

        assert_eq!(responder.0, "original:1234");
    }

    #[test]
    fn test_node_id_as_ref_responder_id() {
        let node = make_test_node_id("test:5678", 7);
        let responder_ref: &ResponderId = node.as_ref();

        assert_eq!(responder_ref.0, "test:5678");
    }

    #[test]
    fn test_node_id_clone() {
        let node1 = make_test_node_id("clone:1234", 99);
        let node2 = node1.clone();

        assert_eq!(node1, node2);
        assert_eq!(node1.responder_id, node2.responder_id);
    }

    #[test]
    fn test_node_id_error_display() {
        let errors = [
            NodeIDError::Deserialization,
            NodeIDError::InvalidInputLength,
            NodeIDError::InvalidOutputLength,
            NodeIDError::InvalidInput,
            NodeIDError::KeyParseError,
        ];

        for err in errors {
            let msg = format!("{}", err);
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn test_node_id_error_from_key_error() {
        let key_error = KeyError::LengthMismatch(32, 64);
        let node_error: NodeIDError = key_error.into();

        assert_eq!(node_error, NodeIDError::KeyParseError);
    }

    #[test]
    fn test_node_id_error_equality() {
        assert_eq!(NodeIDError::Deserialization, NodeIDError::Deserialization);
        assert_ne!(NodeIDError::Deserialization, NodeIDError::InvalidInput);
    }

    #[test]
    fn test_node_id_error_ordering() {
        // Errors should be orderable
        let err1 = NodeIDError::Deserialization;
        let err2 = NodeIDError::InvalidInput;

        // Just verify ordering works without panicking
        let _ = err1.cmp(&err2);
        let _ = err1.partial_cmp(&err2);
    }

    #[test]
    fn test_node_id_in_hash_set() {
        use std::collections::HashSet;

        let node1 = make_test_node_id("host:1", 1);
        let node2 = make_test_node_id("host:2", 1); // Same key seed
        let node3 = make_test_node_id("host:3", 2); // Different key seed

        let mut set = HashSet::new();
        set.insert(node1.clone());

        // node2 has same public key, should be considered duplicate
        assert!(set.contains(&node2));
        // node3 has different public key
        assert!(!set.contains(&node3));

        set.insert(node3);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_node_id_prost_message() {
        let node = make_test_node_id("proto:8443", 55);

        // Encode
        let mut buf = Vec::new();
        node.encode(&mut buf).unwrap();

        // Verify it encoded something
        assert!(!buf.is_empty());

        // Decode
        let decoded = NodeID::decode(&buf[..]).unwrap();
        assert_eq!(node, decoded);
    }

    #[test]
    fn test_node_id_serde_roundtrip() {
        let node = make_test_node_id("serde:9999", 77);

        let json = serde_json::to_string(&node).unwrap();
        let decoded: NodeID = serde_json::from_str(&json).unwrap();

        assert_eq!(node, decoded);
    }
}
