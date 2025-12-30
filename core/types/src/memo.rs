#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Memo HMAC container type
#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Hmac(pub [u8; 16]);

impl AsRef<[u8; 16]> for Hmac {
    fn as_ref(&self) -> &[u8; 16] {
        &self.0
    }
}

impl From<Hmac> for [u8; 16] {
    fn from(value: Hmac) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::format;

    #[test]
    fn test_hmac_creation() {
        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hmac = Hmac(bytes);
        assert_eq!(hmac.0, bytes);
    }

    #[test]
    fn test_hmac_as_ref() {
        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hmac = Hmac(bytes);
        assert_eq!(hmac.as_ref(), &bytes);
    }

    #[test]
    fn test_hmac_into_bytes() {
        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hmac = Hmac(bytes);
        let recovered: [u8; 16] = hmac.into();
        assert_eq!(bytes, recovered);
    }

    #[test]
    fn test_hmac_clone() {
        let bytes: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let hmac = Hmac(bytes);
        let cloned = hmac.clone();
        assert_eq!(hmac, cloned);
    }

    #[test]
    fn test_hmac_partial_eq() {
        let bytes1: [u8; 16] = [1; 16];
        let bytes2: [u8; 16] = [1; 16];
        let bytes3: [u8; 16] = [2; 16];

        let hmac1 = Hmac(bytes1);
        let hmac2 = Hmac(bytes2);
        let hmac3 = Hmac(bytes3);

        assert_eq!(hmac1, hmac2);
        assert_ne!(hmac1, hmac3);
    }

    #[test]
    fn test_hmac_debug() {
        let bytes: [u8; 16] = [0; 16];
        let hmac = Hmac(bytes);
        let debug = format!("{:?}", hmac);
        assert!(debug.contains("Hmac"));
    }
}
