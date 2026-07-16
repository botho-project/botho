# wBTH — Wrapped BTH on Ethereum

`WrappedBTH.sol` is the ERC-20 token minted 1:1 against BTH locked in the
bridge reserve (epic #816). One wBTH base unit equals one picocredit
(`decimals() == 12`); no scaling happens anywhere across the bridge boundary.

## Security model (decided in #826)

### Custody — ADR 0002

No single key can mint. Roles are granted at deployment to Gnosis Safes,
never to EOAs, and the deployer receives no roles:

| Role | Holder | Purpose |
|------|--------|---------|
| `MINTER_ROLE` | Validator Safe (t-of-n secp256k1, owners = SCP validators; t ≥ SCP safety threshold) | The only mint path: `Safe.execTransaction → bridgeMint` with threshold owner signatures from the #824 attestation |
| `DEFAULT_ADMIN_ROLE` | Governance Safe (distinct from the validator Safe) | Rate-limit / breaker configuration, role administration |
| `PAUSER_ROLE` | Guardian Safe (may be lower-threshold) | Fast `pause()` / `unpause()` — fail-safe only, cannot move funds |

Record the concrete Safe addresses + thresholds here per deployment:

| Network | wBTH | Admin Safe | Minter Safe | Pauser Safe |
|---------|------|-----------|-------------|-------------|
| Sepolia (testnet) | [`0x49b985ec427ee771a601f11b18f7d4402fa2dd7b`](https://sepolia.etherscan.io/address/0x49b985ec427ee771a601f11b18f7d4402fa2dd7b#code) (verified) | `0x61274F558f9027e2D402d3340dE89152FA3F3947` | `0x61274F558f9027e2D402d3340dE89152FA3F3947` | `0x61274F558f9027e2D402d3340dE89152FA3F3947` |

Sepolia Safe is a **2-of-3** Gnosis Safe (owners
`0xc74E98…`, `0x1D72CD…`, `0x53bce9…`); the deployer
(`0x111018…`) holds no roles. Deployed + verified 2026-07-16 (#1013).

**Live DeFi round trip (2026-07-16, #866/#868/#869).** The full mainnet
liquidity-launch sequence was run end to end on Sepolia via
`scripts/live-defi-roundtrip.ts` (a faithful ethers port of the fork-tested
`bridge/service/src/{uniswap_fork_tests,defi_round_trip_tests}.rs`):

1. **Mint** 100,000 wBTH to the LP through the real 2-of-3 Safe —
   `execTransaction(bridgeMint)` with two owner secp256k1 sigs, relayed by
   the role-less deployer (ADR-0002 custody, exactly as
   `bridge/service/src/mint/ethereum.rs`).
2. **Pool** wBTH/WETH created on Uniswap v3 (0.30% tier) at
   [`0x16C4fDbe2b7497EA67f1DC8205dd2F5B31458D53`](https://sepolia.etherscan.io/address/0x16C4fDbe2b7497EA67f1DC8205dd2F5B31458D53),
   seeded with 100,000 wBTH + 0.1 WETH full-range liquidity.
3. **Swap** 0.01 WETH → 9,066 wBTH through `SwapRouter02.exactInputSingle`.
4. **Repatriate** — `bridgeBurn` of the swap proceeds (Ethereum-side leg;
   totalSupply 100,000 → 90,934 wBTH).

The custody mint was pre-flighted against live state with
`scripts/validate-mint-sim.ts` (signature recovery + `eth_call`, zero spend).
The native-BTH release leg (Layer 2, #866/#868) remains separate — it needs a
live Botho node + watcher.

### Replay-proof, order-bound minting

`bridgeMint(to, amount, orderId)` consumes a unique `bytes32` order id
(the bridge order id from the attestation protocol, #824) recorded in
`processedOrders`. A duplicate reverts — this is the on-chain guard that
closes the residual double-mint window even if the off-chain service
retries or is compromised into re-submitting an authorization. The
function/event ABI is bound by `bridge/service/src/mint/ethereum.rs`
(`bridgeMint(address,uint256,bytes32)`, `BridgeMint` with indexed `to` and
`orderId`); do not change these signatures without updating the Rust side.

### Upgradeability — immutable, no proxy

The contract is deliberately not upgradeable. A proxy admin is a rug
vector that would negate the Safe-threshold custody model (whoever can
upgrade can mint), contrary to the project's no-hard-forks / minimal-trust
posture. Trade-off: bugs are handled by `pause()`, deploying a corrected
token, and migrating balances through the bridge itself (burn on old,
mint on new). Accepted and documented in the contract NatSpec.

### Rate limits + circuit breaker

- `maxMintPerTx` (default 1M BTH) and `dailyMintLimit` (default 10M BTH),
  both in picocredits; the daily counter lazily resets on the first mint
  of a later UTC day (correct across multi-day gaps).
- **Auto-pause breaker:** when cumulative daily volume reaches
  `autoPauseThreshold` (default = the daily limit; 0 disables), the
  contract pauses itself — anomalous volume becomes a hard stop requiring
  guardian review, not just a revert. The service-side breaker (#827)
  complements this.
- The per-recipient mint cooldown was removed: recipient rotation
  bypassed it trivially, while honest multi-order throughput was griefed.

### Burn path

`bridgeBurn(amount, bthAddress)` is the only burn path; it emits the
`BridgeBurn` event that authorizes the native-chain release (watched by
`bridge/service/src/watchers/ethereum.rs`). The open ERC20Burnable
surface was removed because a plain `burn` destroys wBTH without a
redemption destination, stranding locked reserve and breaking the supply
invariant `totalSupply == Σ mints − Σ event-bearing burns`.

### Reentrancy

OpenZeppelin ERC-20 v5 `_mint`/`_burn` make no external calls (no
ERC-777 hooks), so no reentrant path exists today. Defense-in-depth:
strict checks-effects-interactions ordering plus `ReentrancyGuard` on
both bridge entrypoints, so the property survives future edits.

## Development

```bash
npm install
npm run compile
npm test
```

The test suite covers access control (Safe-only mint, no deployer
roles), order-id replay, rate limits + daily reset boundaries, pause +
auto-pause breaker, decimals/unit pinning against the Rust bindings, and
a randomized supply-accounting invariant run.
