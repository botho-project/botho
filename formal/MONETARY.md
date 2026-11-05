# Checked production monetary policy (#1921)

`Botho/Monetary/Emission.lean` models the production policy and proves its
bounded arithmetic properties in Lean 4.35.0 core. It also refutes the
unrestricted typed-supply safety and positive-floor claims. This is a manual,
source-mapped model, not extraction, a full controller/ledger proof or a
completed production DRT campaign. #1922 owns the subsequent comparison leg.

The source baseline is `ae6655e5d6a6e6c5dd2e4cdb580856303624b4eb`.
The node constructor is `botho/src/monetary.rs::mainnet_policy`; the mirrored
constructor is `cluster-tax/src/monetary.rs::mainnet_policy_mirror`.
Production parameters are 50,000,000,000,000 pico (50 BTH) initial reward,
6,307,200 blocks per halving epoch, five epochs, 200 bps tail target plus
50 bps assumed burns, and a 5-second reference block time. The generic
60-second defaults and `fast_test` are different policies.

## Five parent properties and their limits

| Property from #1710 | Proven / refuted in the model | Rust evidence and remaining boundary |
|---|---|---|
| 1. No overflow or panic | `phase_safe` proves the production threshold, nonzero divisors and safe shifts for every u64 height. `tail_safe_domain`, `block_reward_safe` and `successor_safe` derive one-call tail/step safety from the stated supply domain. `unrestricted_tail_claims_refuted` disproves all-typed-supply safety. | Actual release tests call the real tail helper and catch its u128 multiplication panic. They also expose the legacy simulator's narrow-counter and max-height panics. No universal reachable-chain or full-controller safety is proved. |
| 2. Monotonicity / floor | `halving_nonincreasing` applies only within Phase 1. Tail reward is at least one and narrows losslessly on S≤B. A typed, arithmetic-safe supply outside B returns zero after casting. | Actual-source boundary calls include zero, one, u64::MAX, B−1/B/B+1, the zero-cast input and u128::MAX. No cross-phase monotonicity or unrestricted floor is claimed. |
| 3. Phase-1 total | `phase1_exact_sum` partitions the actual per-height reward sum into five epochs, excluding genesis. It proves 611,009,950 BTH, exactly 50 BTH below the nominal inclusive-height-zero total. `first_tail_value` computes the first tail reward from the accepted total. | Tests invoke `cumulative_issuance_at` and `block_reward_at`, including genesis and the first tail block. This is an arithmetic schedule result, not evidence that a particular chain or wall-clock horizon has been reached. |
| 4. Lottery share bound | `lottery_emission_bound` takes the actual successful `blockRewardAt` result on the safe domain, derives its width, binds minting/header height and production interval, and reuses `Supply.Block.emission_exact` for the clamp, casts, bps/product bound, share bound and exact coinbase split. The existing theorem also covers any independently typed reward. | The source path is `MintingTx::lottery_emission_share` / `miner_payout` / `to_tx_output`. No emission model/proof is duplicated. Production-function DRT belongs to #1922. |
| 5. Epoch-0 holiday | `demurrage_current` proves zero for h<H and 200 bps thereafter under the current production policy. | Actual-source tests pin epoch boundaries. #1685 remains an unresolved policy decision; this task does not decide it. |

## Exact executable source map

All scalar model inputs/outputs are naturals. Explicit height/supply width
errors reject values outside the Rust u64/u128 argument types; these are
serialization-boundary errors, not extra Rust behaviors. Other `Error`
constructors record the source's checked-overflow panic boundary. They do not
claim that the Rust API returns `Result`. The workspace release profile has
`overflow-checks = true`; no profile or compiler-trust setting changes here.

| Model operation | Actual Rust symbol / evaluation order |
|---|---|
| `tailEmissionStartHeight`, `isHalvingPhase`, `halvingReward` | The corresponding `MonetaryPolicy` methods: H×5, height below that threshold, then R0 shifted right by height/H. On Phase 1 the shift is <5<64. Outside it the helper returns None. The helper at height zero is still 50 BTH. |
| `annualNet`, `annualBurn`, `annualGross`, `tailPreCast`, `tailRewardU128` | `calculate_tail_reward_u128`: first S×200/10000, then S×50/10000, then checked annual sum, division by (31536000/5), `max(1)`, and finally the u128→u64 modulo cast. The model checks products before division, so division cannot conceal a source overflow. |
| `tailRewardU64`, `tail_u64_domain` | `calculate_tail_reward` widens the u64 input into the same u128 helper. Every u64 supply is inside B. The u64 wrapper is not the node's full-scale supply representation. |
| `blockRewardAt` | `MonetaryPolicy::block_reward_at`, matching `botho::block::calculate_block_reward`: Phase 1 ignores supply, while each tail block recomputes from current cumulative gross issuance. |
| `issuanceSuccessor` | Checked next height, reward at that next height/current gross, then checked gross addition. `store/validation.rs::validate_ordinary_body` uses `state.total_mined` for the reward; `store/writer.rs::block_effects` adds the reward to this u128 gross counter. This is a scalar projection, not their complete accepted-block procedure. |
| `lottery_emission_bound` | Reuses the exact `Supply.Block` production emission path, including its current unsigned saturation spelling `checked_mul(...).unwrap_or(u32::MAX)`. Minting height equals the header height; the same checked reward feeds emission and miner payout. |
| `demurrageRateBps` | `MonetaryPolicy::demurrage_rate_bps`: production H is nonzero, with zero before H and 200 thereafter. |

The node's `ChainState.total_mined` is gross issuance before the block, not
supply net of burns. Fees and their burn share do not reduce this reward
input. The 250-bps gross calibration combines a 200-bps net target with
50-bps assumed burns. Realized net inflation depends on actual burns and
block timing; it is not guaranteed to equal 2%. Difficulty targets time,
not realized net inflation. #1788 retains the separate policy decision.

## Derived sufficient domain and counterexamples

Let M=2^64−1, H=6,307,200 and

```text
B = M * H * 40 = 4653892168868035357845120000
```

For every natural S≤B, `tail_safe_domain` derives that both u128 products and
their divided annual sum fit, the pre-cast reward is between 1 and M, and
the narrowing is lossless. The proof obtains these facts from S≤B; it does
not assume an intermediate product/reward bound. `tail_success_iff` covers
all typed supplies: success has the actual modulo result, without asserting
positivity or deleting arithmetic-safe cast-loss cases.

`successor_safe` requires h<M and S≤B. It derives nextHeight=h+1≤M, the
checked reward and its width, and exact nextGross=S+reward≤u128::MAX using
B+M<2^128. `successor_failures` characterizes max-height rejection and the
conditional gross-add overflow after a successful reward. This is one step:
B is not closed under issuance. A kernel example starting at h=0,S=B succeeds
but exceeds B immediately. No inductive supply bound, arbitrary genesis-chain
horizon or full simulation/controller safety follows.

The kernel and real Rust tests preserve these synthetic boundaries:

```text
S = 2^64 * H * 40 = 4653892168868035358097408000
pre-cast reward = 2^64, after max(1); returned u64 reward = 0
S = u128::MAX: first multiplication overflows (Rust panic)
```

Both zero-cast products and the annual sum fit u128. The unrestricted floor
therefore fails even without an overflow panic. These are typed helper inputs,
not claimed genesis-reachable accepted blocks or economic exploits. Synthetic
scalar max-height/gross-overflow examples have the same scope limitation.

## Genesis and the exact finite sum

Accepted genesis issues zero, with Phase-1 accepted heights 1 through
31,535,999. The five reward levels are 50, 25, 12.5, 6.25 and 3.125 BTH.
Epoch zero has H−1 accepted rewards; the other four have H each:

```text
nominal inclusive-height-zero sum = 611010000000000000000 pico
accepted Phase-1 sum              = 611009950000000000000 pico
unminted genesis difference        =        50000000000000 pico
first tail reward                 =         2421874801813 pico
```

The executable `nominalPhase1`/`acceptedPhase1` compute five epoch terms.
`phase1_exact_sum` connects that efficient result to an explicit sum of the
actual `halvingReward` over accepted heights. Private lemmas prove constant
reward within each epoch, partition the range by induction on the five epochs,
and separate genesis. Kernel evaluation only handles the small finite total;
it does not enumerate thirty-one million heights. The existing node schedule
text retains the valid nominal formula while identifying accepted issuance.

## Simulation parity and exclusions

`simulation::emission_sweep::cumulative_supply_at` already delegates to
`MonetaryPolicy::cumulative_issuance_at`. It starts at height one and iterates
tail rewards from current gross issuance (#1699). Its
`phase1_supply_picocredits` is the nominal inclusive-zero epoch sum, and its
fixed-tail comparison is a separate baseline; neither is accepted issuance.
No existing parity correction is reimplemented here.

`DifficultyController::block_reward` uses state.height+1 and gross_issued;
its cached tail_reward is informational. `process_block` is simulation tooling,
with u64 `total_supply`, reward and epoch shadows in addition to the u128 gross
counter. The focused actual-source tests show that zero-start, raw-pico mainnet
simulation processes 368,934 full rewards and then panics at the narrow supply
addition on reward 368,935. A synthetic state.height=u64::MAX panics at the
successor expression. These are not node u128-ledger failures. The unrelated
difficulty, fee-statistics and simulator transition engine are not modeled.

## Evidence and remaining trust

The five `monetary_1921_*` tests in the existing monetary test module invoke
actual implementations, including `catch_unwind` around all three declared
panics. The focused command uses the unchanged release overflow profile:

```sh
CARGO_BUILD_JOBS=2 cargo +nightly-2026-06-09 test --release \
  -p bth-cluster-tax --lib monetary_1921_ -- --nocapture
```

The existing node/mirror drift guard is preserved. Tests and manual source
mapping support correspondence; neither is extraction or universal Rust
semantic equivalence. Records land in an independently reviewed prerequisite
before the proof PR. The complete proposed source is built and checked with
sorry/axiom gates, exact declaration closures and independent artifact replay;
the proof stage repeats the checks against actual landed default records.

`formal/SUPPLY.md`, its real-commit oracle receipts and the existing
CLSAG/identity/context/storage/commit boundaries remain unchanged. This module
does not prove signatures, cryptographic soundness, LMDB atomicity or all
accepted-block behavior. No DRT leg, codec, scheduler, case count or budget is
changed. #1922 owns model/production DRT and bounded simulator parity; #1896
retains its distinct accepted-block runtime gap. #1710, #1704 and #1839 remain
open until their remaining obligations are independently reconciled.
