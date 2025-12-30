// Copyright (c) 2018-2022 The Botho Foundation

use crate::{
    crypto::metadata::{MetadataSigner, MetadataVerifier},
    BlockID, QuorumSet,
};
use ::prost::Message;
use displaydoc::Display;
use bth_common::ResponderId;
use bth_crypto_digestible::Digestible;
use bth_crypto_keys::{Ed25519Pair, Ed25519Public, Ed25519Signature, SignatureError};
use serde::{Deserialize, Serialize};

/// Metadata for a block.
#[derive(Clone, Deserialize, Digestible, Display, Eq, Message, PartialEq, Serialize)]
pub struct BlockMetadataContents {
    /// The Block ID.
    #[prost(message, required, tag = 1)]
    block_id: BlockID,

    /// Quorum set configuration at the time of externalization.
    #[prost(message, required, tag = 2)]
    quorum_set: QuorumSet,

    /// Responder ID of the consensus node that externalized this block.
    #[prost(message, required, tag = 4)]
    responder_id: ResponderId,
}

impl BlockMetadataContents {
    /// Instantiate a [BlockMetadataContents] with the given data.
    pub fn new(
        block_id: BlockID,
        quorum_set: QuorumSet,
        responder_id: ResponderId,
    ) -> Self {
        Self {
            block_id,
            quorum_set,
            responder_id,
        }
    }

    /// Get the [BlockID].
    pub fn block_id(&self) -> &BlockID {
        &self.block_id
    }

    /// Get the [QuorumSet].
    pub fn quorum_set(&self) -> &QuorumSet {
        &self.quorum_set
    }

    /// Get the [ResponderId].
    pub fn responder_id(&self) -> &ResponderId {
        &self.responder_id
    }
}

/// Signed metadata for a block.
#[derive(Clone, Deserialize, Digestible, Display, Eq, Message, PartialEq, Serialize)]
pub struct BlockMetadata {
    /// Metadata signed by the consensus node.
    #[prost(message, required, tag = 1)]
    contents: BlockMetadataContents,

    /// Message signing key (signer).
    #[prost(message, required, tag = 2)]
    node_key: Ed25519Public,

    /// Signature using `node_key` over the Digestible encoding of `contents`.
    #[prost(message, required, tag = 3)]
    signature: Ed25519Signature,
}

impl BlockMetadata {
    /// Instantiate a [BlockMetadata] with the given data.
    pub fn new(
        contents: BlockMetadataContents,
        node_key: Ed25519Public,
        signature: Ed25519Signature,
    ) -> Self {
        Self {
            contents,
            node_key,
            signature,
        }
    }

    /// Instantiate a [BlockMetadata] by signing the given
    /// [BlockMetadataContents] with the given [Ed25519Pair].
    pub fn from_contents_and_keypair(
        contents: BlockMetadataContents,
        key_pair: &Ed25519Pair,
    ) -> Result<Self, SignatureError> {
        let signature = key_pair.sign_metadata(&contents)?;
        Ok(Self::new(contents, key_pair.public_key(), signature))
    }

    /// Verify that this signature is over a given block.
    pub fn verify(&self) -> Result<(), SignatureError> {
        self.node_key
            .verify_metadata(&self.contents, &self.signature)
    }

    /// Get the [BlockMetadataContents].
    pub fn contents(&self) -> &BlockMetadataContents {
        &self.contents
    }

    /// Get the signing key.
    pub fn node_key(&self) -> &Ed25519Public {
        &self.node_key
    }

    /// Get the signature.
    pub fn signature(&self) -> &Ed25519Signature {
        &self.signature
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::str::FromStr;
    use bth_util_from_random::FromRandom;
    use rand::{rngs::StdRng, SeedableRng};

    /// Create a test BlockID
    fn make_test_block_id() -> BlockID {
        BlockID::try_from(&[1u8; 32][..]).unwrap()
    }

    /// Create a test QuorumSet
    fn make_test_quorum_set() -> QuorumSet {
        QuorumSet::empty()
    }

    /// Create a test ResponderId
    fn make_test_responder_id() -> ResponderId {
        ResponderId::from_str("test-node:8080").unwrap()
    }

    /// Create a test Ed25519Pair with a deterministic seed
    fn make_test_keypair() -> Ed25519Pair {
        let mut rng = StdRng::seed_from_u64(42);
        Ed25519Pair::from_random(&mut rng)
    }

    /// Create another test Ed25519Pair with a different seed
    fn make_other_keypair() -> Ed25519Pair {
        let mut rng = StdRng::seed_from_u64(99);
        Ed25519Pair::from_random(&mut rng)
    }

    #[test]
    fn test_block_metadata_contents_new() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id.clone(),
            quorum_set.clone(),
            responder_id.clone(),
        );

        assert_eq!(contents.block_id(), &block_id);
        assert_eq!(contents.quorum_set(), &quorum_set);
        assert_eq!(contents.responder_id(), &responder_id);
    }

    #[test]
    fn test_block_metadata_contents_getters() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id.clone(),
            quorum_set.clone(),
            responder_id.clone(),
        );

        // Test all getter methods
        assert_eq!(*contents.block_id(), block_id);
        assert_eq!(*contents.quorum_set(), quorum_set);
        assert_eq!(contents.responder_id().to_string(), "test-node:8080");
    }

    #[test]
    fn test_block_metadata_new() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        let keypair = make_test_keypair();
        let signature = keypair.sign_metadata(&contents).unwrap();
        let node_key = keypair.public_key();

        let metadata = BlockMetadata::new(contents.clone(), node_key, signature.clone());

        assert_eq!(metadata.contents(), &contents);
        assert_eq!(metadata.node_key(), &node_key);
        assert_eq!(metadata.signature(), &signature);
    }

    #[test]
    fn test_block_metadata_from_contents_and_keypair() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        let keypair = make_test_keypair();

        let metadata = BlockMetadata::from_contents_and_keypair(contents.clone(), &keypair).unwrap();

        assert_eq!(metadata.contents(), &contents);
        assert_eq!(metadata.node_key(), &keypair.public_key());
    }

    #[test]
    fn test_block_metadata_verify_valid_signature() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        let keypair = make_test_keypair();
        let metadata = BlockMetadata::from_contents_and_keypair(contents, &keypair).unwrap();

        // Valid signature should verify
        assert!(metadata.verify().is_ok());
    }

    #[test]
    fn test_block_metadata_verify_invalid_signature() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        let keypair = make_test_keypair();
        let other_keypair = make_other_keypair();

        // Sign with one key, but use different public key
        let signature = keypair.sign_metadata(&contents).unwrap();
        let wrong_node_key = other_keypair.public_key();

        let metadata = BlockMetadata::new(contents, wrong_node_key, signature);

        // Invalid signature should fail to verify
        assert!(metadata.verify().is_err());
    }

    #[test]
    fn test_block_metadata_contents_equality() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents1 = BlockMetadataContents::new(
            block_id.clone(),
            quorum_set.clone(),
            responder_id.clone(),
        );

        let contents2 = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        assert_eq!(contents1, contents2);
    }

    #[test]
    fn test_block_metadata_clone() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        let keypair = make_test_keypair();
        let metadata = BlockMetadata::from_contents_and_keypair(contents, &keypair).unwrap();

        let cloned = metadata.clone();
        assert_eq!(metadata, cloned);
    }

    #[test]
    fn test_block_metadata_prost_encode_decode() {
        let block_id = make_test_block_id();
        let quorum_set = make_test_quorum_set();
        let responder_id = make_test_responder_id();

        let contents = BlockMetadataContents::new(
            block_id,
            quorum_set,
            responder_id,
        );

        let keypair = make_test_keypair();
        let metadata = BlockMetadata::from_contents_and_keypair(contents, &keypair).unwrap();

        // Encode
        let mut buf = Vec::new();
        metadata.encode(&mut buf).unwrap();

        // Decode
        let decoded = BlockMetadata::decode(&buf[..]).unwrap();
        assert_eq!(metadata, decoded);

        // Signature should still verify after round-trip
        assert!(decoded.verify().is_ok());
    }
}
