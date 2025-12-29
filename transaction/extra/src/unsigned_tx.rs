// Copyright (c) 2018-2023 The Botho Foundation

use alloc::vec::Vec;
use bth_crypto_ring_signature_signer::RingSigner;
use bth_transaction_core::{
    ring_ct::{
        CommittedTagSigningData, Error as RingCtError, ExtendedMessageDigest, InputRing,
        OutputSecret, SignatureRctBulletproofs, SigningData,
    },
    tx::{Tx, TxPrefix},
    FeeMap,
};
use bth_transaction_summary::{TxOutSummaryUnblindingData, TxSummaryUnblindingData};
use bth_transaction_types::{Amount, BlockVersion, TokenId, TxSummary, UnmaskedAmount};
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};

/// A structure containing an unsigned transaction together with the data
/// required to sign it that does not involve the spend private key.
///
/// The idea is that this can be generated without having the spend private key,
/// and then transferred to an offline/hardware service that does have the spend
/// private key, which can then be used together with the data here to produce a
/// valid, signed Tx. Note that whether the UnsignedTx can be signed on its own
/// or requires the spend private key will depend on the contents of the
/// InputRings.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnsignedTx {
    /// The fully constructed TxPrefix.
    pub tx_prefix: TxPrefix,

    /// rings
    pub rings: Vec<InputRing>,

    /// Output secrets
    pub tx_out_unblinding_data: Vec<TxOutSummaryUnblindingData>,

    /// Block version
    pub block_version: BlockVersion,
}

impl UnsignedTx {
    /// Sign the transaction signing data with a given signer
    pub fn sign<RNG: CryptoRng + RngCore, S: RingSigner + ?Sized>(
        &self,
        signer: &S,
        fee_map: Option<&FeeMap>,
        rng: &mut RNG,
    ) -> Result<Tx, RingCtError> {
        let prefix = self.tx_prefix.clone();
        let output_secrets: Vec<OutputSecret> = self
            .tx_out_unblinding_data
            .iter()
            .map(|data| OutputSecret::from(data.unmasked_amount.clone()))
            .collect();
        let signature = SignatureRctBulletproofs::sign(
            self.block_version,
            &prefix,
            self.rings.as_slice(),
            output_secrets.as_slice(),
            Amount::new(prefix.fee, TokenId::from(prefix.fee_token_id)),
            signer,
            rng,
        )?;
        // Note: fee_map is no longer used for fee_map_digest (removed with SGX)
        let _ = fee_map;

        Ok(Tx { prefix, signature })
    }

    /// Sign the transaction with committed cluster tags (Phase 2).
    ///
    /// This method creates the standard ring signature and then adds the
    /// extended tag signature for committed cluster tags.
    ///
    /// # Arguments
    /// * `signer` - The ring signer entity
    /// * `fee_map` - Optional fee map (legacy, unused)
    /// * `tag_data` - Committed tag signing data from mc-cluster-tax
    /// * `rng` - Random number generator
    ///
    /// # Feature
    /// This method requires the `cluster-tax` feature to be enabled.
    #[cfg(feature = "cluster-tax")]
    pub fn sign_with_committed_tags<RNG: CryptoRng + RngCore, S: RingSigner + ?Sized>(
        &self,
        signer: &S,
        fee_map: Option<&FeeMap>,
        tag_data: &CommittedTagSigningData,
        rng: &mut RNG,
    ) -> Result<Tx, RingCtError> {
        use bth_cluster_tax::{
            create_tag_signature, CommittedTagVector, CommittedTagVectorSecret, RingTagData,
            TagSigningConfig, TagSigningInput, TagSigningOutput,
        };

        let prefix = self.tx_prefix.clone();
        let output_secrets: Vec<OutputSecret> = self
            .tx_out_unblinding_data
            .iter()
            .map(|data| OutputSecret::from(data.unmasked_amount.clone()))
            .collect();

        // Create the base signature with tag data validation
        let mut signature = SignatureRctBulletproofs::sign_with_committed_tags(
            self.block_version,
            &prefix,
            self.rings.as_slice(),
            output_secrets.as_slice(),
            Amount::new(prefix.fee, TokenId::from(prefix.fee_token_id)),
            tag_data,
            signer,
            rng,
        )?;

        // Deserialize and convert to cluster-tax types
        let inputs: Vec<TagSigningInput> = tag_data
            .input_tag_rings
            .iter()
            .map(|ring| {
                let ring_tags = ring
                    .member_tags
                    .iter()
                    .filter_map(|bytes| CommittedTagVector::from_bytes(bytes).ok())
                    .collect();
                let tag_secret = CommittedTagVectorSecret::from_bytes(&ring.real_tag_secret)
                    .expect("Invalid tag secret bytes");
                TagSigningInput {
                    ring_tags,
                    real_index: ring.real_index,
                    tag_secret,
                }
            })
            .collect();

        let outputs: Vec<TagSigningOutput> = tag_data
            .output_tag_secrets
            .iter()
            .map(|out| {
                let tag_commitment = CommittedTagVector::from_bytes(&out.tag_commitment)
                    .expect("Invalid tag commitment bytes");
                let tag_secret = CommittedTagVectorSecret::from_bytes(&out.tag_secret)
                    .expect("Invalid tag secret bytes");
                TagSigningOutput {
                    tag_commitment,
                    tag_secret,
                }
            })
            .collect();

        let config = TagSigningConfig {
            decay_rate: tag_data.decay_rate,
        };

        // Create the tag signature
        let tag_sig_bytes = create_tag_signature(&inputs, &outputs, &config, rng)
            .map_err(|_| RingCtError::InvalidTagSignature)?;

        signature.extended_tag_signature = Some(tag_sig_bytes);

        // Note: fee_map is no longer used for fee_map_digest (removed with SGX)
        let _ = fee_map;

        Ok(Tx { prefix, signature })
    }

    /// Get prepared (but unsigned) ringct bulletproofs which can be signed
    /// later. Also gets the TxSummary and related digests.
    ///
    /// Returns:
    /// * SigningData This is essentially all parts of SignatureRctBulletproofs
    ///   except the ring signatures
    /// * TxSummary This is a small snapshot of the Tx used by hardware wallets
    /// * TxSummaryUnblindingData
    /// * ExtendedMessageDigest This is a digest used in connection with the
    ///   TxSummary
    pub fn get_signing_data<RNG: CryptoRng + RngCore>(
        &self,
        rng: &mut RNG,
    ) -> Result<
        (
            SigningData,
            TxSummary,
            TxSummaryUnblindingData,
            ExtendedMessageDigest,
        ),
        RingCtError,
    > {
        let fee_amount = Amount::new(
            self.tx_prefix.fee,
            TokenId::from(self.tx_prefix.fee_token_id),
        );
        let output_secrets: Vec<OutputSecret> = self
            .tx_out_unblinding_data
            .iter()
            .map(|data| OutputSecret::from(data.unmasked_amount.clone()))
            .collect();
        let (signing_data, tx_summary, extended_message_digest) = SigningData::new_with_summary(
            self.block_version,
            &self.tx_prefix,
            &self.rings,
            &output_secrets,
            fee_amount,
            true,
            rng,
        )?;
        // Try to build the TxSummary unblinding data, which requires the amounts from
        // the rings, and the blinding factors from the signing data segment.
        if signing_data.pseudo_output_blindings.len() != self.rings.len() {
            return Err(RingCtError::LengthMismatch(
                signing_data.pseudo_output_blindings.len(),
                self.rings.len(),
            ));
        }
        let tx_summary_unblinding_data = TxSummaryUnblindingData {
            block_version: *self.block_version,
            outputs: self.tx_out_unblinding_data.clone(),
            inputs: signing_data
                .pseudo_output_blindings
                .iter()
                .zip(self.rings.iter())
                .map(|(blinding, ring)| {
                    let amount = ring.amount();
                    UnmaskedAmount {
                        value: amount.value,
                        token_id: *amount.token_id,
                        blinding: (*blinding).into(),
                    }
                })
                .collect(),
        };
        Ok((
            signing_data,
            tx_summary,
            tx_summary_unblinding_data,
            extended_message_digest,
        ))
    }
}
