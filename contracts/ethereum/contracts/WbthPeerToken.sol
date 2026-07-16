// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Burnable} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

/// @title WbthPeerToken — the HyperEVM-side wBTH for the Wormhole NTT bridge.
/// @notice A burn-and-mint NTT spoke token: the NttManager is the sole `minter`
///         and mints/burns on cross-chain transfers, backed 1:1 by wBTH locked
///         on the Ethereum Sepolia hub. Mirrors Wormhole's `PeerTokenLite` but
///         pins `decimals()` to 12 so a base unit equals one picocredit (parity
///         with the Sepolia wBTH, 0x49b985ec…). NTT trims cross-chain amounts to
///         8 decimals, so transfers are quantized to 1e-8 wBTH (10,000 pc).
contract WbthPeerToken is ERC20, ERC20Burnable, Ownable {
    error CallerNotMinter(address caller);
    error InvalidMinterZeroAddress();

    event NewMinter(address newMinter);

    address public minter;

    modifier onlyMinter() {
        if (msg.sender != minter) revert CallerNotMinter(msg.sender);
        _;
    }

    constructor(string memory _name, string memory _symbol, address _minter, address _owner)
        ERC20(_name, _symbol)
        Ownable(_owner)
    {
        minter = _minter;
    }

    /// @notice 12 decimals — 1 base unit == 1 picocredit (1:1 with Sepolia wBTH).
    function decimals() public pure override returns (uint8) {
        return 12;
    }

    function mint(address _account, uint256 _amount) external onlyMinter {
        _mint(_account, _amount);
    }

    /// @notice Rotate the minter (e.g. to the NttManager after it is deployed).
    function setMinter(address newMinter) external onlyOwner {
        if (newMinter == address(0)) revert InvalidMinterZeroAddress();
        minter = newMinter;
        emit NewMinter(newMinter);
    }
}
