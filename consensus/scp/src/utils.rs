// Copyright (c) 2018-2022 The Botho Foundation

use crate::SlotIndex;
use bth_common::fast_hash;
use primitive_types::U256;

/// A "salted" Keccak hash function, parametrized by slot, round, and an extra
/// value.
///
/// # Arguments
/// * `slot_index`
/// * `extra_salt`
/// * `round_index`
/// * `bytes` - The bytes to hash.
///
/// # Returns
/// 256-bit unsigned value
/// Keccak(slot_index || extra_salt || round_index || bytes), where || denotes
/// concatenation,
pub fn slot_round_salted_keccak(
    slot_index: SlotIndex,
    extra_salt: u8,
    round_index: u32,
    bytes: &[u8],
) -> U256 {
    let slot_index_bytes: [u8; 8] = slot_index.to_be_bytes();
    let round_index_bytes: [u8; 4] = round_index.to_be_bytes();
    let extra: [u8; 1] = [extra_salt]; // Wrap this in an array so that concatenation is more consistent.

    let mut concatenation: Vec<u8> = vec![];
    concatenation.extend(slot_index_bytes.iter());
    concatenation.extend(extra.iter());
    concatenation.extend(round_index_bytes.iter());
    concatenation.extend(bytes.iter());

    // Big-endian by construction: primitive-types < 0.13 implemented
    // `From<[u8; 32]>` as a big-endian read, and SCP round priorities derived
    // from this value are consensus-relevant, so the byte order must never
    // change (see the golden test below).
    U256::from_big_endian(&fast_hash(&concatenation))
}

#[cfg(test)]
mod utils_tests {
    use super::*;

    /// Golden vector: pins the hash-to-U256 byte order across dependency
    /// upgrades. The expected value is `keccak(concatenation)` read as a
    /// big-endian integer, exactly what `U256::from([u8; 32])` produced on
    /// primitive-types 0.12 — a silent endianness flip here would fork SCP
    /// round priorities.
    #[test]
    fn slot_round_salted_keccak_is_big_endian_stable() {
        let value = slot_round_salted_keccak(1, 2, 3, b"golden");
        let hash = fast_hash(
            &[
                1u64.to_be_bytes().as_slice(),
                &[2u8],
                3u32.to_be_bytes().as_slice(),
                b"golden",
            ]
            .concat(),
        );
        assert_eq!(value.to_big_endian(), hash);
    }
}
