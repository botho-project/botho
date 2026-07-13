// Copyright (c) 2024 The Botho Foundation

//! Ethereum fork tests (#828): the REAL Rust mint pipeline against a live
//! local Ethereum node with a deployed `WrappedBTH` and a Gnosis-Safe-
//! compatible threshold multisig (`contracts/ethereum/contracts/test/
//! SafeStub.sol`).
//!
//! Unlike the mocked unit tests, nothing is stubbed here: the
//! [`FederationAttestationProvider`] collects real secp256k1 owner
//! signatures over the EIP-712 SafeTx digest (reading the Safe nonce over
//! JSON-RPC), [`EthMinter`] wraps them in `Safe.execTransaction`, signs
//! with the relayer key, broadcasts, and polls confirmation; the burn leg
//! is detected by [`AlloyEthClient`] exactly as the watcher would.
//!
//! ## Running
//!
//! The live tests are `#[ignore]`d — they need a local node and compiled
//! contract artifacts. Easiest:
//!
//! ```text
//! ./scripts/bridge-e2e-local.sh
//! ```
//!
//! or manually:
//!
//! ```text
//! (cd contracts/ethereum && npm install && npx hardhat compile && npx hardhat node &)
//! cargo test -p bth-bridge-service -- --ignored fork_
//! ```
//!
//! `BRIDGE_FORK_RPC_URL` overrides the RPC endpoint (default
//! `http://127.0.0.1:8545`); both `npx hardhat node` and `anvil` work —
//! they share chain id 31337 and the standard funded dev accounts.

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{SolCall, SolValue},
};
use bth_bridge_core::{
    attestation::AttestationKind, BridgeConfig, BridgeOrder, Chain, GasPriceStrategy, OrderStatus,
};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    attestation::{sign_attestation_secp256k1, AttestationProvider, FederationAttestationProvider},
    mint::{
        ethereum::{encode_bridge_mint_calldata, safe_tx_hash, EthMinter},
        ConfirmationStatus, Minter,
    },
    watchers::ethereum::{with_tx_ordinals, AlloyEthClient, EthChainClient},
};

/// Cross-language EIP-712 pin, shared with
/// `contracts/ethereum/test/BridgeFlow.test.ts` ("matches the Rust
/// safe_tx_hash vector") which computes the same digest with ethers'
/// `TypedDataEncoder`, and with `SafeStub.getTransactionHash` on-chain. If
/// the Rust `SafeTx` struct, domain, or calldata encoding ever drifts from
/// the Solidity side, Rust-signed attestations stop verifying on-chain —
/// this vector turns that drift into a red test. Runs in every `cargo
/// test` pass (not ignored).
#[test]
fn test_safe_tx_digest_cross_language_vector() {
    let to: Address = "0x1111111111111111111111111111111111111111"
        .parse()
        .unwrap();
    let calldata = encode_bridge_mint_calldata(
        to,
        U256::from(5_000_000_000_000u64), // 5 BTH in picocredits
        [0x22u8; 32],
    );
    let digest = safe_tx_hash(
        31337,
        "0x0000000000000000000000000000000000005afe"
            .parse()
            .unwrap(),
        "0x00000000000000000000000000000000000b0170"
            .parse()
            .unwrap(),
        &calldata,
        U256::from(7u64),
    );
    assert_eq!(
        format!("{:#x}", digest),
        "0x5e70bedc7f0afce2208fd231d402628090aa65b017c3b0bd9d5aa0382197c4c3",
        "SafeTx digest drifted from the pinned cross-language vector"
    );
}

sol! {
    #[allow(missing_docs)]
    interface IWrappedBTHView {
        function balanceOf(address account) external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function processedOrders(bytes32 orderId) external view returns (bool);
        function bridgeBurn(uint256 amount, string calldata bthAddress) external;
    }
}

/// Well-known dev accounts of `npx hardhat node` / `anvil` (mnemonic
/// "test test test test test test test test test test test junk"). These
/// hold test ETH on chain 31337 only — they are not secrets.
const DEV_KEYS: [&str; 4] = [
    // 0: contract deployer + relayer EOA (pays gas, holds no authority)
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    // 1: Safe owner 1 (local attestation signer)
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    // 2: Safe owner 2 (peer federation member, envelope injected)
    "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    // 3: the bridging user (mint recipient / burner)
    "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
];

fn rpc_url() -> String {
    std::env::var("BRIDGE_FORK_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string())
}

fn dev_signer(index: usize) -> PrivateKeySigner {
    DEV_KEYS[index].parse().expect("valid dev key")
}

/// Read a Hardhat artifact's creation bytecode.
fn artifact_bytecode(rel_path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/ethereum/artifacts")
        .join(rel_path);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read contract artifact {} ({}); run \
             `npm install && npx hardhat compile` in contracts/ethereum \
             first (see scripts/bridge-e2e-local.sh)",
            path.display(),
            e
        )
    });
    let json: serde_json::Value = serde_json::from_str(&raw).expect("artifact is JSON");
    let hex_code = json["bytecode"].as_str().expect("artifact has bytecode");
    hex::decode(hex_code.trim_start_matches("0x")).expect("bytecode is hex")
}

/// Deploy a contract (creation bytecode + ABI-encoded constructor args).
async fn deploy(provider: &DynProvider, bytecode: Vec<u8>, ctor_args: Vec<u8>) -> Address {
    let mut code = bytecode;
    code.extend_from_slice(&ctor_args);
    let tx = TransactionRequest::default().with_deploy_code(code);
    let receipt = provider
        .send_transaction(tx)
        .await
        .expect("deploy tx accepted")
        .get_receipt()
        .await
        .expect("deploy tx mined");
    assert!(receipt.status(), "deploy reverted");
    receipt
        .contract_address
        .expect("deploy receipt has address")
}

async fn call_u256(provider: &DynProvider, to: Address, input: Vec<u8>) -> U256 {
    let ret = provider
        .call(TransactionRequest::default().with_to(to).with_input(input))
        .await
        .expect("eth_call succeeds");
    U256::abi_decode(&ret).expect("uint256 return")
}

/// The full BTH->wBTH->burn Ethereum leg through the production Rust
/// pipeline: 2-of-2 federation attestation (one local signer, one injected
/// peer envelope), Safe-wrapped mint, confirmation polling, idempotent
/// re-broadcast, then the user's redemption burn detected by the watcher
/// transport. Requires a local node — see the module docs.
#[tokio::test]
#[ignore = "requires a local Ethereum node (hardhat/anvil) and compiled artifacts; run scripts/bridge-e2e-local.sh"]
async fn fork_eth_mint_and_burn_round_trip() {
    let url: alloy::transports::http::reqwest::Url = rpc_url().parse().expect("valid RPC url");

    let deployer = dev_signer(0);
    let owner1 = dev_signer(1);
    let owner2 = dev_signer(2);
    let user = dev_signer(3);
    let user_addr = user.address();

    let deploy_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(deployer.clone()))
        .connect_http(url.clone())
        .erased();

    let chain_id = deploy_provider.get_chain_id().await.expect("node is up");
    assert_eq!(chain_id, 31337, "expected a hardhat/anvil dev chain");

    // ---- Deploy the 2-of-2 validator Safe and the token --------------
    let mut owners = vec![owner1.address(), owner2.address()];
    owners.sort();
    let safe_addr = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/test/SafeStub.sol/SafeStub.json"),
        (owners.clone(), U256::from(2u8)).abi_encode_params(),
    )
    .await;
    let wbth_addr = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/WrappedBTH.sol/WrappedBTH.json"),
        // admin / MINTER (the Safe!) / pauser
        (deployer.address(), safe_addr, deployer.address()).abi_encode_params(),
    )
    .await;

    // ---- Bridge configuration (as a real deployment would set it) ----
    let dir = tempfile::tempdir().expect("tempdir");
    let relayer_key_path = dir.path().join("relayer.hex");
    std::fs::write(&relayer_key_path, DEV_KEYS[0]).unwrap();
    let owner1_key_path = dir.path().join("owner1.hex");
    std::fs::write(&owner1_key_path, DEV_KEYS[1]).unwrap();

    let mut config = BridgeConfig::default();
    config.ethereum.rpc_url = rpc_url();
    config.ethereum.chain_id = chain_id;
    config.ethereum.wbth_contract = format!("{:#x}", wbth_addr);
    config.ethereum.safe_address = Some(format!("{:#x}", safe_addr));
    config.ethereum.private_key_file = Some(relayer_key_path.to_string_lossy().into_owned());
    config.ethereum.confirmations_required = 1;
    // Fixed gas so the test does not depend on the node's fee-history
    // support (the strategy mapping has its own unit tests).
    config.ethereum.gas_price_strategy = GasPriceStrategy::Fixed(3);
    config.ethereum.mint_signers = vec![
        format!("{:#x}", owner1.address()),
        format!("{:#x}", owner2.address()),
    ];
    config.ethereum.mint_threshold = 2;
    config.bridge.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
    config.bridge.attestation_secp256k1_key_file =
        Some(owner1_key_path.to_string_lossy().into_owned());

    // ---- The order: a confirmed 100 BTH deposit ----------------------
    let amount = 100_000_000_000_000u64; // 100 BTH in picocredits
    let mut order = BridgeOrder::new_mint(
        Chain::Ethereum,
        amount,
        0, // fee accounting is covered by unit tests; keep the peg exact
        "bth_reserve_deposit_address".to_string(),
        format!("{:#x}", user_addr),
    );
    order.source_tx = Some("bth_deposit_tx_fork_test".to_string());
    order
        .try_set_status(OrderStatus::DepositDetected)
        .expect("AwaitingDeposit -> DepositDetected");
    order
        .try_set_status(OrderStatus::DepositConfirmed)
        .expect("DepositDetected -> DepositConfirmed");

    // ---- Federation attestation to threshold (#824 pipeline) ---------
    let provider = FederationAttestationProvider::from_config(&config)
        .expect("valid federation config")
        .expect("federation configured");

    // First pass self-attests with the local owner-1 key but must FAIL
    // the threshold (1/2) — fail-safe until the peer signs.
    let below = provider.authorize_mint(&order).await;
    assert!(
        below.is_err(),
        "threshold 2 must not authorize with one signer"
    );

    // Peer (owner 2) submits its envelope, bound to the same Safe nonce
    // the local signer used (the on-chain nonce, 0 for a fresh Safe).
    let kind = AttestationKind::MintWbth {
        dest_chain: Chain::Ethereum,
        dest_address: order.dest_address.clone(),
        amount: order.net_amount(),
        order_id: order.id,
        source_tx: order.source_tx.clone().unwrap(),
        safe_nonce: Some(0),
    };
    let now = Utc::now().timestamp().max(0) as u64;
    let envelope = sign_attestation_secp256k1(
        &kind,
        &owner2,
        chain_id,
        safe_addr,
        wbth_addr,
        &Uuid::new_v4().simple().to_string(),
        now,
        now + 120,
    )
    .expect("peer envelope signs");
    let outcome = provider.submit_attestation(&envelope, &order);
    assert!(
        outcome.accepted,
        "peer attestation refused: {}",
        outcome.message
    );

    let auth = provider
        .authorize_mint(&order)
        .await
        .expect("threshold met -> authorization");
    assert_eq!(auth.signatures.len(), 2);
    assert_eq!(auth.order_id, order.order_id_bytes());

    // ---- Mint through the Safe (prepare -> broadcast -> confirm) -----
    let minter = EthMinter::new(config.ethereum.clone()).expect("minter builds");
    let prepared = minter.prepare_mint(&order, &auth).await.expect("prepare");
    minter.broadcast(&prepared).await.expect("broadcast");
    order
        .try_set_status(OrderStatus::MintPending)
        .expect("DepositConfirmed -> MintPending");
    order.dest_tx = Some(prepared.tx_id.clone());

    let mut confirmed = false;
    for _ in 0..30 {
        match minter
            .check_confirmation(&order, &prepared.tx_id)
            .await
            .expect("confirmation poll")
        {
            ConfirmationStatus::Confirmed => {
                confirmed = true;
                break;
            }
            ConfirmationStatus::Pending { .. } => {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            other => panic!("mint did not confirm: {:?}", other),
        }
    }
    assert!(confirmed, "mint tx never reached confirmation depth");
    order
        .try_set_status(OrderStatus::Completed)
        .expect("MintPending -> Completed");
    assert!(order.status.is_terminal());

    // Idempotency: re-broadcasting the SAME prepared bytes must succeed
    // as a no-op ("already known"), never a competing mint.
    minter
        .broadcast(&prepared)
        .await
        .expect("re-broadcast is idempotent");

    // ---- On-chain assertions: exact factor-1 peg (ADR 0003) ----------
    let balance = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHView::balanceOfCall { account: user_addr }.abi_encode(),
    )
    .await;
    assert_eq!(
        balance,
        U256::from(order.net_amount()),
        "1 base unit == 1 picocredit"
    );

    let processed = deploy_provider
        .call(
            TransactionRequest::default().with_to(wbth_addr).with_input(
                IWrappedBTHView::processedOrdersCall {
                    orderId: B256::from(order.order_id_bytes()),
                }
                .abi_encode(),
            ),
        )
        .await
        .expect("processedOrders call");
    assert_eq!(U256::abi_decode(&processed).unwrap(), U256::from(1u8));

    // ---- Burn leg: the user redeems, the watcher transport sees it ---
    let user_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(user.clone()))
        .connect_http(url)
        .erased();
    let bth_dest = "bth_declared_destination_re_shielded_per_adr_0004";
    let burn_receipt = user_provider
        .send_transaction(
            TransactionRequest::default().with_to(wbth_addr).with_input(
                IWrappedBTHView::bridgeBurnCall {
                    amount: U256::from(order.net_amount()),
                    bthAddress: bth_dest.to_string(),
                }
                .abi_encode(),
            ),
        )
        .await
        .expect("burn tx accepted")
        .get_receipt()
        .await
        .expect("burn tx mined");
    assert!(burn_receipt.status(), "bridgeBurn reverted");

    // Scan with the SAME transport the Ethereum watcher runs in
    // production (eth_getLogs + decode), not a hand-rolled filter.
    let client = AlloyEthClient::new(&config.ethereum).expect("watcher client");
    let tip = client.latest_block().await.expect("tip");
    let events = client.burn_events(0, tip).await.expect("burn scan");
    let ordered = with_tx_ordinals(events);
    let (event, ordinal) = ordered
        .iter()
        .find(|(e, _)| e.tx_hash == format!("{:#x}", burn_receipt.transaction_hash))
        .expect("watcher sees the burn");
    assert_eq!(
        event.amount,
        order.net_amount(),
        "burn amount is exact picocredits"
    );
    assert_eq!(event.bth_address, bth_dest);
    assert_eq!(event.from, format!("{:#x}", user_addr));
    assert_eq!(*ordinal, 0);

    // The burn order the watcher would create walks its happy path up to
    // the release gate (release construction is live-node work, #856).
    let mut burn_order = BridgeOrder::new_burn(
        Chain::Ethereum,
        event.amount,
        0,
        event.from.clone(),
        event.bth_address.clone(),
        event.tx_hash.clone(),
    );
    assert_eq!(burn_order.status, OrderStatus::BurnDetected);
    burn_order
        .try_set_status(OrderStatus::BurnConfirmed)
        .expect("BurnDetected -> BurnConfirmed");

    // ---- Peg invariant closes: supply returns to zero ----------------
    let supply = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHView::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(supply, U256::ZERO, "sum(mints) - sum(burns) == 0");
}
