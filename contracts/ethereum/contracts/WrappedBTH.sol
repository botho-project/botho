// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/access/AccessControl.sol";
import "@openzeppelin/contracts/utils/Pausable.sol";

/**
 * @title WrappedBTH
 * @dev Wrapped BTH (wBTH) ERC-20 token for bridging BTH to Ethereum.
 *
 * Key features:
 * - 12 decimals (matches BTH's picocredits base unit)
 * - Minting controlled by bridge operator (MINTER_ROLE)
 * - Users can burn to redeem BTH on the native chain
 * - Rate limiting for security (daily caps, per-tx limits)
 * - Pausable for emergency situations
 */
contract WrappedBTH is ERC20, ERC20Burnable, AccessControl, Pausable {
    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");
    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");

    // BTH has 12 decimals (picocredits), we match this for 1:1 mapping
    uint8 private constant DECIMALS = 12;

    // Rate limiting configuration
    uint256 public mintCooldown = 1 minutes;
    uint256 public maxMintPerTx = 1_000_000 * 10 ** 12; // 1M BTH
    uint256 public dailyMintLimit = 10_000_000 * 10 ** 12; // 10M BTH

    // Rate limiting state
    mapping(address => uint256) public lastMintTime;
    uint256 public dailyMinted;
    uint256 public lastResetDay;

    // Events for bridge monitoring
    event BridgeMint(
        address indexed to,
        uint256 amount,
        bytes32 indexed bthTxHash
    );

    event BridgeBurn(
        address indexed from,
        uint256 amount,
        string bthAddress
    );

    event RateLimitUpdated(
        uint256 mintCooldown,
        uint256 maxMintPerTx,
        uint256 dailyMintLimit
    );

    constructor() ERC20("Wrapped BTH", "wBTH") {
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(MINTER_ROLE, msg.sender);
        _grantRole(PAUSER_ROLE, msg.sender);
        lastResetDay = block.timestamp / 1 days;
    }

    /**
     * @dev Returns the number of decimals (12 to match BTH).
     */
    function decimals() public pure override returns (uint8) {
        return DECIMALS;
    }

    /**
     * @dev Mint wBTH when BTH is deposited to the bridge.
     * @param to Recipient address
     * @param amount Amount in picocredits (12 decimals)
     * @param bthTxHash BTH transaction hash for tracking
     */
    function bridgeMint(
        address to,
        uint256 amount,
        bytes32 bthTxHash
    ) external onlyRole(MINTER_ROLE) whenNotPaused {
        require(to != address(0), "Invalid recipient");
        require(amount > 0, "Amount must be positive");
        require(amount <= maxMintPerTx, "Exceeds max mint per tx");
        require(
            block.timestamp >= lastMintTime[to] + mintCooldown,
            "Cooldown active"
        );

        // Reset daily limit if new day
        uint256 today = block.timestamp / 1 days;
        if (today > lastResetDay) {
            dailyMinted = 0;
            lastResetDay = today;
        }

        require(
            dailyMinted + amount <= dailyMintLimit,
            "Daily limit exceeded"
        );

        lastMintTime[to] = block.timestamp;
        dailyMinted += amount;

        _mint(to, amount);
        emit BridgeMint(to, amount, bthTxHash);
    }

    /**
     * @dev Burn wBTH to receive BTH on the native chain.
     * @param amount Amount to burn (in picocredits)
     * @param bthAddress BTH address to receive funds (stealth address string)
     */
    function bridgeBurn(
        uint256 amount,
        string calldata bthAddress
    ) external whenNotPaused {
        require(amount > 0, "Amount must be positive");
        require(bytes(bthAddress).length > 0, "Invalid BTH address");

        _burn(msg.sender, amount);
        emit BridgeBurn(msg.sender, amount, bthAddress);
    }

    /**
     * @dev Pause all bridge operations.
     */
    function pause() external onlyRole(PAUSER_ROLE) {
        _pause();
    }

    /**
     * @dev Unpause bridge operations.
     */
    function unpause() external onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    /**
     * @dev Update the mint cooldown period.
     * @param _cooldown New cooldown in seconds
     */
    function setMintCooldown(
        uint256 _cooldown
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        mintCooldown = _cooldown;
        emit RateLimitUpdated(mintCooldown, maxMintPerTx, dailyMintLimit);
    }

    /**
     * @dev Update the maximum mint per transaction.
     * @param _max New maximum in picocredits
     */
    function setMaxMintPerTx(
        uint256 _max
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        maxMintPerTx = _max;
        emit RateLimitUpdated(mintCooldown, maxMintPerTx, dailyMintLimit);
    }

    /**
     * @dev Update the daily mint limit.
     * @param _limit New daily limit in picocredits
     */
    function setDailyMintLimit(
        uint256 _limit
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        dailyMintLimit = _limit;
        emit RateLimitUpdated(mintCooldown, maxMintPerTx, dailyMintLimit);
    }

    /**
     * @dev Returns the remaining daily mint capacity.
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

    /**
     * @dev Returns the time until an address can mint again.
     */
    function cooldownRemaining(address account) external view returns (uint256) {
        uint256 nextMintTime = lastMintTime[account] + mintCooldown;
        if (block.timestamp >= nextMintTime) {
            return 0;
        }
        return nextMintTime - block.timestamp;
    }
}
