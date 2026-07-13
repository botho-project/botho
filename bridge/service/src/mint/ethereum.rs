// Copyright (c) 2024 The Botho Foundation

//! Ethereum wBTH minting via alloy.
//!
//! Per ADR 0002, the Ethereum mint authority is a Gnosis Safe whose owners
//! are the validators' secp256k1 keys and which holds `MINTER_ROLE` on
//! `WrappedBTH.sol`. A mint is submitted as:
//!
//! ```text
//! relayer EOA --eth_sendRawTransaction--> Safe.execTransaction(
//!     to = WrappedBTH, data = bridgeMint(to, amount, orderId),
//!     signatures = t-of-n owner signatures from the #824 attestation)
//! ```
//!
//! The `bytes32` order id (`BridgeOrder::order_id_bytes`) is the on-chain
//! idempotency key: `WrappedBTH.sol` records it in `processedOrders` and
//! reverts on a duplicate (#826), closing the double-mint window even if
//! an authorization is re-submitted.

use alloy::{
    eips::{eip2718::Encodable2718, BlockNumberOrTag},
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, Bytes, B256, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::{Log, TransactionRequest},
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{eip712_domain, SolCall, SolEvent, SolStruct},
};
use async_trait::async_trait;
use bth_bridge_core::{
    BridgeOrder, Chain, EthereumConfig, GasPriceStrategy, MintAuthorization, SignatureScheme,
};
use tracing::{debug, info, warn};

use super::{ConfirmationStatus, MintError, Minter, PreparedMint};

sol! {
    /// Typed binding for the wBTH token (`contracts/ethereum/contracts/WrappedBTH.sol`).
    #[allow(missing_docs)]
    interface IWrappedBTH {
        function bridgeMint(address to, uint256 amount, bytes32 orderId) external;
        event BridgeMint(address indexed to, uint256 amount, bytes32 indexed orderId);
    }

    /// Gnosis Safe (v1.3+) surface used by the bridge.
    #[allow(missing_docs)]
    interface IGnosisSafe {
        function nonce() external view returns (uint256);
        function execTransaction(
            address to,
            uint256 value,
            bytes calldata data,
            uint8 operation,
            uint256 safeTxGas,
            uint256 baseGas,
            uint256 gasPrice,
            address gasToken,
            address payable refundReceiver,
            bytes memory signatures
        ) external payable returns (bool success);
    }

    /// Gnosis Safe EIP-712 transaction struct (for computing the digest the
    /// #824 attestation signers must sign).
    #[allow(missing_docs)]
    struct SafeTx {
        address to;
        uint256 value;
        bytes data;
        uint8 operation;
        uint256 safeTxGas;
        uint256 baseGas;
        uint256 gasPrice;
        address gasToken;
        address refundReceiver;
        uint256 nonce;
    }
}

/// Headroom multiplier applied to `eth_estimateGas` (percent).
const GAS_HEADROOM_PERCENT: u64 = 125;

/// Minimum priority fee (wei) so the tx is never submitted tip-less.
const MIN_PRIORITY_FEE_WEI: u128 = 100_000_000; // 0.1 gwei

/// Encode the `bridgeMint(to, amount, orderId)` calldata.
pub fn encode_bridge_mint_calldata(to: Address, amount: U256, order_id: [u8; 32]) -> Vec<u8> {
    IWrappedBTH::bridgeMintCall {
        to,
        amount,
        orderId: B256::from(order_id),
    }
    .abi_encode()
}

/// Compute the Gnosis Safe EIP-712 transaction hash for a `bridgeMint` call
/// wrapped at the given Safe nonce. This is the digest the #824 attestation
/// protocol must collect owner signatures over.
pub fn safe_tx_hash(
    chain_id: u64,
    safe: Address,
    wbth: Address,
    mint_calldata: &[u8],
    safe_nonce: U256,
) -> B256 {
    let tx = SafeTx {
        to: wbth,
        value: U256::ZERO,
        data: Bytes::copy_from_slice(mint_calldata),
        operation: 0,
        safeTxGas: U256::ZERO,
        baseGas: U256::ZERO,
        gasPrice: U256::ZERO,
        gasToken: Address::ZERO,
        refundReceiver: Address::ZERO,
        nonce: safe_nonce,
    };
    // Safe's domain separator only carries chainId + verifyingContract.
    let domain = eip712_domain! {
        chain_id: chain_id,
        verifying_contract: safe,
    };
    tx.eip712_signing_hash(&domain)
}

/// Assemble the Safe `signatures` blob from a threshold attestation.
///
/// Gnosis Safe requires the 65-byte `{r, s, v}` owner signatures
/// concatenated in ascending order of owner address, with no duplicates.
pub fn assemble_safe_signatures(auth: &MintAuthorization) -> Result<Bytes, MintError> {
    if auth.scheme != SignatureScheme::Secp256k1 {
        return Err(MintError::Attestation(
            "Ethereum mint requires secp256k1 attestation signatures".to_string(),
        ));
    }
    if !auth.meets_threshold() {
        return Err(MintError::Attestation(format!(
            "attestation has {} signature(s), threshold is {}",
            auth.signatures.len(),
            auth.threshold
        )));
    }

    let mut sigs = auth.signatures.clone();
    sigs.sort_by(|a, b| a.signer.cmp(&b.signer));
    sigs.dedup_by(|a, b| a.signer == b.signer);

    let mut blob = Vec::with_capacity(sigs.len() * 65);
    for sig in &sigs {
        if sig.signer.len() != 20 {
            return Err(MintError::Attestation(format!(
                "safe owner identity must be a 20-byte address, got {} bytes",
                sig.signer.len()
            )));
        }
        if sig.signature.len() != 65 {
            return Err(MintError::Attestation(format!(
                "safe owner signature must be 65 bytes, got {}",
                sig.signature.len()
            )));
        }
        blob.extend_from_slice(&sig.signature);
    }

    Ok(blob.into())
}

/// Pre-broadcast Safe-nonce cross-check (#848).
///
/// The collected Safe owner signatures on `auth` are bound to a specific Safe
/// nonce (`auth.safe_nonce`). If an unrelated Safe transaction executed
/// between attestation collection and mint submission, `on_chain_nonce` has
/// advanced past it and `Safe.execTransaction` would revert. Detecting the
/// mismatch here — before any transaction is persisted or broadcast — lets the
/// engine re-authorize and re-collect at the fresh nonce instead of
/// broadcasting a doomed transaction and silently discarding threshold
/// signatures.
///
/// `Ok(())` when the authorization carries no nonce (Solana / legacy) or the
/// attested nonce equals the on-chain nonce; a retryable
/// [`MintError::StaleNonce`] otherwise.
fn check_attested_nonce(auth: &MintAuthorization, on_chain_nonce: U256) -> Result<(), MintError> {
    if let Some(attested_nonce) = auth.safe_nonce {
        if U256::from(attested_nonce) != on_chain_nonce {
            return Err(MintError::StaleNonce(format!(
                "attestation bound to Safe nonce {} but Safe.nonce() is now {}; \
                 re-authorizing to re-collect signatures at the current nonce",
                attested_nonce, on_chain_nonce
            )));
        }
    }
    Ok(())
}

/// Map the configured [`GasPriceStrategy`] to EIP-1559 fee fields, given the
/// next block's base fee and the observed priority-fee percentiles from
/// `eth_feeHistory` (10th / 50th / 90th).
///
/// Returns `(max_fee_per_gas, max_priority_fee_per_gas)` in wei.
pub fn eip1559_fees(
    strategy: GasPriceStrategy,
    next_base_fee: u128,
    tip_percentiles: [u128; 3],
) -> (u128, u128) {
    match strategy {
        GasPriceStrategy::Fixed(gwei) => {
            // Legacy-style fixed price: cap total and tip at the fixed value.
            let wei = (gwei as u128) * 1_000_000_000;
            (wei, wei)
        }
        _ => {
            let tip = match strategy {
                GasPriceStrategy::Low => tip_percentiles[0],
                GasPriceStrategy::Medium => tip_percentiles[1],
                GasPriceStrategy::High => tip_percentiles[2],
                GasPriceStrategy::Fixed(_) => unreachable!(),
            }
            .max(MIN_PRIORITY_FEE_WEI);
            // Double the base fee so the tx survives 100% base-fee growth
            // while waiting for inclusion.
            (next_base_fee.saturating_mul(2).saturating_add(tip), tip)
        }
    }
}

/// Scan receipt logs for the `BridgeMint` event bound to `order_id`, emitted
/// by the wBTH contract.
pub fn find_bridge_mint_event(logs: &[Log], wbth: Address, order_id: [u8; 32]) -> bool {
    let order_topic = B256::from(order_id);
    logs.iter().any(|log| {
        log.address() == wbth
            && log.topic0() == Some(&IWrappedBTH::BridgeMint::SIGNATURE_HASH)
            // topics: [signature, to (indexed), orderId (indexed)]
            && log.topics().get(2) == Some(&order_topic)
    })
}

/// Ethereum minting backend.
pub struct EthMinter {
    config: EthereumConfig,
    provider: DynProvider,
    wbth: Address,
    safe: Address,
    /// Relayer EOA that pays gas to submit `execTransaction`. `None` in
    /// watch-only mode (confirmation polling still works).
    relayer: Option<(Address, EthereumWallet)>,
}

impl EthMinter {
    /// Build a minter from configuration. Does not perform network I/O.
    pub fn new(config: EthereumConfig) -> Result<Self, MintError> {
        let wbth: Address = config
            .wbth_contract
            .parse()
            .map_err(|e| MintError::Config(format!("invalid wbth_contract: {}", e)))?;

        let safe: Address = config
            .safe_address
            .as_deref()
            .ok_or_else(|| {
                MintError::Config(
                    "ethereum.safe_address is required for mint submission (ADR 0002)".to_string(),
                )
            })?
            .parse()
            .map_err(|e| MintError::Config(format!("invalid safe_address: {}", e)))?;

        let url = config
            .rpc_url
            .parse()
            .map_err(|e| MintError::Config(format!("invalid ethereum rpc_url: {}", e)))?;
        let provider = ProviderBuilder::new().connect_http(url).erased();

        // TODO(#827): the relayer key file is currently read as plaintext
        // hex; key provisioning/encryption is tracked in the ops runbook
        // issue. The relayer is NOT a custody key — it only pays gas; the
        // mint authority is the Safe threshold (ADR 0002).
        let relayer = match config.private_key_file.as_deref() {
            Some(path) => {
                let raw = std::fs::read_to_string(path).map_err(|e| {
                    MintError::Config(format!("cannot read private_key_file {}: {}", path, e))
                })?;
                let signer: PrivateKeySigner = raw
                    .trim()
                    .parse()
                    .map_err(|e| MintError::Config(format!("invalid relayer key: {}", e)))?;
                let address = signer.address();
                Some((address, EthereumWallet::from(signer)))
            }
            None => None,
        };

        Ok(Self {
            config,
            provider,
            wbth,
            safe,
            relayer,
        })
    }

    /// Read the Safe's current nonce from chain.
    async fn safe_nonce(&self) -> Result<U256, MintError> {
        let call = TransactionRequest::default()
            .with_to(self.safe)
            .with_input(IGnosisSafe::nonceCall {}.abi_encode());
        let ret = self
            .provider
            .call(call)
            .await
            .map_err(|e| MintError::Rpc(format!("safe nonce() call failed: {}", e)))?;
        IGnosisSafe::nonceCall::abi_decode_returns(&ret)
            .map_err(|e| MintError::Rpc(format!("safe nonce() decode failed: {}", e)))
    }

    /// Fetch fee-history-derived EIP-1559 fees per the configured strategy.
    async fn current_fees(&self) -> Result<(u128, u128), MintError> {
        if let GasPriceStrategy::Fixed(gwei) = self.config.gas_price_strategy {
            return Ok(eip1559_fees(GasPriceStrategy::Fixed(gwei), 0, [0, 0, 0]));
        }

        let history = self
            .provider
            .get_fee_history(5, BlockNumberOrTag::Latest, &[10.0, 50.0, 90.0])
            .await
            .map_err(|e| MintError::Rpc(format!("eth_feeHistory failed: {}", e)))?;

        // The last entry of base_fee_per_gas is the NEXT block's base fee.
        let next_base_fee = history
            .base_fee_per_gas
            .last()
            .copied()
            .ok_or_else(|| MintError::Rpc("empty base_fee_per_gas".to_string()))?;

        // Average each percentile column across the sampled blocks.
        let mut tips = [0u128; 3];
        if let Some(rewards) = &history.reward {
            if !rewards.is_empty() {
                for (i, tip) in tips.iter_mut().enumerate() {
                    let sum: u128 = rewards.iter().filter_map(|r| r.get(i)).copied().sum();
                    *tip = sum / rewards.len() as u128;
                }
            }
        }

        Ok(eip1559_fees(
            self.config.gas_price_strategy,
            next_base_fee,
            tips,
        ))
    }
}

#[async_trait]
impl Minter for EthMinter {
    fn chain(&self) -> Chain {
        Chain::Ethereum
    }

    async fn prepare_mint(
        &self,
        order: &BridgeOrder,
        auth: &MintAuthorization,
    ) -> Result<PreparedMint, MintError> {
        let order_id = order.order_id_bytes();

        // The attestation must be bound to THIS order's on-chain id — a
        // replayed authorization for another order is rejected here, and a
        // replay for the same order can only re-produce the same mint.
        if auth.order_id != order_id {
            return Err(MintError::Attestation(
                "attestation order id does not match order".to_string(),
            ));
        }

        let (relayer_addr, wallet) = self.relayer.as_ref().ok_or_else(|| {
            MintError::Config("no relayer key configured (ethereum.private_key_file)".to_string())
        })?;

        let to: Address = order
            .dest_address
            .parse()
            .map_err(|e| MintError::Config(format!("invalid dest_address: {}", e)))?;
        let amount = U256::from(order.net_amount());

        // Inner call: bridgeMint(to, amount, orderId) on wBTH.
        let mint_calldata = encode_bridge_mint_calldata(to, amount, order_id);

        // Safe wrapper: execTransaction with the threshold owner signatures.
        let safe_nonce = self.safe_nonce().await?;

        // Pre-broadcast nonce cross-check (#848): if the Safe's on-chain nonce
        // advanced since the signatures were collected, fail with a distinct
        // retryable error BEFORE persisting or broadcasting any tx, so the
        // engine re-authorizes and re-collects at the fresh nonce.
        check_attested_nonce(auth, safe_nonce)?;

        let signatures = assemble_safe_signatures(auth)?;
        debug!(
            "safe tx hash for order {}: {}",
            order.id,
            safe_tx_hash(
                self.config.chain_id,
                self.safe,
                self.wbth,
                &mint_calldata,
                safe_nonce
            )
        );
        let exec_calldata = IGnosisSafe::execTransactionCall {
            to: self.wbth,
            value: U256::ZERO,
            data: mint_calldata.into(),
            operation: 0,
            safeTxGas: U256::ZERO,
            baseGas: U256::ZERO,
            gasPrice: U256::ZERO,
            gasToken: Address::ZERO,
            refundReceiver: Address::ZERO,
            signatures,
        }
        .abi_encode();

        // Relayer EOA nonce (pending, so sequential submissions chain).
        let relayer_nonce = self
            .provider
            .get_transaction_count(*relayer_addr)
            .pending()
            .await
            .map_err(|e| MintError::Rpc(format!("get_transaction_count failed: {}", e)))?;

        let (max_fee, max_priority_fee) = self.current_fees().await?;

        let mut tx = TransactionRequest::default()
            .with_from(*relayer_addr)
            .with_to(self.safe)
            .with_input(exec_calldata)
            .with_nonce(relayer_nonce)
            .with_chain_id(self.config.chain_id)
            .with_max_fee_per_gas(max_fee)
            .with_max_priority_fee_per_gas(max_priority_fee);

        let gas = self
            .provider
            .estimate_gas(tx.clone())
            .await
            .map_err(|e| MintError::Rpc(format!("eth_estimateGas failed: {}", e)))?;
        tx = tx.with_gas_limit(gas.saturating_mul(GAS_HEADROOM_PERCENT) / 100);

        // Sign locally. The raw bytes are kept so every retry re-broadcasts
        // the SAME transaction (same relayer nonce, same Safe nonce, same
        // order id) — a retry can therefore never double-mint.
        let envelope = tx
            .build(wallet)
            .await
            .map_err(|e| MintError::Config(format!("tx signing failed: {}", e)))?;

        let tx_id = format!("{:#x}", envelope.tx_hash());
        let raw = envelope.encoded_2718();

        info!(
            "prepared ETH mint for order {}: tx {} (safe nonce {}, {} wBTH to {})",
            order.id,
            tx_id,
            safe_nonce,
            order.net_amount(),
            order.dest_address
        );

        Ok(PreparedMint { tx_id, raw })
    }

    async fn broadcast(&self, prepared: &PreparedMint) -> Result<(), MintError> {
        match self.provider.send_raw_transaction(&prepared.raw).await {
            Ok(pending) => {
                debug!("broadcast ETH tx {}", pending.tx_hash());
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                // Idempotent re-broadcast: the node already has (or already
                // mined) this exact transaction.
                if msg.contains("already known")
                    || msg.contains("already imported")
                    || msg.contains("known transaction")
                    || msg.contains("nonce too low")
                {
                    warn!(
                        "ETH tx {} already known to the node ({}); treating as broadcast",
                        prepared.tx_id, msg
                    );
                    Ok(())
                } else {
                    Err(MintError::Rpc(format!(
                        "eth_sendRawTransaction failed: {}",
                        e
                    )))
                }
            }
        }
    }

    async fn check_confirmation(
        &self,
        order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ConfirmationStatus, MintError> {
        let tx_hash: B256 = dest_tx
            .parse()
            .map_err(|e| MintError::Config(format!("invalid dest_tx {}: {}", dest_tx, e)))?;

        let receipt = self
            .provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| MintError::Rpc(format!("get_transaction_receipt failed: {}", e)))?;

        let Some(receipt) = receipt else {
            // No receipt. If the node no longer knows the tx at all it was
            // dropped (or its block reorged out and it was not re-included):
            // unwind and re-submit.
            let known = self
                .provider
                .get_transaction_by_hash(tx_hash)
                .await
                .map_err(|e| MintError::Rpc(format!("get_transaction_by_hash failed: {}", e)))?;
            return Ok(if known.is_some() {
                ConfirmationStatus::Pending { confirmations: 0 }
            } else {
                ConfirmationStatus::Reorged
            });
        };

        if !receipt.status() {
            return Ok(ConfirmationStatus::Failed {
                reason: "execTransaction reverted on-chain".to_string(),
            });
        }

        // The Safe swallows inner-call failures (ExecutionFailure) without
        // reverting, so a successful receipt is not enough: require the
        // BridgeMint event bound to this exact order id.
        if !find_bridge_mint_event(receipt.inner.logs(), self.wbth, order.order_id_bytes()) {
            return Ok(ConfirmationStatus::Failed {
                reason: "tx executed but no BridgeMint event for this order id \
                         (Safe inner call failed?)"
                    .to_string(),
            });
        }

        let Some(block_number) = receipt.block_number else {
            return Ok(ConfirmationStatus::Pending { confirmations: 0 });
        };

        let current = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| MintError::Rpc(format!("get_block_number failed: {}", e)))?;

        let confirmations = current.saturating_sub(block_number) + 1;
        if confirmations < self.config.confirmations_required as u64 {
            return Ok(ConfirmationStatus::Pending { confirmations });
        }

        // Depth reached — verify the receipt's block is still canonical
        // before declaring finality.
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number))
            .await
            .map_err(|e| MintError::Rpc(format!("get_block_by_number failed: {}", e)))?;

        match (block, receipt.block_hash) {
            (Some(block), Some(receipt_hash)) if block.header.hash == receipt_hash => {
                Ok(ConfirmationStatus::Confirmed)
            }
            _ => Ok(ConfirmationStatus::Reorged),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::LogData;
    use bth_bridge_core::AttestationSignature;

    fn addr(byte: u8) -> Address {
        Address::from([byte; 20])
    }

    #[test]
    fn test_bridge_mint_calldata_roundtrip() {
        let to = addr(0x11);
        let amount = U256::from(999_000_000_000u64);
        let order_id = [7u8; 32];

        let calldata = encode_bridge_mint_calldata(to, amount, order_id);

        // Selector for bridgeMint(address,uint256,bytes32).
        assert_eq!(&calldata[..4], IWrappedBTH::bridgeMintCall::SELECTOR);

        let decoded = IWrappedBTH::bridgeMintCall::abi_decode(&calldata).unwrap();
        assert_eq!(decoded.to, to);
        assert_eq!(decoded.amount, amount);
        assert_eq!(decoded.orderId, B256::from(order_id));
    }

    #[test]
    fn test_safe_signature_assembly_sorted_by_owner() {
        let sig = |owner: u8, marker: u8| AttestationSignature {
            signer: vec![owner; 20],
            signature: vec![marker; 65],
        };
        // Provided out of order — must be emitted ascending by owner.
        let auth = MintAuthorization {
            order_id: [0u8; 32],
            scheme: SignatureScheme::Secp256k1,
            threshold: 2,
            signatures: vec![sig(0xBB, 2), sig(0xAA, 1)],
            safe_nonce: Some(0),
        };

        let blob = assemble_safe_signatures(&auth).unwrap();
        assert_eq!(blob.len(), 130);
        assert_eq!(blob[0], 1, "lower owner address must come first");
        assert_eq!(blob[65], 2);
    }

    #[test]
    fn test_safe_signature_assembly_rejects_below_threshold() {
        let auth = MintAuthorization {
            order_id: [0u8; 32],
            scheme: SignatureScheme::Secp256k1,
            threshold: 2,
            signatures: vec![AttestationSignature {
                signer: vec![1u8; 20],
                signature: vec![0u8; 65],
            }],
            safe_nonce: Some(0),
        };
        assert!(matches!(
            assemble_safe_signatures(&auth),
            Err(MintError::Attestation(_))
        ));
    }

    #[test]
    fn test_check_attested_nonce_detects_mismatch_pre_broadcast() {
        let auth = MintAuthorization {
            order_id: [0u8; 32],
            scheme: SignatureScheme::Secp256k1,
            threshold: 1,
            signatures: vec![AttestationSignature {
                signer: vec![1u8; 20],
                signature: vec![0u8; 65],
            }],
            safe_nonce: Some(7),
        };

        // Matching nonce: proceed.
        assert!(check_attested_nonce(&auth, U256::from(7u64)).is_ok());

        // Safe nonce advanced past the attested nonce: distinct stale-nonce
        // error, so the engine re-authorizes instead of broadcasting.
        let err = check_attested_nonce(&auth, U256::from(8u64)).unwrap_err();
        assert!(matches!(err, MintError::StaleNonce(_)), "{err:?}");

        // Authorizations without a Safe nonce (Solana / legacy) never trip the
        // check.
        let no_nonce = MintAuthorization {
            safe_nonce: None,
            ..auth
        };
        assert!(check_attested_nonce(&no_nonce, U256::from(999u64)).is_ok());
    }

    #[test]
    fn test_safe_signature_assembly_rejects_wrong_scheme() {
        let auth = MintAuthorization {
            order_id: [0u8; 32],
            scheme: SignatureScheme::Ed25519,
            threshold: 0,
            signatures: vec![],
            safe_nonce: None,
        };
        assert!(matches!(
            assemble_safe_signatures(&auth),
            Err(MintError::Attestation(_))
        ));
    }

    #[test]
    fn test_gas_strategy_mapping() {
        let base = 10_000_000_000u128; // 10 gwei
        let tips = [
            1_000_000_000u128, // p10
            2_000_000_000,     // p50
            5_000_000_000,     // p90
        ];

        let (max_low, tip_low) = eip1559_fees(GasPriceStrategy::Low, base, tips);
        assert_eq!(tip_low, tips[0]);
        assert_eq!(max_low, base * 2 + tips[0]);

        let (max_med, tip_med) = eip1559_fees(GasPriceStrategy::Medium, base, tips);
        assert_eq!(tip_med, tips[1]);
        assert_eq!(max_med, base * 2 + tips[1]);

        let (max_high, tip_high) = eip1559_fees(GasPriceStrategy::High, base, tips);
        assert_eq!(tip_high, tips[2]);
        assert_eq!(max_high, base * 2 + tips[2]);

        // Fixed maps gwei -> wei on both fields (legacy-style price).
        let (max_fixed, tip_fixed) = eip1559_fees(GasPriceStrategy::Fixed(30), base, tips);
        assert_eq!(max_fixed, 30_000_000_000);
        assert_eq!(tip_fixed, 30_000_000_000);

        // Tip is floored so the tx is never tip-less.
        let (_, tip_floor) = eip1559_fees(GasPriceStrategy::Low, base, [0, 0, 0]);
        assert_eq!(tip_floor, MIN_PRIORITY_FEE_WEI);
    }

    #[test]
    fn test_safe_tx_hash_binds_all_inputs() {
        let calldata = encode_bridge_mint_calldata(addr(1), U256::from(5u8), [9u8; 32]);
        let h = |chain: u64, nonce: u64| {
            safe_tx_hash(chain, addr(0x5A), addr(0xEE), &calldata, U256::from(nonce))
        };

        // Deterministic.
        assert_eq!(h(1, 0), h(1, 0));
        // Changes with chain id and Safe nonce (replay protection).
        assert_ne!(h(1, 0), h(5, 0));
        assert_ne!(h(1, 0), h(1, 1));
        // Changes with calldata (order id binding).
        let other = encode_bridge_mint_calldata(addr(1), U256::from(5u8), [8u8; 32]);
        assert_ne!(
            safe_tx_hash(1, addr(2), addr(3), &calldata, U256::ZERO),
            safe_tx_hash(1, addr(2), addr(3), &other, U256::ZERO)
        );
    }

    fn mint_log(contract: Address, to: Address, order_id: [u8; 32]) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: contract,
                data: LogData::new_unchecked(
                    vec![
                        IWrappedBTH::BridgeMint::SIGNATURE_HASH,
                        B256::left_padding_from(to.as_slice()),
                        B256::from(order_id),
                    ],
                    U256::from(1u8).to_be_bytes_vec().into(),
                ),
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_find_bridge_mint_event_matches_order_id() {
        let wbth = addr(0xEE);
        let order_id = [3u8; 32];

        let logs = vec![mint_log(wbth, addr(1), order_id)];
        assert!(find_bridge_mint_event(&logs, wbth, order_id));

        // Wrong order id: a mint for a DIFFERENT order must not confirm
        // this one.
        assert!(!find_bridge_mint_event(&logs, wbth, [4u8; 32]));

        // Right event shape but wrong emitting contract.
        let spoofed = vec![mint_log(addr(0xDD), addr(1), order_id)];
        assert!(!find_bridge_mint_event(&spoofed, wbth, order_id));
    }
}
