# CT1 public-factor and decoy economics: inactive source-backed experiment

**Status:** research evidence for #1306 and the proposed [CT1 contract](../design/ct-transaction-contract.md),
not approval of D3, base fees, a new sampler or activation. This is a scoped
research checkpoint; #1306 remains open for the acceptance work listed below. Ratified D2 EpochOrigin
F=1500/K=17280 is unchanged. These results do not reproduce the old EpochOrigin
Gini calibration: that simulation used value-weighted circulation mixing, while
CT1 proposes maximum public ring factor and paid deflation.

## What ran

The checked-in [summary](../../scripts/research/ct-economics/summary.json) records
source/fixture hashes, toolchain, host and base commit. The raw report is generated
by the test and uploaded by the workspace Linux CI job as
`ct1-economics-<commit>`; its digest is in the summary. The source tree contains
no private keys, live chain data or daemon configuration.

- **Node wallet route:** actual `GammaDecoySelector::select_decoys_for_input`,
  called by `Ledger::get_decoy_outputs_for_input` and the native node wallet.
  Its gamma draw selects a target age; it then chooses the nearest eligible
  candidate. Equal-distance ties retain candidate order. It is not independent
  weighted sampling of every candidate. A separate temporary-ledger fixture
  verifies mature/nonexcluded filtering and ordering against the direct adapter.
- **CLI route:** the actual RPC pool decode/exclude/deduplicate and shuffle path
  from `ring_builder::fetch_decoy_ring_members`. Small helper extraction preserves
  production OsRng and the original error text. The harness prepares unchanged
  fixture pools once and calls the same sampling function with seeded RNGs.
  The live wrapper still fetches its strict age window first; the harness supplies
  exactly that window. Tests compare the combined wrapper helper and split path.
  The unused factor-relaxation selector is **not** treated as this production route.
- **Web route:** actual TypeScript `buildSendTransaction`, with existing boundary
  injection for RPC, ownership and signing. It captures the ring members sent to
  the signer, including two-input rotating windows. This executes selection
  orchestration, **not** WASM cryptography or a real ownership scan. The signer
  consumes these decoys; shuffling ring position is distinct from choosing members.

Native/CLI routes each make 8192 attempts:16 pools, seeds 1306/577/902/17280 and 128
seeded draws per pool/seed. Web records 16 deterministic first-input observations
and 16 additional two-input captures; it is not artificially counted as 8192
independent samples. Every eligible shortfall and fee-affordability failure stays
in the denominator. Unexpected selector errors fail the test.

Pools contain at most 512 synthetic outputs with explicit public age/factor and
identifier fields; synthetic point bytes are identifiers, not signature fixtures.
Rows in `pools.json` are `[id, age, factor]`. Scenarios include mixed factors,
correlated ages, sparse cohorts,18/19-decoy edges and reversed RPC ordering. Both
Rust and TypeScript consume the same pools. A separate real-point ledger fixture
checks the storage adapter. Seeded replay is within the pinned runtime: gamma
floating arithmetic is not a cross-platform consensus identity claim.

## Observations and limits

For a 1 BTH background real input, background output, rate 200 bps, one output and
s=2, base is 0.25 BTH and a 6000 ring floor produces a rounded total fee of
**0.353079215104 BTH**. The original-input floor would charge only 0.25 BTH.
The difference is overcharge caused by the selected public ring, not an extra
actual loss of input value hidden elsewhere in the model.

| Fixture | Node | CLI | Web |
|---|---|---|---|
|1% high-factor, equal ages |512/512 selections; no high factor observed |512/512; some high-factor exposure |1 ordered ring; no high factor observed |
|10% high-factor, equal ages |512/512; median fee 0.353079215104 BTH |512/512; same median |1 ordered ring; same fee |
| Sparse age cohort |512/512 via older candidates; max age 1000 vs real 100 |0/512: insufficient in-window decoys |1 ring includes old candidates |
|18 available decoys |0/512 |0/512 |0/1 |

The equal-age node outcome follows target-age/nearest-candidate tie behavior;
zero observed high factors in one ordered pool does not establish safety in
another pool. CLI shuffle and web RPC ordering produce different membership
support. The report includes member frequencies, empirical support, overlap with
the first ring and maximum age/factor. Age-band violations include younger AND older selected
decoys. Repeat-overlap/Jaccard values are null when fewer than two successful
draws exist, including the deterministic web observations. These are **selection statistics**, not
proofs of sender anonymity or an exploited vulnerability. Factor/age cohorts can
change which outputs are available; selection policy and economics need joint
review.

The candidate base alone makes the 0.0001 BTH and 0.01 BTH value scenarios unable to
pay the modeled half-value transfer. No ring rule can fix that. Sixteen outputs
cost at least 4 BTH under the candidate base, and are unaffordable for a 1 BTH input.
The report distinguishes a successful ring from a fundable transaction and
requires at least 0.000001 BTH modeled change. Zero denominators are explicit;
there is no misleading relative-overcharge ratio for a zero-charge baseline.

The synthetic current-centroid comparison invokes actual
`ring_centroid_implied_factor` with declared member amounts and wealth inputs.
It is a primitive comparison, **not** full existing ledger fee parity: CT1 changes
per-input accounting and public factor derivation, and existing import/tag/base
composition is not recreated by that column.

## Exact rule and boundary checks

The inactive Rust reference reuses production `demurrage_charge` and
`capitalized_reset_charge`, including independent saturation before subtraction.
Tests cover the 49-unit annual-floor case,51-unit reset difference, both capped
terms cancelling, one-cap difference 1, all power-of-two bucket boundaries through
the 68-bit aggregate domain, overflow rejection, counts 1/16 and five self-hops
with preserved public factor. Multi-input charges round once after aggregation.
The reference does not implement a proof, wallet migration or consensus switch.

A public origin-prefix **model** checks epoch 17279/17280, origin floor/interpolation,
known-origin/tag bounds, prefix growth, closed-epoch stability and rollback of
both issuance and import-message consumption. The model uses `(kind,epoch)` keys;
it does not implement the 41-byte network/domain origin codec, authenticate bridge
messages or provide durable reorg storage. Restoring a cloned model is not a
crash-consistent LMDB migration. Those integration/codec obligations remain
#1308/#1309. Ordinary hidden decoy amount permutation leaves node membership
unchanged; this is an input-independence check, not proof that all production CT
consumers are value-free.

## Path C accounting experiment

[Path C results](../../scripts/research/ct-economics/path-c.json) use actual
`count_eligible` and `draw_winners`, plus consensus `reward_cap`,
`compute_pool_accounting` and `carryover_after` from
`botho/src/consensus/lottery.rs`. Gross fees split 80% to the pool and 20% to burn;
payouts are limited by both the Path C ticket cap and the actual block reward.
The early-height reward/emission share comes from the production monetary policy
(the lottery emission share is zero in this measured early epoch). Each of 24 histories uses
720 warmup blocks followed by 512 measurement blocks, retaining the production
720-block maturity,100 mature honest tickets and 1/2/16 attacker outputs per
creation batch. New attacker cohorts are funded from a finite old stock outside
the circulation window, created during the first 512 warmup blocks, and pay their
candidate per-output base **at creation**, not at maturity. All creation fees,
capitalized ticket values, burned fees, payouts and final reserve are accounted
for. Each block checks gross fees plus lottery emission equals distributed
payouts plus reserve plus burns. A positive payout with eligible candidates must
produce a draw; an unexpected missing draw fails the test.

Two controlled external fee conditions are compared: zero and 2.5 BTH per block.
The former yields no attacker capture in this horizon after the creation fees
have already been distributed during warmup; the latter permits capture of other
users' fee contributions. Positive net capture in that condition is not by itself
a demonstrated splitting exploit: proper marginal controls and longer repeated
strategies are needed. The report supplies sampled payouts and analytical uniform
expected share separately. It neither assumes they coincide on one seed nor
asserts net-zero splitting from accounting conservation.

For seed 1306, paid/captured/net BTH with 2.5 BTH external fees per block are:

| Outputs per batch | Paid | Captured | Net |
|---:|---:|---:|---:|
|1 |128 |663.5 |535.5 |
|2 |256 |764.5 |508.5 |
|16 |2048 |967.5 |-1080.5 |

These histories do not bind the reward cap; a separate exact fixture forces a
100-base-unit gross fee against 20 eligible tickets and checks 20 units burned,
20 paid and 60 carried forward. An additional 103-picocredit fee fixture checks
21 burned, 82 paid across four winners (20/20/20/22), and zero rounding carryover:
the actual draw gives its division remainder to the last winner.
The stock/time/fee model is intentionally bounded: no reinvestment, adversarial
adaptive ownership strategies, stationary equilibrium, authentic traffic trace
or whole-network Gini calculation. Repeated self-hop charge arithmetic is tested
separately; the lottery history does not claim to simulate every churn strategy.

## Subsequent funded-payment and reinvestment controls

The later funded-workload capture adds a useful control without changing the
candidate policy. The checked-in
[`workload-summary.json`](../../scripts/research/ct-economics/workload-summary.json)
contains two seeds, four ownership strategies and 11,232-block finite histories.
Each history completes 113 ordinary payments; the captured background histories
produce 56.5 BTH gross fees, 45.2 BTH of lottery capture and 11.3 BTH burned,
with the owner-level accounting checker reconciling all 808 owner records. This
is still a fixed synthetic population and the native gamma membership boundary,
not an observed traffic trace or a complete wallet construction.

The separate
[`ct-reinvestment` capture](../../scripts/research/ct-reinvestment/RESULTS.md)
tests eight histories over the same 11,232-block horizon, comparing locked
awards with oldest-first consolidation at ages 720 and 10,001. Every history
completes its 113 ordinary payments. The enabled consolidation modes make 11,413
attempts each and have zero successful reinvestments: attempts fail through
missing policy-aged inputs or unaffordability, while no payment or selector
failure is hidden. The fixed background fee is 0.25 BTH per input, so a batch of
0.1 BTH awards cannot fund its own consolidation under this strategy. A separate
positive accounting control uses existing ordinary principal to verify the
conservation and draw arithmetic; it is not evidence that the main-grid strategy
is affordable.

This result is useful evidence for the fee-policy decision: a static candidate
base can make small lottery awards unusable even when the BTH purchasing power
changes. It is not a universal impossibility result because the histories do not
search adaptive strategies, ordinary-principal financing, observed ownership,
or long-run reinvestment equilibria. The captures preserve source/configuration
hashes and are run by the bounded Linux workflow; the source and raw-result
identities remain tied to their recorded checkout commits rather than being
presented as a rerun on every later main commit.

## Decision implication

**Do not ratify D3 or the proposed base from these tests.** They demonstrate
concrete honest-cost/liveness differences and show that the independent-decoy
Bernoulli toy model is not a model of every wired wallet route. Maximum ring
factor prevents dilution by extra low-factor decoys but can charge an honest
background input for a wealthy decoy. Its interaction with actual selection and
change allocation is materially different from the ratified D2 calibration.

The next review should choose whether to revise the conservative pricing model,
change the provisional base/affordability target, or pursue a fully linked
hidden-value/selected-origin construction before endorsing circulation semantics.
Any alternative sampler or proof linkage needs its own reviewed proposal; this
PR changes neither. Remaining evidence includes realistic pool distributions,
longer controlled marginal splitting/reinvestment experiments, economic welfare
calibration, and complete CT proof/client integration.

## Acceptance still unmet (#1306 remains open)

- Realistic observed pool/age/factor/ownership distributions and independently
  calibrated EpochOrigin welfare/Gini behavior under the proposed D3 semantics.
- Marginal splitting controls holding **total attacker capital and financing
  opportunity constant** across strategies. The 1/2/16-output schedules here have
  different initial capital; their net results cannot establish split resistance.
- Conditional long-horizon, repeated churn and reinvestment controls, including
  ticket lifetime, endogenous honest activity, ownership concentration and
  adversarially selected cohorts. Five arithmetic self-hops are not that study.
- A ratified affordability/base-fee target and an accepted public-factor/change
  allocation rule, with resulting native/CLI/web policy proposals reviewed together.
- Complete authenticated origin/codec/storage and combined-proof/client integration
  remain the separate implementation gates in #1307–#1310.

This PR is **Part of #1306**, not a claim that the entire issue's economics
validation is complete. Its real source adapters and bounded dataset make the
remaining experiments reviewable and reproducible.

## Reproduction and checks

From the repository root, using the pinned Rust toolchain:

```sh
CT_ECONOMICS_WRITE_REPORT=1 cargo test --locked -p botho --test ct_economics_simulation -- --nocapture
cargo test --locked -p botho --lib ct1_research_ledger_decoy_adapter_parity
cargo test --locked -p botho-wallet --lib ring_builder
python3 scripts/research/ct-economics/summarize.py
```

From `web/`, after the repository's locked pnpm install:

```sh
pnpm exec vitest run packages/wasm-signer/test/ct-economics-selection.test.ts
pnpm exec tsc --noEmit -p packages/wasm-signer/tsconfig.json
```

The web test checks committed captured memberships. To regenerate them deliberately,
set `CT_ECONOMICS_WRITE_WEB=1`; review every changed membership before accepting
evidence. The workspace job compiles scoped binaries separately, then runs native
checks under a 3-minute execution budget and uploads full results. Web CI executes
the actual TypeScript orchestration test. Local native execution passed 6/6 in 45.53 s,
ledger parity 1/1, CLI ring-builder checks 2/2, web selection plus existing memo plumbing
3/3 and TypeScript checking; Linux evidence is recorded on the PR, not inferred
from those local results.
