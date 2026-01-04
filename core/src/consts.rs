// Copyright (c) 2018-2022 The Botho Foundation

//! Botho core constants

/// The BIP44 "usage" component of a BIP32 path.
///
/// See https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki for more details.
pub const USAGE_BIP44: u32 = 44;

/// The Botho "coin type" component of a BIP32 path.
///
/// See https://github.com/satoshilabs/slips/blob/master/slip-0044.md for reference.
pub const COINTYPE_BOTHO: u32 = 866;

/// Domain separator for hashing a private view key and index into a subaddress.
pub(crate) const SUBADDRESS_DOMAIN_TAG: &str = "bth_subaddress";

/// An account's "default address" is its zero^th subaddress.
pub const DEFAULT_SUBADDRESS_INDEX: u64 = 0;

/// u64::MAX is a reserved subaddress value for "invalid/none" (MCIP #36)
pub const INVALID_SUBADDRESS_INDEX: u64 = u64::MAX;

/// An account's "change address" is the 1st reserved subaddress,
/// counting down from `u64::MAX`. (See MCIP #4, MCIP #36)
pub const CHANGE_SUBADDRESS_INDEX: u64 = u64::MAX - 1;

/// The subaddress derived using u64::MAX - 2 is the reserved subaddress
/// for gift code TxOuts to be sent as specified in MCIP #32.
pub const GIFT_CODE_SUBADDRESS_INDEX: u64 = u64::MAX - 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bip44_usage_constant() {
        // BIP44 usage should be 44 as defined in BIP-0044
        assert_eq!(USAGE_BIP44, 44);
    }

    #[test]
    fn test_cointype_botho() {
        // Botho coin type should be 866 as registered in SLIP-0044
        assert_eq!(COINTYPE_BOTHO, 866);
    }

    #[test]
    fn test_subaddress_domain_tag() {
        // Domain tag should be the expected string
        assert_eq!(SUBADDRESS_DOMAIN_TAG, "bth_subaddress");
    }

    #[test]
    fn test_default_subaddress_index() {
        // Default subaddress is always index 0
        assert_eq!(DEFAULT_SUBADDRESS_INDEX, 0);
    }

    #[test]
    fn test_invalid_subaddress_index() {
        // Invalid/none subaddress is u64::MAX (MCIP #36)
        assert_eq!(INVALID_SUBADDRESS_INDEX, u64::MAX);
    }

    #[test]
    fn test_change_subaddress_index() {
        // Change address is u64::MAX - 1 (MCIP #4, #36)
        assert_eq!(CHANGE_SUBADDRESS_INDEX, u64::MAX - 1);
    }

    #[test]
    fn test_gift_code_subaddress_index() {
        // Gift code subaddress is u64::MAX - 2 (MCIP #32)
        assert_eq!(GIFT_CODE_SUBADDRESS_INDEX, u64::MAX - 2);
    }

    #[test]
    fn test_reserved_subaddress_ordering() {
        // Reserved subaddresses should have proper ordering:
        // DEFAULT < GIFT_CODE < CHANGE < INVALID
        assert!(DEFAULT_SUBADDRESS_INDEX < GIFT_CODE_SUBADDRESS_INDEX);
        assert!(GIFT_CODE_SUBADDRESS_INDEX < CHANGE_SUBADDRESS_INDEX);
        assert!(CHANGE_SUBADDRESS_INDEX < INVALID_SUBADDRESS_INDEX);
    }

    #[test]
    fn test_reserved_indices_are_distinct() {
        // All reserved indices should be unique
        let indices = [
            DEFAULT_SUBADDRESS_INDEX,
            CHANGE_SUBADDRESS_INDEX,
            GIFT_CODE_SUBADDRESS_INDEX,
            INVALID_SUBADDRESS_INDEX,
        ];

        for i in 0..indices.len() {
            for j in (i + 1)..indices.len() {
                assert_ne!(
                    indices[i], indices[j],
                    "Indices {} and {} should be distinct",
                    i, j
                );
            }
        }
    }

    #[test]
    fn test_reserved_indices_leave_room_for_user_subaddresses() {
        // There should be ample room for user subaddresses before hitting reserved
        // indices User can use indices 1 through at least a billion without
        // collision
        let max_user_index = 1_000_000_000u64;
        assert!(max_user_index < GIFT_CODE_SUBADDRESS_INDEX);
        assert!(max_user_index < CHANGE_SUBADDRESS_INDEX);
        assert!(max_user_index < INVALID_SUBADDRESS_INDEX);
    }
}
