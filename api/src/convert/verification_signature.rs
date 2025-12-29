// Copyright (c) 2018-2022 The Botho Foundation

//! Convert to/from external::VerificationSignature

use crate::external;
use bt_blockchain_types::VerificationSignature;

impl From<&VerificationSignature> for external::VerificationSignature {
    fn from(src: &VerificationSignature) -> Self {
        Self {
            contents: src.contents.clone(),
        }
    }
}

impl From<&external::VerificationSignature> for VerificationSignature {
    fn from(src: &external::VerificationSignature) -> Self {
        VerificationSignature {
            contents: src.contents.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test round-trip conversion of prost to protobuf to prost
    #[test]
    fn prost_to_proto_roundtrip() {
        let sig = VerificationSignature {
            contents: b"this is a fake signature".to_vec(),
        };

        // external -> prost
        let proto_sig = external::VerificationSignature::from(&sig);
        // prost -> external
        let prost_sig = VerificationSignature::from(&proto_sig);

        assert_eq!(sig, prost_sig);
    }
}
