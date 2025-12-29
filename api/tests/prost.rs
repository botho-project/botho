// Copyright (c) 2018-2022 The Botho Foundation

//! Tests that prost-versions of structures round-trip with the versions
//! generated from external.proto

use bth_account_keys::{AccountKey, PublicAddress, RootIdentity};
use bth_api::{blockchain, external, quorum_set};
use bth_blockchain_test_utils::{get_blocks, make_block_metadata, make_quorum_set, make_verification_report};
use bth_blockchain_types::{BlockData, BlockID, BlockMetadata, BlockVersion, QuorumSet, VerificationReport};
use bth_util_from_random::FromRandom;
use bth_util_serial::round_trip_message;
use bth_util_test_helper::{run_with_several_seeds, CryptoRng, RngCore};

// Generate some example root identities (non-fog only)
fn root_identity_examples<T: RngCore + CryptoRng>(rng: &mut T) -> Vec<RootIdentity> {
    vec![
        RootIdentity::from_random(rng),
        RootIdentity::from_random(rng),
        RootIdentity::from_random(rng),
    ]
}

// Test that RootIdentity roundtrips through .proto structure
#[test]
fn root_identity_round_trip() {
    run_with_several_seeds(|mut rng| {
        for example in root_identity_examples(&mut rng).iter() {
            round_trip_message::<RootIdentity, external::RootIdentity>(example);
        }
    })
}

// Test that AccountKey roundtrips through .proto structure
#[test]
fn account_key_round_trip() {
    run_with_several_seeds(|mut rng| {
        for example in root_identity_examples(&mut rng).iter() {
            round_trip_message::<AccountKey, external::AccountKey>(&AccountKey::from(example));
        }
    })
}

// Test that PublicAddress roundtrips through .proto structure
#[test]
fn public_address_round_trip() {
    run_with_several_seeds(|mut rng| {
        for example in root_identity_examples(&mut rng).iter() {
            round_trip_message::<PublicAddress, external::PublicAddress>(
                &AccountKey::from(example).default_subaddress(),
            );
        }
    })
}

// NOTE: SignedContingentInput round trip test removed - requires fog functionality
// which was removed as part of the fog removal.

#[test]
fn block_metadata_round_trip() {
    run_with_several_seeds(|mut rng| {
        let block_id = BlockID(FromRandom::from_random(&mut rng));
        let metadata = make_block_metadata(block_id, &mut rng);
        round_trip_message::<BlockMetadata, blockchain::BlockMetadata>(&metadata)
    })
}

#[test]
fn quorum_set_round_trip() {
    run_with_several_seeds(|mut rng| {
        let qs = make_quorum_set(&mut rng);
        round_trip_message::<QuorumSet, quorum_set::QuorumSet>(&qs)
    })
}

#[test]
fn verification_report_round_trip() {
    run_with_several_seeds(|mut rng| {
        let report = make_verification_report(&mut rng);
        round_trip_message::<VerificationReport, external::VerificationReport>(&report)
    })
}

#[test]
fn block_data_round_trip() {
    run_with_several_seeds(|mut rng| {
        let block_data = get_blocks(BlockVersion::MAX, 1, 2, 3, 4, 5, None, &mut rng)
            .pop()
            .unwrap();
        // This does not need to remain an invariant, as of this writing.
        // It's a nice property, though.
        round_trip_message::<BlockData, blockchain::ArchiveBlockV1>(&block_data);
    })
}
