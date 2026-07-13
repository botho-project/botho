// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title SafeStub
 * @notice TEST ONLY — a minimal Gnosis-Safe-compatible t-of-n multisig used
 *         by the bridge fork/integration tests (#828). DO NOT DEPLOY to a
 *         real network: production deployments use an audited Gnosis Safe
 *         (ADR 0002).
 *
 * @dev Implements exactly the Safe v1.3 surface the bridge relies on
 *      (`bridge/service/src/mint/ethereum.rs`):
 *      - `nonce()` — the replay counter each SafeTx signature binds to.
 *      - `execTransaction(...)` — verifies `threshold` 65-byte {r,s,v}
 *        owner signatures over the EIP-712 SafeTx digest, concatenated in
 *        strictly ascending owner-address order (mirroring
 *        `assemble_safe_signatures` on the Rust side), then performs the
 *        inner call. Like the real Safe, an inner-call failure does NOT
 *        revert the outer transaction — it emits `ExecutionFailure` — which
 *        is the behavior `EthMinter::check_confirmation` guards against by
 *        requiring the order-bound `BridgeMint` event.
 *
 *      The digest computation (domain typehash with only
 *      `chainId`/`verifyingContract`, SafeTx typehash and field order) is
 *      byte-identical to Gnosis Safe v1.3 and to the Rust `safe_tx_hash`;
 *      the cross-language test vector pinned in
 *      `bridge/service/src/fork_tests.rs` and
 *      `contracts/ethereum/test/BridgeFlow.test.ts` keeps all three in sync.
 */
contract SafeStub {
    // keccak256("EIP712Domain(uint256 chainId,address verifyingContract)")
    bytes32 private constant DOMAIN_SEPARATOR_TYPEHASH =
        keccak256("EIP712Domain(uint256 chainId,address verifyingContract)");

    // keccak256("SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)")
    bytes32 private constant SAFE_TX_TYPEHASH =
        keccak256(
            "SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)"
        );

    /// @notice Safe replay counter; incremented on every executed SafeTx.
    uint256 public nonce;

    /// @notice Number of distinct owner signatures required.
    uint256 public threshold;

    /// @notice Registered owner set.
    mapping(address => bool) public isOwner;

    /// @notice Mirrors Gnosis Safe's execution result events.
    event ExecutionSuccess(bytes32 txHash);
    event ExecutionFailure(bytes32 txHash);

    constructor(address[] memory owners, uint256 _threshold) {
        require(
            _threshold >= 1 && _threshold <= owners.length,
            "SafeStub: bad threshold"
        );
        for (uint256 i = 0; i < owners.length; i++) {
            require(owners[i] != address(0), "SafeStub: zero owner");
            require(!isOwner[owners[i]], "SafeStub: duplicate owner");
            isOwner[owners[i]] = true;
        }
        threshold = _threshold;
    }

    /// @notice EIP-712 domain separator (chainId + this contract only,
    ///         exactly like Gnosis Safe v1.3).
    function domainSeparator() public view returns (bytes32) {
        return
            keccak256(
                abi.encode(DOMAIN_SEPARATOR_TYPEHASH, block.chainid, address(this))
            );
    }

    /// @notice The digest owners must sign for the given SafeTx fields.
    function getTransactionHash(
        address to,
        uint256 value,
        bytes calldata data,
        uint8 operation,
        uint256 safeTxGas,
        uint256 baseGas,
        uint256 gasPrice,
        address gasToken,
        address refundReceiver,
        uint256 _nonce
    ) public view returns (bytes32) {
        bytes32 safeTxHash = keccak256(
            abi.encode(
                SAFE_TX_TYPEHASH,
                to,
                value,
                keccak256(data),
                operation,
                safeTxGas,
                baseGas,
                gasPrice,
                gasToken,
                refundReceiver,
                _nonce
            )
        );
        return
            keccak256(
                abi.encodePacked(bytes1(0x19), bytes1(0x01), domainSeparator(), safeTxHash)
            );
    }

    /**
     * @notice Verify threshold owner signatures over the current-nonce
     *         SafeTx digest, then perform the inner call.
     * @dev The nonce is consumed BEFORE the call (like the real Safe), so a
     *      failed inner call still burns the signatures — re-submission
     *      requires signing at the new nonce.
     */
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
    ) external payable returns (bool success) {
        require(operation == 0, "SafeStub: only CALL supported");
        bytes32 txHash = getTransactionHash(
            to,
            value,
            data,
            operation,
            safeTxGas,
            baseGas,
            gasPrice,
            gasToken,
            refundReceiver,
            nonce
        );
        checkSignatures(txHash, signatures);
        nonce++;

        (success, ) = to.call{value: value}(data);
        if (success) {
            emit ExecutionSuccess(txHash);
        } else {
            emit ExecutionFailure(txHash);
        }
    }

    /**
     * @notice Gnosis-style signature check: `threshold` 65-byte {r,s,v}
     *         signatures, recovered owners strictly ascending (which also
     *         enforces distinctness and rejects ecrecover failures, since
     *         address(0) can never be strictly greater than the previous).
     */
    function checkSignatures(bytes32 dataHash, bytes memory signatures) public view {
        require(
            signatures.length >= threshold * 65,
            "SafeStub: signatures too short"
        );
        address lastOwner = address(0);
        for (uint256 i = 0; i < threshold; i++) {
            (uint8 v, bytes32 r, bytes32 s) = signatureSplit(signatures, i);
            address currentOwner = ecrecover(dataHash, v, r, s);
            require(currentOwner > lastOwner, "SafeStub: owners not ascending");
            require(isOwner[currentOwner], "SafeStub: not an owner");
            lastOwner = currentOwner;
        }
    }

    /// @dev Extract {r,s,v} of the `pos`-th 65-byte signature.
    function signatureSplit(
        bytes memory signatures,
        uint256 pos
    ) internal pure returns (uint8 v, bytes32 r, bytes32 s) {
        // Identical to Gnosis Safe's SignatureDecoder: v is the LAST byte
        // of the word loaded at offset 0x41 into this signature.
        // solhint-disable-next-line no-inline-assembly
        assembly {
            let signaturePos := mul(0x41, pos)
            r := mload(add(signatures, add(signaturePos, 0x20)))
            s := mload(add(signatures, add(signaturePos, 0x40)))
            v := and(mload(add(signatures, add(signaturePos, 0x41))), 0xff)
        }
    }
}
