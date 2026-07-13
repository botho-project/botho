// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/access/AccessControl.sol";
import "@openzeppelin/contracts/utils/Pausable.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/**
 * @title WrappedBTH
 * @notice Wrapped BTH (wBTH) ERC-20 token for bridging BTH to Ethereum.
 *
 * @dev Key properties:
 * - 12 decimals: 1 wBTH base unit == 1 picocredit, 1:1 with native BTH.
 *   All amounts (including `maxMintPerTx` / `dailyMintLimit`) are raw
 *   picocredits with no additional scaling anywhere across the bridge
 *   boundary.
 * - Minting is order-bound and replay-proof: every `bridgeMint` consumes a
 *   unique `orderId` (the bridge order id from the attestation protocol,
 *   see #824) recorded in `processedOrders`. A duplicate order id reverts,
 *   closing the double-mint window even if the off-chain service retries
 *   or is compromised into re-submitting an authorization.
 * - Burning to redeem native BTH is ONLY possible via `bridgeBurn`, which
 *   emits the `BridgeBurn` event the bridge watchers rely on. The open
 *   ERC20Burnable surface (`burn`/`burnFrom`) was deliberately removed: a
 *   plain burn destroys wBTH without a redemption destination, silently
 *   stranding the corresponding locked BTH reserve and breaking the
 *   watcher-tracked supply invariant (totalSupply == sum(mints) - sum(burns
 *   with a BridgeBurn event)).
 *
 * ## Custody (ADR 0002)
 * Per ADR 0002 (docs/decisions/0002-bridge-custody-scp-validator-federation.md)
 * the mint authority is the SCP validator federation operating a Gnosis Safe
 * whose owners are the validators' secp256k1 keys:
 * - `MINTER_ROLE` is granted at deployment to the validator Gnosis Safe
 *   (t-of-n, with t no lower than the SCP safety threshold). Threshold
 *   enforcement lives in the Safe, NOT in this contract — no single EOA can
 *   ever hold a mint path in the deployed configuration.
 * - `DEFAULT_ADMIN_ROLE` (rate-limit / breaker configuration, role
 *   administration) is granted to a SEPARATE governance Safe so that
 *   parameter changes and minting cannot be authorized by the same quorum.
 * - `PAUSER_ROLE` is granted to a guardian Safe that may use a LOWER
 *   threshold than the mint quorum: pausing is a fail-safe action (it can
 *   only halt the bridge, never move funds), so fast incident response is
 *   preferred over a high bar.
 * The deployer receives NO roles. Safe addresses and thresholds for each
 * deployment are recorded in contracts/ethereum/README.md.
 *
 * ## Upgradeability: immutable (no proxy)
 * This contract is deliberately NOT upgradeable. Rationale (mirrors the
 * project's no-hard-forks / minimal-trust posture):
 * - A proxy admin key is a rug vector: whoever can upgrade can mint or
 *   freeze at will, which negates the Safe-threshold custody model.
 * - The contract's job is small and stable (mint/burn/limits/pause); bugs
 *   are handled by pausing, deploying a corrected token, and migrating
 *   balances via the bridge itself (burn on old, mint on new).
 * The trade-off (redeployment instead of in-place fixes) is accepted and
 * documented in contracts/ethereum/README.md.
 *
 * ## Reentrancy
 * Neither `_mint` nor `_burn` in OpenZeppelin ERC-20 v5 performs external
 * calls (no ERC-777 style hooks), so there is no reentrant path today.
 * Defense-in-depth is still applied: strict checks-effects-interactions
 * ordering (all rate-limit and replay-guard state is written BEFORE
 * `_mint`/`_burn`) plus OpenZeppelin `ReentrancyGuard` on both bridge
 * entrypoints, so the properties survive future modifications.
 *
 * ## Rate limiting & circuit breaker
 * - `maxMintPerTx` caps any single mint.
 * - `dailyMintLimit` caps mints per UTC day (day = block.timestamp / 1 days;
 *   the counter lazily resets on the first mint of a later day, which is
 *   correct across multi-day gaps).
 * - Auto-pause breaker: when cumulative daily mints reach
 *   `autoPauseThreshold`, the contract pauses itself (the triggering mint
 *   still succeeds — it is within the daily limit) and requires the
 *   guardian to investigate and unpause. This converts "anomalous volume"
 *   from a soft revert into a hard stop. Set to 0 to disable; defaults to
 *   `dailyMintLimit`.
 * - The previous per-recipient mint cooldown was REMOVED: an attacker
 *   trivially bypasses it by rotating recipient addresses, while it griefs
 *   honest throughput (two legitimate orders to the same recipient within
 *   the window would fail). It provided no security beyond the per-tx and
 *   daily caps above.
 */
contract WrappedBTH is ERC20, AccessControl, Pausable, ReentrancyGuard {
    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");
    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");

    /// @dev BTH has 12 decimals (picocredits); wBTH matches for a 1:1 peg.
    uint8 private constant DECIMALS = 12;

    // ------------------------------------------------------------------
    // Rate limiting / circuit breaker configuration (picocredits)
    // ------------------------------------------------------------------

    /// @notice Maximum amount a single `bridgeMint` may mint.
    uint256 public maxMintPerTx = 1_000_000 * 10 ** 12; // 1M BTH

    /// @notice Maximum cumulative amount mintable per UTC day.
    uint256 public dailyMintLimit = 10_000_000 * 10 ** 12; // 10M BTH

    /// @notice When `dailyMinted` reaches this value the contract
    ///         auto-pauses (0 disables the breaker).
    uint256 public autoPauseThreshold = 10_000_000 * 10 ** 12;

    // ------------------------------------------------------------------
    // Rate limiting / replay-guard state
    // ------------------------------------------------------------------

    /// @notice Cumulative amount minted during the current UTC day.
    uint256 public dailyMinted;

    /// @notice UTC day index (block.timestamp / 1 days) of the last reset.
    uint256 public lastResetDay;

    /// @notice Replay guard: order ids that have already been minted.
    mapping(bytes32 => bool) public processedOrders;

    // ------------------------------------------------------------------
    // Events (bridge watchers bind these signatures — do not change them
    // without updating bridge/service/src/mint/ethereum.rs and
    // bridge/service/src/watchers/ethereum.rs)
    // ------------------------------------------------------------------

    /// @notice Emitted on every successful mint, bound to the bridge order.
    /// @dev Signature `BridgeMint(address,uint256,bytes32)` with `to` and
    ///      `orderId` indexed is relied upon by the mint confirmation logic.
    event BridgeMint(address indexed to, uint256 amount, bytes32 indexed orderId);

    /// @notice Emitted on every burn-for-redemption; `bthAddress` is the
    ///         native-chain destination (resolved to a fresh one-time
    ///         stealth address by the bridge, per ADR 0004).
    event BridgeBurn(address indexed from, uint256 amount, string bthAddress);

    /// @notice Emitted whenever a limit or the breaker threshold changes.
    event RateLimitUpdated(
        uint256 maxMintPerTx,
        uint256 dailyMintLimit,
        uint256 autoPauseThreshold
    );

    /// @notice Emitted when the auto-pause circuit breaker trips.
    event AutoPaused(uint256 dailyMinted, uint256 threshold);

    /**
     * @param adminSafe  Governance Safe: role administration + limit/breaker
     *                   configuration (`DEFAULT_ADMIN_ROLE`).
     * @param minterSafe Validator Gnosis Safe (t-of-n secp256k1, ADR 0002):
     *                   the ONLY mint authority (`MINTER_ROLE`).
     * @param pauserSafe Guardian Safe (may be lower-threshold for fast
     *                   incident response): pause/unpause (`PAUSER_ROLE`).
     * @dev The deployer receives no roles; distinct Safes are strongly
     *      recommended so configuration, minting and pausing require
     *      different quorums.
     */
    constructor(
        address adminSafe,
        address minterSafe,
        address pauserSafe
    ) ERC20("Wrapped BTH", "wBTH") {
        require(adminSafe != address(0), "Invalid admin");
        require(minterSafe != address(0), "Invalid minter");
        require(pauserSafe != address(0), "Invalid pauser");

        _grantRole(DEFAULT_ADMIN_ROLE, adminSafe);
        _grantRole(MINTER_ROLE, minterSafe);
        _grantRole(PAUSER_ROLE, pauserSafe);
        lastResetDay = block.timestamp / 1 days;
    }

    /**
     * @notice Returns 12 — one wBTH base unit is one picocredit (1:1 BTH).
     */
    function decimals() public pure override returns (uint8) {
        return DECIMALS;
    }

    /**
     * @notice Mint wBTH for a locked-BTH bridge order.
     * @dev Only callable by the validator Safe (`MINTER_ROLE`). Replay-proof:
     *      each `orderId` can be minted exactly once. All state (replay
     *      guard, daily accounting) is written before `_mint`
     *      (checks-effects-interactions).
     * @param to Recipient address.
     * @param amount Amount in picocredits (12 decimals), no extra scaling.
     * @param orderId Unique bridge order id (attestation-bound, see #824);
     *        the on-chain idempotency key.
     */
    function bridgeMint(
        address to,
        uint256 amount,
        bytes32 orderId
    ) external onlyRole(MINTER_ROLE) whenNotPaused nonReentrant {
        require(to != address(0), "Invalid recipient");
        require(amount > 0, "Amount must be positive");
        require(amount <= maxMintPerTx, "Exceeds max mint per tx");
        require(orderId != bytes32(0), "Invalid order id");
        require(!processedOrders[orderId], "Order already processed");

        // Lazily reset the daily counter on the first mint of a later day
        // (strictly-greater comparison, so multi-day gaps reset correctly).
        uint256 today = block.timestamp / 1 days;
        if (today > lastResetDay) {
            dailyMinted = 0;
            lastResetDay = today;
        }

        require(dailyMinted + amount <= dailyMintLimit, "Daily limit exceeded");

        // Effects before interaction.
        processedOrders[orderId] = true;
        dailyMinted += amount;

        _mint(to, amount);
        emit BridgeMint(to, amount, orderId);

        // Circuit breaker: anomalous cumulative volume halts the bridge
        // instead of merely reverting subsequent mints, forcing a human
        // (guardian Safe) to investigate before minting resumes.
        if (autoPauseThreshold != 0 && dailyMinted >= autoPauseThreshold) {
            _pause();
            emit AutoPaused(dailyMinted, autoPauseThreshold);
        }
    }

    /**
     * @notice Burn wBTH to redeem native BTH.
     * @dev This is the ONLY burn path (no open `burn`/`burnFrom`); the
     *      emitted `BridgeBurn` event is what authorizes the native-chain
     *      release, so a burn without it would strand reserve funds.
     * @param amount Amount to burn (picocredits).
     * @param bthAddress Destination BTH address (stealth-address string).
     */
    function bridgeBurn(
        uint256 amount,
        string calldata bthAddress
    ) external whenNotPaused nonReentrant {
        require(amount > 0, "Amount must be positive");
        require(bytes(bthAddress).length > 0, "Invalid BTH address");

        _burn(msg.sender, amount);
        emit BridgeBurn(msg.sender, amount, bthAddress);
    }

    /**
     * @notice Pause all bridge operations (mint AND burn).
     */
    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    /**
     * @notice Unpause bridge operations.
     */
    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    /**
     * @notice Update the maximum mint per transaction.
     * @param _max New maximum in picocredits.
     */
    function setMaxMintPerTx(
        uint256 _max
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        maxMintPerTx = _max;
        emit RateLimitUpdated(maxMintPerTx, dailyMintLimit, autoPauseThreshold);
    }

    /**
     * @notice Update the daily mint limit.
     * @param _limit New daily limit in picocredits.
     */
    function setDailyMintLimit(
        uint256 _limit
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        dailyMintLimit = _limit;
        emit RateLimitUpdated(maxMintPerTx, dailyMintLimit, autoPauseThreshold);
    }

    /**
     * @notice Update the auto-pause breaker threshold (0 disables).
     * @param _threshold New threshold in picocredits.
     */
    function setAutoPauseThreshold(
        uint256 _threshold
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        autoPauseThreshold = _threshold;
        emit RateLimitUpdated(maxMintPerTx, dailyMintLimit, autoPauseThreshold);
    }

    /**
     * @notice Remaining mint capacity for the current UTC day.
     */
    function remainingDailyMint() external view returns (uint256) {
        uint256 today = block.timestamp / 1 days;
        if (today > lastResetDay) {
            return dailyMintLimit;
        }
        if (dailyMinted >= dailyMintLimit) {
            return 0;
        }
        return dailyMintLimit - dailyMinted;
    }
}
