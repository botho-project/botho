# Inactive native wallet acceptance slice

Part of #1286, implemented under #1358 on the private validated local chain.
This does not activate LotteryV2, migrate legacy payouts, add RPC proofs, or
complete the web/mobile/WASM wallet and cache obligations. Parent #1357 records
a supported macOS-launcher pass of all 93 ledger tests and retains the earlier
EINVAL history; this child still requires its parent merge and final-head CI.

The crate-private `WalletRead` seam preserves ordinary Ledger direct-index
recovery, errors, and decoy parameters. The original wallet tag inheritance,
ring assembly, signing hash, and CLSAG construction remain one implementation.
The private validated adapter supplies strict persisted input/context reads and
strict persisted decoy records. It is only reachable from the inactive local
boundary; no network configuration or serialized context can select it.

Discovery authenticates each record through the accepted local store, removes
the canonical accumulated tweak for the existing classical/hybrid ownership
scan, and recovers through `recover_with_context`, including its final public-key
check. The final output key image controls spent filtering. An owned output
carries its actual outpoint, private context, chain identity and checkpoint;
construction rereads the current accepted state and requires a rescan after a
checkpoint change. It never trusts a cached secret or externally supplied
context. Local accepted DB provenance is the trust boundary, not a remote proof.

Discovery returns owned unspent outputs, including immature outputs. Construction
requires ten confirmations. The canonical 720-block age is lottery eligibility,
not the wallet's spend-confirmation threshold. Source and independently derived
awards retain different final key images even when they share ownership ancestry.

## Bounded positive workload

`native_wallet_accepted_repeated_nested_payout_spends` is an explicitly selected
long profile. It uses actual native wallet signing and actual producer, validator
and writer calls throughout. A wallet funds 16 classical and 16 hybrid outputs
at block 40 using two actual mature coinbase inputs and checked value arithmetic.
Hybrid outputs have nonzero original indices. Recent mature coinbase
outputs fund ordinary ten-BTH fees every ten blocks from 721 through 1440, then
every block until the required natural families appear or height 1700 is reached.
No winner vector, reserve, mature UTXO, reduced maturity or policy override is
injected. The random output keys and lottery draws are not deterministic vectors;
the disclosed workload and stopping bounds are fixed. Failure to realize the
families is an explicit failed acceptance run, not a reason to silently retry.

The matrix requires a hybrid source with original index greater than four and
two affordable awards, a separate classical source/award, and an accepted award
that wins again. The original index necessarily differs from the payout's 1..4
outpoint index. Each family is naturally realized and has ten confirmations.
The wallet signs and the ledger accepts seven independent spends: hybrid source
then two awards; classical award then source; and an award then its nested award.
Checks cover final image independence, unspent sibling discovery after every
spend, recipient discovery, reopen/rescan, transaction/image indexes, and value
conservation including fees, burn, pool and distributed awards. The optional
emission-controller transaction counter remains unchanged under
`apply(None)`; actual transaction-index rows are counted instead. Genesis and
canonical 720 maturity are retained. There is no live network or signed negative
transaction reproduction.

Each profile has a 30-minute runtime budget, separate from compilation.
Phase and accepted-spend records are emitted to the raw log. The measured local
outcomes below bind actual execution to its source and build profile; compilation
or key recovery alone is not evidence of accepted spendability.

```sh
cargo test --locked -p botho --lib authenticated_discovery_separates_maturity_and_checkpoint
CARGO_PROFILE_CI_DEBUG_ASSERTIONS=true CARGO_PROFILE_CI_OVERFLOW_CHECKS=true \
  cargo test --locked --profile ci -p botho --lib native_wallet_accepted_repeated_nested_payout_spends -- --ignored --nocapture
```

Remaining gates include final PR review, exact-head Linux evidence,
parent merge, authenticated remote context/header propagation,
other wallet clients and snapshots, explicit legacy disposition, and activation
policy. This child does not close #1286.

## Initial local profile outcome

The first ordinary unoptimized test-profile run reached the external 1,800-second
limit (exit 124, wall 1,800.012 seconds). Its last reported accepted checkpoint
was height 1,100 with 111 source families at 1,524.786 seconds. It did not reach
the complete repeated/nested spend matrix. This is incomplete runtime acceptance,
not a passing spendability result. The original log and source/binary manifest
are preserved. A one-second read-only stack sample observed funding-payment
wallet discovery/key recovery; that sample is not whole-run attribution.

A separately authorized compile using the existing `ci` profile (opt-level 2,
inheriting `test`) passed in 85.074 seconds under a 900-second bound. Cargo's
emitted artifact metadata confirms debug assertions and overflow checks enabled.
The separately authorized run then **passed in 445.121 seconds**, with natural
families found at height 1,491 and seven accepted spends at heights 1,492–1,498.
It accepted 131 ordinary transactions; the receiver discovered seven outputs
worth 6,937,500,000,000 pico after reopen. Fees were 1,247,000,000,000,000 pico,
burn 249,400,000,000,000, distributed awards 997,600,000,000,000, and final pool
zero. Both full conservation assertions passed. This is one successful measured
history, not a guarantee that every random history realizes the matrix.

The frozen Rust source, strict validation, wallet construction, 1,700-height /
1,800-second bounds and all acceptance assertions were identical between runs.
No automatic retry was performed. [Compact evidence and both raw logs](../research/lottery-v2-native-wallet/evidence.json)
retain the timeout separately from the passing profile, compiler/profile details,
source and binary hashes, and all seven actual transaction IDs and key images.
The source hashes bind the captures to the reviewed uncommitted implementation;
the checkout commit in the raw manifests is its parent, not a claim that a later
PR commit was executed locally.

CI preserves the existing default-profile ledger suite. A dedicated compilation
uses the checked `ci` profile and verifies its emitted settings; a separate step
runs the 19 wallet/focused regressions and the exact ignored long test once with
an external 1,800-second limit. Compile/runtime logs and source/binary hashes
are uploaded even on failure. Final-head Linux evidence is still required.
