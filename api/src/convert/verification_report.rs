// Copyright (c) 2018-2022 The Botho Foundation

//! Convert to/from external::VerificationReport

use crate::external;
use bth_blockchain_types::VerificationReport;

impl From<&VerificationReport> for external::VerificationReport {
    fn from(src: &VerificationReport) -> Self {
        Self {
            sig: src.sig.as_ref().map(|s| s.into()),
            chain: src.chain.as_slice().into(),
            http_body: src.http_body.clone(),
        }
    }
}

impl From<&external::VerificationReport> for VerificationReport {
    fn from(src: &external::VerificationReport) -> Self {
        VerificationReport {
            sig: src.sig.as_ref().map(Into::into),
            chain: src.chain.to_vec(),
            http_body: src.http_body.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pem::Pem;

    const IAS_JSON: &str = include_str!("../../tests/data/ias_ok.json");

    /// Test round-trip conversion of prost to protobuf to prost
    #[test]
    fn prost_to_proto_roundtrip() {
        use bth_blockchain_types::VerificationSignature;
        let report = VerificationReport {
            sig: Some(VerificationSignature {
                contents: b"this is a fake signature".to_vec(),
            }),
            chain: pem::parse_many(bth_crypto_x509_test_vectors::ok_rsa_chain_25519_leaf().0)
                .expect("Could not parse PEM input")
                .into_iter()
                .map(Pem::into_contents)
                .collect(),
            http_body: IAS_JSON.to_owned(),
        };

        // external -> prost
        let proto_report = external::VerificationReport::from(&report);
        // prost -> external
        let prost_report = VerificationReport::from(&proto_report);

        assert_eq!(report, prost_report);
    }
}
