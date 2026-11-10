# formal/ - Lean 4 models and proofs

This directory holds Lean 4 models of selected Botho mechanisms, proofs about
those models, and the gates that keep the proofs honest. It is the scaffold for
epic #1704, set up by #1705. It contains a toy model (`Botho/Basic.lean`)
that mirrors no Rust code, and the LotteryV2 model (`Botho/Lottery/`, #1707),
which mirrors the node's lottery draw, reward cap and pool accounting and is
differentially tested against the Rust code (see
[The lottery model](#the-lottery-model) and
[Differential random testing](#differential-random-testing-drt)).
It also holds the flat-FBAS quorum model (`Botho/Quorum/`, #1714), which
decides the quorum gate's intersection, fault-tolerance and rotation questions
(see [The quorum model](#the-quorum-model)), and the cluster-tag model
(`Botho/Tags/`, #1840), which mirrors the consensus tag check C6, the fee
floor's reads of output tags and the wallet's tag merge
(see [The cluster-tag model](#the-cluster-tag-model)).

The approach follows Cedar's verification-guided development: write a small
executable Lean model of a Rust component, prove properties about the model,
and use differential random testing (DRT) to check that the Rust code and the
model agree.

## The model, prove and DRT loop

1. **Model.** Write an executable Lean definition (`def`, not `axiom`) that
   mirrors a Rust function or state transition, in `Botho/<Area>.lean`. Keep it
   as close to the Rust structure as the proofs allow. Cite the Rust file and
   function it mirrors in the module docstring.
2. **Prove.** State and prove properties of the model (conservation, bounds,
   determinism, monotonicity) as `theorem`s in the same module. Add each
   exported theorem to `Theorems.lean` (see below).
3. **DRT.** Compile the model to an executable and run it against the Rust
   implementation on random and boundary inputs, comparing outputs. A model
   proof and passing finite DRT provide evidence of Rust correspondence; they do not prove universal Rust equivalence.
   The harness is `formal-drt/` (#1706, #1707); see
   [Differential random testing](#differential-random-testing-drt).
4. **Mirror.** Add the mirrored Rust files to the `pattern` in
   `.github/workflows/formal.yml` so that edits to them re-run the Lean job.

## Labeling rule: proven / tested / trusted

Label each claim in its module docstring and any document that cites it.
A component can have different labels for different claims:

| Label | Meaning |
|-------|---------|
| **proven (model)** | A theorem about the Lean model, listed in `Theorems.lean`, using only `propext`, `Classical.choice` and `Quot.sound`. Its stated hypotheses apply; finite DRT does not turn it into a proof about every Rust execution. |
| **tested** | The Rust code and the Lean model agree on the finite inputs exercised by DRT. This is evidence of correspondence, not universal equivalence. |
| **trusted** | Depends on an explicit assumption about primitives or code outside the model. Name it beside the claim. The current allowlists contain only the three standard axioms; `Lottery.Prims` supplies hash functions as parameters, not custom hash axioms. |
| **not covered** | No applicable proof or test boundary establishes this property. |

Never write "verified" on its own. Write "proven (model)", "proven (model) +
tested (finite DRT)", or "trusted (SHA-256 security assumption)". A theorem
quantifying over arbitrary hash parameters does not prove their security.

The [block supply accounting note](SUPPLY.md) records the #1839 fee-split
results and the conditional block accounting theorem, including the ledger
and cryptographic premises still needed for an accepted-block claim.

### Proven about the extraction vs proven about the model (#1821)

Some kernels are also proven in `formal-aeneas/`, about Lean that Charon and
Aeneas generate mechanically from the Rust source (`formal-aeneas/generated/`),
instead of about a model written by hand here. Those claims carry a
different label, and the two must not be mixed:

| Label | Meaning |
|-------|---------|
| **proven (extraction)** | A theorem in `formal-aeneas/BothoAeneas/`, listed in `formal-aeneas/Theorems.lean` and passing `formal-aeneas/scripts/check-gates.sh` (only the axioms in `formal-aeneas/axioms-allowlist.txt`, today the same three standard ones; no `sorry`, no `axiom` or `opaque` in the package). It is about the generated Lean, which CI keeps identical to a fresh extraction of the current Rust source, so it says something about the Rust code with no DRT step. What it trusts instead: the Charon + Aeneas translation at the pin in `formal-aeneas/toolchain.env`, the Aeneas library's model of the Rust primitives used (integer arithmetic with panics as `fail`, casts, `max`), and, in CI, a gate result that comes from a non-isolated build (the `typecheck` job compiles and gates in one job, with no separate verify step; see the rules below). An overflow is modelled as a panic, not a wrap, which matches production because release builds keep `overflow-checks = true` (#663). |
| **proven (model) + tested** | The `formal/` path above: a theorem about a hand-written model, with finite DRT evidence of agreement with Rust on the tested inputs. |

Rules:

- Cite which one you mean: "proven (extraction): `halving_reward` never
  panics when ..." or "proven (model) + tested: ...". Plain "proven" means
  proven (model), as in the table further up.
- Keep extraction and model labels distinct. The pool-accounting bridge
  below proves one precise equality; other extraction postconditions do not
  establish equality to a canonical model. Transferring a model theorem also
  requires its own premises and the bridge's representation/domain conditions.
- Use the extraction where Aeneas can reach the code (pure integer kernels
  in `cluster-tax`, see `formal/aeneas/SPIKE.md`). Use a model + DRT where it
  cannot (floats, hashing, iterator chains, protocol state machines).
- The two packages never share a build, a cache or a gate: `formal/` stays
  dependency-free and its build -> verify trust chain (#1799) is unchanged,
  while `formal-aeneas/` depends on the Aeneas library and Mathlib and runs
  in its own CI job (`.github/workflows/aeneas-extract.yml`, job
  `typecheck`). That job is not a required check and has no separate trusted
  verify step, so its result is evidence from an untrusted build, like the
  `formal/` build job's.

### Extraction coverage by kernel (#1713)

| Rust kernel | Actual extraction claim | Canonical model correspondence |
|---|---|---|
| `compute_pool_accounting_permille` | Successful return plus full-record equality (`compute_pool_accounting_permille_model`); `_model_800` and `_no_fail` consequences | Proven equality to `Botho.Lottery.computePoolAccounting` for all typed inputs with `pm.val ≤ 1000`, including `u128` saturation, and arbitrary draw config |
| `split_fees_permille` | Exact fraction and fee conservation | Used in the pool bridge; no separate canonical equality export |
| `MonetaryPolicy::halving_reward` | No failure, phases and antitonicity with interval/count bounds | Finite `monetary` DRT leg; no canonical equality bridge |
| `MonetaryPolicy::calculate_tail_reward_u128` | Conditional no failure and exact truncation; positive result needs its width premise | No canonical equality bridge; zero-cast counterexample and mainnet supply bounds remain |
| `sybil_reward_cap`, `is_eligible_at` | Exact scalar postconditions | Finite `sybil_reward_cap` and `is_eligible` DRT legs (the latter calls the candidate wrapper); no canonical equality bridge |
| `lottery_emission_bps`, `lottery_emission_share` | Exact scalar postconditions and share bounds | Finite `monetary` DRT leg; no canonical equality bridge |
| `capitalized_reset_charge` | Only the no-downgrade branch | Finite `demurrage_composition` DRT compares canonical reset/settlement/spend models with the public Rust wrappers; no extracted arithmetic equality proof; remaining composition properties stay #1711 |
| `count_eligible_in`, `demurrage_charge` | Extracted and axiom-gated, without correctness theorems | Finite `count_eligible` wrapper DRT and `demurrage_charge` ↔ `ExactCharge.exactCharge` DRT; neither proves these extractions |

The reset-charge oracle test is
`cluster-tax/src/demurrage.rs::capitalized_reset_charge_matches_exact_on_grid`.
It is distinct from the accrued-charge model claims and DRT in
[DEMURRAGE.md](DEMURRAGE.md); no full reset/composition result is implied.

The bridge reads the five `.val` fields into `Botho.Lottery.Accounting`;
it does not copy a substitute arithmetic model. Its successful-return
conclusion excludes panic and divergence in the extracted semantics. The
unextracted public config wrapper has a restricted forwarding source check,
not a semantic proof. See [the exact statements and boundaries](../formal-aeneas/README.md#equality-to-the-canonical-pool-model-1713).

The Aeneas package compiles precisely `Pool`, `Draw`, `Prims`, `Candidate`
and `Arith` from this directory as sibling **sources**, using matching Lean
pins and its own `.lake/build` artifacts. It does not consume `formal/.lake`
or add Aeneas/Mathlib to this package. All five modules are scanned and owned
by its compiled axiom walk, including unused declarations. Canonical edits
trigger its separate CI. This source sharing does not merge the trust chains:
`formal/` retains isolated verification; the extraction gate still runs in
the same job as its build and trusts the pinned translator and primitive
library. Whole-node correctness and other kernels are not upgraded by this
one bridge.

## Building locally

Install elan, the Lean toolchain manager:

```bash
curl -sSfL https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y --default-toolchain none
# or: brew install elan-init
```

Then build. elan reads `lean-toolchain` and installs the pinned version
automatically:

```bash
cd formal
lake build                      # type-checks every module under Botho/
scripts/check-sorry.sh          # sorry gate (scans, then builds)
scripts/check-axioms.sh         # axiom gate
scripts/selftest-gates.sh       # proves both gates reject injected defects
scripts/selftest-verify.sh      # proves the verify job rejects the bypass fixtures
scripts/propose-statements.sh   # prints the statement record this tree compiles to
```

To run the verify job's checks on your own build, stage the `.olean` files
the way CI uploads them and point the verifier at them:

```bash
rsync -am --include='*/' --include='*.olean' --exclude='*' \
  .lake/build/lib/lean/ /tmp/formal-lib/
scripts/verify-artifacts.sh --lib /tmp/formal-lib --sources .
```

`verify-artifacts.sh --sysroot DIR` and `selftest-verify.sh`'s `LEAN_SYSROOT`
name the toolchain directory when `lean --print-prefix` does not find it.
`verify-artifacts.sh` passes that directory to every Lean process as
`LEAN_SYSROOT`, so `Init` always comes from it, never from a `lean` on `PATH`;
with `--sysroot` it needs no `lean` on `PATH` at all. It and the gates need a
`python3` with `tomllib` (3.11 or later).

The scripts take an optional `LAKE=/path/to/lake` environment variable if the
`lake` on your `PATH` is not the elan proxy.

The toolchain is pinned to an exact stable release in `lean-toolchain`. Bump it
deliberately in its own PR. Never use `stable` or `nightly`. The project has no
dependencies (no Mathlib). #1708 decided against adding Mathlib: its proofs
need only `omega`, `decide`, core `List.Perm` and `Nat` lemmas, and Mathlib
would add CI time and break the dependency-free build -> verify trust chain
(#1799). `formal-aeneas/` uses Mathlib in its own package and CI job.

## The lottery model

`Botho/Lottery/` (#1707, the pilot of epic #1704) models what the node
computes for each block's LotteryV2 draw. Each definition's docstring names
the Rust function and file it mirrors:

| Lean (`Botho.Lottery.*`) | Rust | Status |
|--------------------------|------|--------|
| `lotterySeedV2` | `lottery_seed_v2`, `botho/src/consensus/lottery.rs:81` | tested |
| `Candidate.isEligible` | `LotteryCandidate::is_eligible`, `cluster-tax/src/lottery.rs:187` | tested |
| `Candidate.weight` | `LotteryCandidate::weight`, `cluster-tax/src/lottery.rs:203` | tested (through the draw) |
| `countEligible` | `count_eligible`, `cluster-tax/src/lottery.rs:248` | tested |
| `sybilRewardCap` | `sybil_reward_cap`, `cluster-tax/src/lottery.rs:272` | tested |
| `drawWinnersWithSeed`, `drawEligible`, `verifiableRandomU128` | `draw_winners_with_seed`, `draw_eligible`, `verifiable_random_u128`, `cluster-tax/src/lottery.rs:341,361,498` | tested |
| `splitFees` | `LotteryFeeConfig::split_fees`, `botho/src/consensus/lottery.rs:167` | tested |
| `Accounting.carryoverAfter` | `LotteryPoolAccounting::carryover_after`, `botho/src/consensus/lottery.rs:283` | tested |
| `rewardCap` | `reward_cap`, `botho/src/consensus/lottery.rs:306` | tested |
| `computePoolAccounting` | `compute_pool_accounting`, `botho/src/consensus/lottery.rs:326` | tested |
| `drawLotteryWinnersSeeded` | `draw_lottery_winners_seeded` and `finish_draw`, `botho/src/consensus/lottery.rs:391,410` | tested |
| `offsetKey`, `lotteryCandidates` | `Ledger::lottery_candidate_offset_key`, `botho/src/ledger/store.rs:2811`; the candidate read `lottery_candidates`, `botho/src/ledger/store/validation.rs:733` (reached through `DerivationReads::lottery_candidates`, `botho/src/ledger/lottery_v2.rs:168`) | tested |
| `headerHash` | `BlockHeader::hash`, `botho/src/block.rs` | tested (through `drawing`) |
| `seed`, `drawing` | `lottery_v2::seed`, `lottery_v2::drawing`, `botho/src/ledger/lottery_v2.rs:220,238` | tested |

Line numbers are those of main at `651d0c45`.

How the model is written:

- **Integers.** The model computes on `Nat` and states each Rust width.
  `Botho/Lottery/Arith.lean` defines one helper per Rust primitive
  (`satSub`, `satAdd`, `satMul`, `checkedAdd`, `truncU64`, `leBytes`,
  `ofLeBytes`), and a plain `+` or `*` is used only where the Rust operation
  cannot overflow, with the reason next to it.
- **Recursion.** Lists are walked by structural recursion, mirroring the Rust
  loops: the cumulative winner walk (`selectAt`), the roll loop
  (`pickWinners`), the capped candidate scan (`scan`).
- **Hashes are parameters, not axioms.** Every hashing function takes a
  `Prims` argument holding `sha256` and RandomX's `powHash`. A theorem about
  the model therefore holds for every hash function, and no axiom is added
  (`axioms-allowlist.txt` is unchanged). A theorem that needs a property of
  the hash takes it as a hypothesis and is labeled **trusted**. The DRT
  executable instantiates `sha256` with a SHA-256 written in Lean
  (`formal-drt/lean/Drt/Sha256.lean`, checked against the FIPS 180-4 vectors
  at start-up) and takes the RandomX output of the parent header from the Rust
  side as data.
- **Inputs, not models.** The per-UTXO cluster factor (the fee curve, #1711),
  the emission share (`MintingTx::lottery_emission_share`, #1710) and
  RandomX are inputs.
- **Out of scope.** `SelectionMode::EntropyWeighted` (research-only, reads a
  floating-point entropy), the `f64` to basis-point conversion of
  `SelectionMode::Hybrid` (the model takes `alpha_bps`; DRT checks the round
  trip for every value it generates), and the strict read policy's failures
  in the candidate read (the model assumes every row decodes).
- **Preconditions.** `winners_per_draw > 0` (Rust divides by it; consensus
  fixes it at 4), and the candidate read's rows sorted strictly by key (as
  LMDB iterates them).

The theorems about this model are #1708. [`lottery.md`](lottery.md) says in
plain words what each one guarantees and what it does not. Part C1, the
Sybil split, is `Botho/Lottery/Sybil.lean`: a counterexample to the
"splitting is net-zero" claim, the inert reward cap, profit as a function of
the split count, and the premise of #1837's R3 option.

## The quorum model

`Botho/Quorum/Fbas.lean` (#1714) models a federated Byzantine agreement
system with flat quorum sets (a threshold over a member set, no inner sets),
which is the shape of Botho's `QuorumSet` and of `bth_quorum_sim`. Byzantine
faults follow Mazieres' SCP paper: `delete F B` is `(V, Q)^B`,
`IntersectsDespite`, `AvailableDespite` and `IsDSet` are its "quorum
intersection despite B", "quorum availability despite B" and DSet. SCP's
federated-voting safety and liveness given these properties are out of scope
(the paper and the Ivy and TLA+ proofs cover them). The module decides the
FBAS-level questions the quorum gate depends on, several of which come out
"no":

| Question | Theorem | Result |
|----------|---------|--------|
| `t`-of-`n` intersects iff `2t > n` (both branches of `symmetric_quorum_has_intersection`) | `sym_intersects_iff_pos`, `analytic_rule_eq` | proven |
| `t`-of-`n` intersects despite `f` faulty iff `2t - n > f` | `sym_despite_iff` | proven |
| `byzantine_tolerance` is the largest tolerated `f` | `byzTol_iff_dset` (needs two honest nodes; `byzTol_conservative_edge` shows the edge is conservative) | proven |
| `quorumFaultTolerant` agrees with it (lead A) | `recommended_crash_flag_overclaims` | refuted for the pre-#1864 flag at odd `n >= 5`, Crash mode: #1843 (historical; fixed by #1864) |
| The node-local gate makes the fleet intersect (lead B) | `recommended_crash_bowtie_fork`, `recommended_bft_bowtie_fork` | refuted: #1844 |
| Pairwise checker sound / complete | `checker_sound`, `checkDespite_sound`, `checker_complete_sym`, `checker_not_complete` | sound; complete for symmetric only |
| Exact checker (the sim's enumeration) | `bruteIntersects_iff` | proven |
| Mixed A/B rotation: `t_A + t_B > \|S_A ∪ S_B\| + f` suffices | `rotation_sound`, `nested_rotation_safe` | proven |
| ... and is necessary (`f = 0`) | `rotation_condition_necessary` | proven |
| The gate checks transitional safety | `reroster_mixed_fork`, `prod_4of5_rotate_two_not_despite_one` | refuted: #1845 |
| 4-of-5 production instance | `prod_4of5_*` | `f = 1`; a one-member swap is safe despite 1, a two-member swap is not |

**Abstraction in the rotation theorems.** `mixed N A B runsB` gives every
node `A` or `B` verbatim. The node's Explicit gate always adds the local node
to the configured roster, so a node that is not on roster `S` really runs
`S ∪ {self}`. "Every split" in `rotation_sound`, `nested_rotation_safe` and
`prod_4of5_rotate_*` therefore covers some quorum sets no node would run and
misses the one an off-roster node does run. The counterexamples and
`rotation_condition_necessary` use only self-listed nodes and are unaffected,
and for the production instances the real-gate quorum sets are checked in
Rust (below): the one-member swap stays despite-1 safe in all 64 splits and
the two-member swap intersects in all 128.

Status: every theorem is proven (model). The Rust side is tested by
`#[cfg(test)]` checks in `botho/src/commands/run.rs`:

- an exhaustive check of `symmetric_quorum_has_intersection` and
  `byzantine_tolerance` against the proven closed forms for `n <= 64`;
- a re-check of each heterogeneous instance with `bth_quorum_sim`, through a
  test-only mirror of `delete`;
- the three bugs through the node's own code: `gated_scp_quorum_set`,
  `QuorumConfig::is_bft_fault_tolerant` and `byzantine_tolerance` (#1843,
  odd-`n` Crash fleets flagged fault-tolerant with tolerance 0), the per-node
  gate on bowtie views (#1844), and two Explicit configurations during a
  re-roster and a two-member swap (#1845). These tests assert the current
  behaviour, bugs included, and link the issue; fixing a bug means updating
  its test. #1843 is fixed on main by #1864: the Lean flag model
  (`recommendedFaultTolerant`) describes the flag as it was before #1864, and
  the Rust test `crash_odd_n_flag_matches_tolerance_through_gate` now asserts
  the fixed flag (not tolerant, degenerate) on the same odd-`n` fleets.

A random heterogeneous `formal-drt` leg is a follow-up.

The separate [demurrage counterexample model](DEMURRAGE.md) covers the
charge kernel and default ring factor (#1711), including refutations of
the positive-charge threshold, churn identity and conservative-overflow claim.
Those theorems are about the pre-9.0.0 kernel (`charge`), kept as the record
of #1875; the 9.0.0 exact kernel is modelled by `exactCharge`
(`Botho/Demurrage/ExactCharge.lean`), which the `demurrage_charge` DRT leg now
compares against. Its six listed and statement-bound claims (#1916, #1963)
prove the sharp positive-charge ceiling threshold, the split loss bound even
with clipping, the two-sided split bound without aggregate clipping, a typed
clipping counterexample, and full-Nat elapsed/factor monotonicity. The six existing exact-centroid/claimed-wealth
claims are also listed and bound with their original statements. These are
**proven (model)**; DRT is separate evidence of Rust correspondence. See
[the theorem domains and checked examples](DEMURRAGE.md#exact-charge-threshold-and-splitting-1916).

`exactResetCharge`, `exactSettlementCharge` and `exactSpendCharge` add
executable composition models, **tested** by the structured
`demurrage_composition` leg (#1985). Each case compares all three public Rust
wrappers: 10,000 cases means 30,000 function results, not 30,000 independent
cases. Separate rounding/caps, the constant settlement horizon and max
composition are covered by live Rust/Lean fixture assertions; see
[the composition domain and controls](DEMURRAGE.md#composition-models-and-drt-1985).

The #2015 increment adds six **proven (model)** full-Nat composition claims:
reset is monotone in input factor and antitone in output factor; settlement is
monotone in input factor; spend is monotone in input factor and elapsed, and
antitone in output factor. Each uses only the respective ordering premise.
The raw guard, separate floors/caps, fixed 31,536,000-block settlement/reset
horizon and `max` composition remain unchanged. Fifteen unnamed kernel-checked
examples supplement the proofs, including zero inputs, clamp/cap plateaus,
separate rounding, spend crossover and the decreasing generic-reset horizon
witness. All 259 approved records remain byte-identical. Rust correspondence
stays **tested**; the epoch-0 two-rate path, universal Rust/extraction equality,
width/panic/allocation/cardinality safety, accepted/genesis reachability,
principal bounds, full fee-floor policy and privacy/economic guarantees are
separate obligations. See [the six contracts and assurance boundary](DEMURRAGE.md#proven-composition-model-properties-2015).


`Botho.Demurrage.ringElapsedQuantile` (`Botho/Demurrage/Age.lean`, #1993)
adds an executable age order-statistic model and the 27th DRT leg. The actual
Rust helper and built Lean codec receive 27 explicit numerical vectors and
114 separately counted value/permutation transformations. Generated lists
are bounded to 0..=64 with full-width scalars and deliberately sampled positive
mixed ages/interior ranks. Both production callers currently use 10,000 bps
(maximum age), read from source rather than exercised as complete validators.
The #1993 increment supplied finite correspondence evidence, with no listed
theorem, extraction-equivalence or generic panic/reachability claim; see
[the quantile domain and controls](DEMURRAGE.md#ring-age-quantile-model-and-drt-1993).
The linked normal-source receipt at `7eb426d8` completed 270,000 comparisons
and 225 replays with zero mismatches; final PR CI is separate from that
historical control-workflow checkout and its captured dependency inputs.

The subsequent #2008 increment adds four **proven (model)** full-Nat quantile
properties: arbitrary per-position value independence under equal creation-height
sequences, whole-pair permutation, monotonicity under the sole premise `q1 ≤ q2`,
and an unconditional bound by current height. They cover arbitrary finite lists,
empty/future-height cases, zero/beyond-u64 values and q outside 0–10,000. Zero-valued
members still count and can yield a positive quantile. Eight unnamed kernel-checked
examples supplement the proofs, including both interior nearest-rank thresholds.
All 253 approved statement records and canonical executable definitions remain
unchanged. Rust/Lean correspondence stays **tested**; no universal Rust/extraction,
width/panic/allocation/cardinality, fee/principal/privacy-economic or accepted/genesis
reachability result follows. The other #1711 obligations remain open. See
[the four exact contracts and provenance](DEMURRAGE.md#proven-elapsed-quantile-model-properties-2008).


`ringElapsedCentroid` (#1998) adds the 28th scalar DRT leg and repairs the
public elapsed-age helper with the existing wide accumulator. Twenty live
expected-output vectors and 45 separately counted transformations check
full-width weighted means, zero/future ages and one final floor. Generation
uses deliberately mixed lengths 0..=64, including numerator-above-u128 and
denominator-above-u64 classes; this is a campaign budget, not a contract cap.
The unchanged normal tier compares 280,000 generated cases when all 28 legs
complete, plus all replay fixtures. Source-bound old checked/unchecked
reproduction and actual-source numerical controls are documented in
[the centroid assurance section](DEMURRAGE.md#exact-elapsed-age-centroid-1998).
The subsequent #2004 increment proves four full-Nat centroid properties:
whole-pair permutation, zero-value insertion at any position, positive common
value scaling, and an unconditional bound by current height. These are
**proven (model)**, including empty/zero-total and future-height inputs;
Rust/Lean correspondence remains **tested**. The canonical executable model
is unchanged. Rust/extraction equivalence, machine-width/panic/allocation/
cardinality safety and accepted/genesis reachability remain unproved. Neither
height boundedness nor scaling proves fee behavior, a principal bound or
general factor dilution resistance. Production age selection is unchanged;
dependency/executable provenance remains specific to each run.


## The cluster-tag model

`Botho/Tags/` (#1840) asks whether the cluster-tag rules keep the invariants
the privacy and demurrage design assumes. It models two kernels and keeps
them apart, because a modified wallet need not run the wallet's:

- **Consensus** (`Botho/Tags/Inherit.lean`). Block acceptance applies no
  tag mixing and no decay. In 7.x its only tag rule was C6,
  `check_cluster_tag_inheritance` (`botho/src/ledger/store.rs:256`,
  `CONSENSUS_TAG_DECAY_RATE = 0`). The ledger also reads output tags for the
  fee floor (`effective_cluster_wealth_from_outputs`,
  `import_floor_factor_from_outputs`, `store/validation.rs:280,313`) and
  credits them to the per-cluster wealth index
  (`update_cluster_wealth_for_output`, `store/writer.rs:482`). The fee curve
  and the ring-centroid factor are parameters (`FeeEnv`), not models (#1711).
- **Wallet** (`Botho/Tags/Vector.lean`).
  `ClusterTagVector::merge_weighted` (`transaction/types/src/cluster_tags.rs:366`)
  and `from_pairs`, which the stock wallets use to choose output tags and
  the mempool uses to compare a transaction's output tags with its ring.

**Scope: the theorems are about protocol ≤ 7.x and the pre-#1863 merge.**
Two refutations were live bugs, now fixed: #1857 (7.x consensus did not
bound output tags; fixed by Rules 1 and 2, protocol 8.0.0, #1870) and #1858
(the mempool's `merge_weighted` could panic; fixed by an `is_valid` relay gate
and a saturating merge, #1863). The statements are unchanged, as the
machine-checked record of both findings. The fixed code is modelled by new
definitions, `inheritOk₈` (Rule 1, Rule 2, then C6) and `mergeWeightedSafe`,
which the DRT legs `cluster_tag_inheritance` and `merge_weighted` now compare
against; their theorems are a follow-up (new statements need their records
on the base branch first).

Not mirrored (no caller outside the simulations): `cluster-tax`'s
`block_decay.rs`, `age_decay.rs`, `entropy_decay.rs`, the floating-point
`TagVector`, and `merge_weighted_with_and_decay`.

| Invariant | Kernel | Result | Theorems |
|---|---|---|---|
| 1. Output tag mass at most 100%, no attribution to clusters absent from the inputs | C6 + writer | **refuted** for 7.x (#1857; fixed by #1870) | `inherit_admits_fresh_mass_above_one`, `fresh_cluster_credit_unbounded`, `inheritOk_cons_fresh`; what C6 does bound: `inheritOk_bound` |
| 2. Mixing is a value-weighted convex combination | wallet | **refuted** exactly; **proven** as a sub-convex bound | `mergeWeighted_truncation_halves_mass`; `mergeWeighted_sub_convex`, `mergeWeighted_mass_le_scale` |
| 2'. The merge does not panic | wallet, mempool | **proven** for well-formed inputs, **refuted** in general, pre-#1863 (#1858; fixed by #1863) | `mergeWeighted_isSome`; `mergeWeighted_overflow_panics`, `mergeWeighted_no_panic_4294` |
| 3. Decay is monotone and never raises a weight | wallet | **proven** below `2^32`, **refuted** (in the rate) above it | `decayEntry_le`, `decayEntry_antitone`; `mergeWeighted_decay_not_antitone` |
| 4. (#1687) No accepted spend drops a tagged input's mass without paying the demurrage owed | C6 + fee floor | **refuted** for 7.x-9.x (empty outputs pass C6 and Rules 1 and 2, and the import floor read the outputs) | `inheritOk_of_untagged`, `importFloor_of_untagged`, `tag_drop_demurrage_factor`, `tag_drop_escapes_import_floor` (trusted), `import_floor_invariant_refuted` |
| 4'. (#1687, protocol 10.0.0) An accepted spend of fully import-tagged coins pays at least `F`; a lone tagged coin keeps at least 95% of its weight | import floor from the inputs + retained-weight floor | **proven** (model), for every fee curve, ring factor and import set; the retained-weight floor does **not** bind when a same-amount decoy lacks the cluster (that drop still pays `F`) | `importFloorIn_full`, `import_floor_honoured₁₀`, `tag_shed_rejected₁₀`, `same_amount_decoy_drop₁₀` (records added; listed in `Theorems.lean` by a follow-up, two-PR rule) |

Every counterexample is also checked end to end, on a real ledger with
validly signed transactions, in `botho/src/ledger/cluster_tag_invariant_tests.rs`;
the #1857 and #1858 tests there now assert the rejection
(`*_rejected_since_1870`, `*_rejected_since_1863`), and the #1687 test
asserts the 10.0.0 rejection, or the import floor where the drop is still
accepted (`import_tag_drop_is_rejected_or_pays_the_import_floor`).

## The difficulty and slot models

`Botho/Difficulty/` and `Botho/Consensus/SlotDuration.lean` (#1841) model the
node's per-block difficulty controller, the header timestamp rules it is fed
by, and the node-local dynamic slot duration.

Scope: the header timestamp layer (`tsValid`, `applyChain`, `observedSecs`,
the 2 h `MAX_FUTURE_TIMESTAMP_SECS`) is the protocol ≤ 7.x rule set, under
which there was no median-time-past rule. Its counterexamples are finding
#1848, fixed in protocol 8.0.0 (#1869): `timestamp > MTP(last 11)`, a 60 s
future bound, a 3 s floor on the observed difficulty gap
(`observed_block_time`), and minters stamping `max(now, MTP + 1)`. The
theorems stay as the historical record of the 7.x bug; the 8.0.0 timestamp
rules have a separate [timestamp model](#the-timestamp-model). The controller kernel and the slot-duration code are
unchanged in 8.0.0, so their models and DRT legs cover the code on main.

| Lean | Rust | Status |
|------|------|--------|
| `Botho.Difficulty.step` | `EmissionController::compute_time_adjusted_difficulty`, `botho/src/block.rs` | proven + tested |
| `Botho.Difficulty.run`, `recordBlock` | `EmissionController::record_block`, `botho/src/block.rs` | proven + tested |
| `Botho.Difficulty.observedSecs` | `observed_secs` in `node::commit_block_locked`, `botho/src/node/mod.rs`, protocol ≤ 7.x (8.0.0: `observed_block_time`, floored at 3 s, separately modelled by `Botho.Timestamps.observedGap`; tested in Rust by `production_feed_floors_gaps_post_1848`) | proven (model); read from the 7.x code |
| `Botho.Difficulty.tsValid`, `applyChain` | the protocol ≤ 7.x timestamp checks in `ledger/store/validation.rs` and `consensus/validation.rs` (`ts >= tip`, `ts <= now + 2 h`; 8.0.0 replaced them with `ts > MTP`, `ts <= now + 60 s`) | proven (model) for 7.x; the validator clock `now` is a trusted input |
| `Botho.Consensus.slotSecs`, `recordBlock` | `ConsensusService::current_slot_duration`, `record_block`, `botho/src/consensus/service.rs` | proven + tested; the f64 rate is modelled exactly (trusted agreement domain in the module docstring) |

Each kernel is tested twice: by the random legs below, and by the fixed
vectors in `Botho/Difficulty/Vectors.lean`, which the Lean kernel checks
against the model (`lake build`) and the Rust tests in
`botho/src/formal_drt_difficulty/tests.rs` check against the code. The
headline results (convergence to `MAX_DIFFICULTY` in 8 blocks, #1842; the
floor in 55 equal-timestamp blocks, and why that is not a cheap halt; the
forward-dating lockout counterexample, #1848, protocol ≤ 7.x, fixed in 8.0.0
by #1869) are in the docstrings of `Botho/Difficulty/Warp.lean`. The Rust
test `forward_dated_tip_accepts_honest_minting_tx_post_1848` pins the fixed
validator behaviour. `compute_time_adjusted_difficulty` is
integer-only and a candidate for the `botho` extraction lane (#1838); this
model does not depend on it.

## The timestamp model

`Botho/Timestamps.lean` (#1941) models the protocol 8.0.0 rules retained in
9.0.0. The five listed claims are **proven (model)**; the skew-only acceptance
claim is **refuted in the timestamp-rule model**. The five pure helpers and three production scalar predicates have **tested**
agreement under the finite `timestamps` DRT domain (#1942, #1943): u64
scalars, generated window lengths 0..32, and pinned boundary inputs. Decoding
and replay accept arbitrary finite window lengths; codec tests also use 64
entries. This testing is evidence, not a proof of Rust equivalence. The
ledger predicates cover full-u64 clocks; minting covers only `now ≤ U-60`.
Above that cap only the nested minting result reports
`validator_now_out_of_domain`; helper and ledger results remain available.
Complete-validator equivalence, clock acquisition/failure, SCP scheduling and
physical-clock realizability remain outside the DRT claim.

| Lean (`Botho.Timestamps.*`) | Rust | Status and scope |
|------|------|------|
| `tail11`, `slots`, `mtp`, `roll`, `nextMin` | `median_time_past_of`, `ChainState::rolled_timestamps`, `next_min_timestamp` in `botho/src/ledger/mod.rs` | tested (finite helper DRT); fixed padded median, rolling history and saturated successor |
| `honestStamp` | `minting_timestamp` in `botho/src/node/minter.rs` | tested (finite helper DRT); maximum of supplied clock and minimum stamp |
| supplied minter clock | clock-reading caller in `botho/src/node/minter.rs` | read from source; clock read not executed by DRT |
| `mtp < stamp`, `futureCap`, `accepts` | `timestamp_after_mtp`, `ledger_timestamp_within_future_bound` in `ledger/mod.rs`, called by ledger C2b | direct scalar DRT over full u64; lower and saturated upper primitives plus conjunction observed independently |
| `accepts` on `v ≤ U-60` | `timestamp_after_mtp`, `minting_timestamp_within_future_bound`, called by contextual minting | direct scalar DRT on the no-overflow domain; ordinary addition preserved; caller checks future before MTP |
| upper clause of `accepts` on `v ≤ U-60` | `minting_timestamp_within_future_bound`, called by intrinsic minting | direct scalar DRT on the no-overflow domain; only the upper bound, no MTP check |
| `observedGap` | `difficulty::observed_block_time` in `botho/src/block.rs` | tested (finite helper DRT); zero parent gives `None`, other gaps are saturated subtraction floored at three |

Let `U = 2^64-1`. Public claims take natural timestamps with explicit
`≤ U` bounds and `Bounded(w) := ∀ t ∈ w, t ≤ U`. The window can be any finite
length, including greater than eleven. Its entries are oldest first: take
the last eleven, pad the front with zeroes to eleven slots, sort ascending,
and select index five. The model proves the slot length and index bounds.
Rolling keeps the final at most ten entries and appends the stamp.
`nextMin(w) = min(MTP(w)+1,U)`, `honestStamp(w,n) = max(n,nextMin(w))`, and
`futureCap(v) = min(v+60,U)` use the existing `Botho.satAdd` convention.

The universal results establish that windows of length at most five have
zero MTP, and that a stamp strictly above MTP cannot decrease the rolled
MTP. The latter assumes neither sorted inputs nor chain reachability. Its
proof characterizes a sorted median by the count of slots below a threshold;
rolling drops one normalized slot and adds a stamp above the old median.
For an accepted history, induct on its transitions: the base MTP is equal to
itself; each accepted step supplies the strict-lower-bound premise, preserves
bounded entries through drop/append, and composes the one-step inequality
by transitivity. Thus MTP never decreases along a timestamp-accepted chain.

The exact honest-stamp result is
`accepts(w,honestStamp(w,n),v) ↔ MTP(w) < futureCap(v) ∧ n ≤ futureCap(v)`.
It requires actual median headroom as well as the clock bound. The typed
counterexample checks six accepted `(stamp=1060, validatorNow=1000)`
transitions from an empty window, reaching six 1060 entries and MTP 1060.
Next minter and validator clocks are both 1000, so skew is zero and below
60; the honest stamp is 1061 and is rejected by the 1060 cap. The earlier
stamps are accepted scalar inputs, not asserted honestly generated stamps.
This refutes a skew-only guarantee in the timestamp-rule model, including
the unconditional acceptance prose in the minter source. It does not prove
realizability under SCP pacing, physical clocks, PoW or complete ledger
acceptance. Header/minting equality, error order and clock-read timing are
outside the model.

At eleven `U` stamps, MTP, the saturated successor, and every bounded-clock
honest stamp all equal `U`, failing the strict lower bound. At `v=U-60`,
ordinary `v+60` equals `U`; above that boundary, the ledger caps while the
minting validators' ordinary u64 addition is outside the agreement domain.
These artificial maximum-time inputs check arithmetic boundaries, not a
present-day exploit. Kernel examples also cover short/long, duplicate and
unsorted windows, ignored prefixes, rolling truncation and backward/equal
gaps. Every `Some` observed gap is at least three; parent zero remains
`None`. No downstream division-safety theorem follows here.

## Differential random testing (DRT)

`formal-drt/` (#1706, #1707) checks that the Lean model and the Rust code
compute the same function. It has two parts:

- `formal-drt/lean/`: a Lake package with the executable `botho-model`. It
  `require`s the model by path (`../../formal`), reads JSON-lines requests on
  stdin (`{"fn": ..., "args": {...}}`) and writes one JSON-lines response per
  request (`{"ok": ...}` or `{"err": ...}`). Integers are decimal strings and
  byte strings are hex. The codec and dispatch are in `Drt/Codec.lean`.
- `formal-drt/` (Rust): the dev-only crate `botho-formal-drt`. It holds one
  long-lived `botho-model` process, generates inputs with `proptest`
  (biased to 0, 1, `u64::MAX`, the eligibility and cap boundaries, empty and
  single candidate sets, and `MAX_LOTTERY_CANDIDATES` ± 1), sends them in
  batches, runs the Rust function on each, and compares the two JSON values.
  A Rust panic counts as a mismatch. A mismatch is shrunk and appended to
  `formal-drt/regressions/<leg>.jsonl`, and every run replays those files
  first.

**Why it lives outside `formal/`.** The gates (above) accept exactly one
shape of the gated package: no `.lean` file under `formal/` besides
`Botho.lean`, `Botho/**` and `Theorems.lean`; no `[[lean_exe]]` or
`[[require]]` in `formal/lakefile.toml`; and no `IO`, `#eval` or `extern` in
model sources. An executable needs all three, so it is a separate package
next to `formal/`, never scanned by the gates or built by the verify job. It
runs the gated model sources unchanged. The Rust crate is kept beside it so
that DRT changes do not touch the gated tree: the formal gate job does not
re-run for them, and the self-tests copy only Lean package inputs, never a
Cargo build.

The Rust crate is its own Cargo workspace (excluded from the root one, like
`fuzz/`): it enables botho's `test-fixtures` feature, which must never be
unified into a node build. It uses that feature for two things: seeding a
real LMDB ledger (the `lottery_candidates` leg runs the production reader)
and `botho::ledger::formal_drt`, which runs the production
`lottery_v2::drawing` over an in-memory `DerivationReads` double (the
`drawing` leg).

### Legs

| Leg | Rust | Model |
|-----|------|-------|
| `lottery_seed_v2` | `consensus::lottery::lottery_seed_v2` | `lotterySeedV2` |
| `is_eligible` | `LotteryCandidate::is_eligible` | `Candidate.isEligible` |
| `count_eligible` | `count_eligible` | `countEligible` |
| `sybil_reward_cap` | `sybil_reward_cap` | `sybilRewardCap` |
| `split_fees` | `LotteryFeeConfig::split_fees` | `splitFees` |
| `reward_cap` | `consensus::lottery::reward_cap` | `rewardCap` |
| `compute_pool_accounting` | `consensus::lottery::compute_pool_accounting` | `computePoolAccounting` |
| `carryover_after` | `LotteryPoolAccounting::carryover_after` | `Accounting.carryoverAfter` |
| `draw_winners_with_seed` | `draw_winners_with_seed` | `drawWinnersWithSeed` |
| `draw_lottery_winners_seeded` | `consensus::draw_lottery_winners_seeded` | `drawLotteryWinnersSeeded` |
| `lottery_candidates` | `Ledger::get_lottery_validation_candidates` over a real LMDB ledger | `lotteryCandidates` |
| `drawing` | `lottery_v2::drawing` over the in-memory double | `drawing` |
| `lottery_sim` | Actual `LotterySimulation::apply_production_block` and separate node cap/accounting/draw calls; independent per-step public-state checks | One composed `Drt.lotterySim` result in both expected slots; Lean advances its own arithmetic carryover |
| `merge_weighted` | `ClusterTagVector::merge_weighted` since #1863 (saturating; a panic would be reported as `{"panic": true}` and mismatch) | `Botho.Tags.mergeWeightedSafe` |
| `ring_elapsed_centroid` | `bth_cluster_tax::ring_elapsed_centroid` | `Botho.Demurrage.ringElapsedCentroid`; exact weighted saturated ages, one final floor, zero total weight gives zero |
| `ring_elapsed_quantile` | `bth_cluster_tax::ring_elapsed_quantile` | `Botho.Demurrage.ringElapsedQuantile`; one scalar, saturated sorted ages and clamped nearest rank; values ignored |
| `demurrage_composition` | `bth_cluster_tax::{capitalized_reset_charge, demurrage_settlement_charge, spend_demurrage_charge}` | `Botho.Demurrage.{exactResetCharge, exactSettlementCharge, exactSpendCharge}`; three results per case, independent typed inputs and fixed production wrapper horizon |
| `monetary` | Actual mainnet policy phase/halving, both tail wrappers, library and node block reward, lottery bps/share and demurrage | `Botho.Monetary` and `Botho.Supply.Block`; independent u64 inputs and arithmetic-safe wide supply 0..A, including modulo cast loss; see [domain/coverage](MONETARY.md#monetary-drt-domain-and-replay-1922) |
| `cluster_tag_inheritance` | 8.0.0 (#1870): `first_malformed_output_tags` (Rule 1), then `ValidationReads::verify_cluster_tag_inheritance(tx, height)` (Rule 2, C6) over the in-memory double `botho::ledger::formal_drt_tags`; fixtures include #1870's `rules_golden_vectors` | `Botho.Tags.inheritOk₈`, the import cluster derived from the height in the DRT driver (and the failing rule's numbers) |
| `effective_cluster_wealth` | 9.0.0 (#1876): `ValidationReads::effective_cluster_wealth_from_outputs` over the same double (exact 256-bit numerator) | `Botho.Tags.effectiveWealthExact` (`effectiveWealth` is the pre-9.0.0 saturating model, kept as the record) |
| `import_floor_factor` | `ValidationReads::import_floor_factor_from_outputs` over the same double | `Botho.Tags.importFloor` |
| `difficulty_step` | `EmissionController::compute_time_adjusted_difficulty` (through `botho::formal_drt_difficulty`) | `Botho.Difficulty.step` |
| `difficulty_run` | `EmissionController::record_block`, one `Some(gap)` per block (half the runs with gaps floored at 3 s, as `observed_block_time` feeds them since 8.0.0) | `Botho.Difficulty.run` |
| `slot_duration` | `ConsensusService::record_block` then `current_slot_duration` | `Botho.Consensus.slotSecs` |
| `timestamps` | `median_time_past_of`, `ChainState::next_min_timestamp`, `rolled_timestamps`, `minting_timestamp`, `observed_block_time`; `timestamp_after_mtp`, `ledger_timestamp_within_future_bound`, `minting_timestamp_within_future_bound` (same predicates called by production) | `Botho.Timestamps`; exact u64 decimal strings and arbitrary finite replay windows; generated lengths 0..32 |

Historical timestamp validation (#1950 / PR #1951) passed 10,000 cases at seed 1915 and
31 replays in its private target; that source's all-leg tier passed 240,000 cases
and 141 replays at seed 1707. These are not current lottery-simulation counts.
The timestamp leg preserves all 30 helper fixtures from #1942 and adds the
missing `U-61` clock boundary. Each candidate/honest lower and ledger upper
primitive, ledger conjunction, and in-domain minting contextual/intrinsic
result is classified separately, alongside common/out-of-domain populations.
The [regression note](../formal-drt/regressions/README.md#timestamps) records
the schema, exact cases, commands and retained validation evidence. Its seven
linked zero-skew snapshots are a scalar timestamp-rule history, not seven
complete ledger admissions or a claim of SCP/physical-clock reachability.

The cluster-tag legs (#1840) live in `formal-drt/src/legs_tags.rs` and
`formal-drt/lean/Drt/Tags.lean`. Their mutation check: a C6 tolerance of
`max(0)` instead of `max(1)`, a pruning test of `>` instead of `>=` in
`from_pairs`, and a looser clamp in the import floor are each caught within
the first 1,024 cases.

The `drawing` double reimplements the candidate read over an in-memory
table (it cannot use LMDB); the `lottery_candidates` leg checks the
production reader against the same model, so the two legs together tie both
to the model.

### Stateful lottery simulation and divergence inventory

`lottery_sim` (#1972) calls the actual
`LotterySimulation::new_production` / `apply_production_block` entry and the
node's reward-cap, pool-accounting and seeded-draw functions separately, with
an explicit 800-permille fee split and default draw configuration. Its JSON
result has `node` and `simulator` arrays (each step's cap, accounting, draw,
paid amount and carryover), plus `simulator_state: {ok, failed_checks}`.
Lean's `Drt.lotterySim` composes the existing model functions and advances
its **own arithmetic carryover**. It puts that **one** result in both expected
array slots; those slots are not independent Lean implementations. The expected
state slot is `{ok: true, failed_checks: []}`. A state-only failure therefore
fails the same JSON equality as an arithmetic or draw mismatch, even when the
node and simulator's returned projections agree.

The Rust reference inventory advances from node results, never from simulator
outputs. It independently resolves later references to prior awards, preserves
original records and predicts output inventory, owner credits and carryover at
every step. It observes the actual simulator's public outputs, UTXOs, owners,
metrics, current block and awards. Fresh synthetic IDs, original-output
immutability and owner/counter bookkeeping are **Rust state-application
invariants**; Lean does not reconstruct owner inventory or prove cryptographic
outpoint identity. Private next-ID/last-height behavior is exercised through
successive transitions and the production module's rollback tests, without
exposing another production API.

The valid comparison domain is 1–4 strictly increasing blocks, at most 10,000
initial outputs and 10,000 candidates per snapshot, decimal-string u64 fields
and u128 carryover, and exact-width hexadecimal IDs (36 bytes) and V2 seeds
(32 bytes). Constructive generation covers representable owner, allocator and
metric state, including boundary values and sparse maximum-sized fixtures;
shrinking stays within that domain. These bounds and guards are simulator/input
representation restrictions, not consensus admission rules. Rejected identities,
mappings, counters and malformed wire inputs have separate rejection/atomicity
tests and are not silently filtered out of a purported full-domain success run.
Each sequence gets one composite class (`kind/size/state`) so its histogram
counts sequences once. The ordinary runner replays committed fixtures first,
including the restored state-only mutation input, and shrinks any new mismatch.

The externally supplied assumptions are candidate discovery, window/order and
spent-record policy; immutable initial registration and owner mapping;
already-collected fees and upstream fee-floor/mode resolution; emission share
and block-reward scheduling; and already-derived V2 seeds. Lean receives resolved
candidate snapshots, including node-derived prior awards, rather than discovering
them. Wider DRT implements SHA-256 in Lean with startup FIPS vectors and accepts
RandomX outputs from Rust where applicable. This leg starts **after V2 seed
derivation**: it does not test that derivation, candidate discovery, accepted-chain
reachability, cryptographic ownership or economic validity. Proven model
statements retain their existing hypotheses; finite correspondence and Rust
state checks do not strengthen those theorems.

[`sim-divergences.json`](../formal-drt/sim-divergences.json) documents the separate
production, research and external-assumption surfaces. It enumerates legacy
ticket/distribution/selection families, floating arithmetic and application,
fee/time/unit conventions, every sweep policy and the faithful analysis's
candidate remedies and window Monte Carlo. The four stable execution IDs are
`sweep.selection`, `sweep.sybil_brake`, `sweep.reward_cap` and
`sweep.reward_cap.faithful` (the last is `lottery-reward-cap-sweep --faithful`).
The flag selects analysis, not the production entry. Shared `FeeModelMode`
labels remain `consensus` and `consensus+relay`. Existing node/simulator constant
parity and faithful mirror goldens remain separate checks; constant agreement
does not establish full-state or economic equivalence.

The strict validator lives only in `legs_sim.rs` tests. Its required identities,
source references and enum members are independent of the JSON; it rejects
unknown/malformed/duplicate/missing declarations and empty explanations.
There is no exemption field or production mismatch waiver. Runtime registration,
comparison, generation, shrinking and replay do not read this document. The
existing actual-state corruption test also checks that an extended valid research
declaration cannot hide a production mismatch. Stable IDs are the handoff to
#1970's later CLI-label/table/conclusion audit. This inventory does not change
the #1682/#1761 profitability refutation or complete parent #1969/#1709.

**Historical prerequisite evidence (not a run of #1973).** The following records
bind results to their tested sources; subsequent inventory changes need their own
final-head CI report, including the restored witness replay.

- Final #1974 head `d949406d0ac3b00d806bf3b0afbc5023a25be525`, actual CI checkout
  `058dfd7c27ebb7e9d5161c839a6332ddf88ff792`, tree
  `bbb7c4504d20035aa10d1b54dee4100ae53cf0ff` (also merged main
  `a0e0b70f7360ef279f79e84c001d29b9cbd6c648`):
  [run 42360160073 / job 128366639221](https://github.com/botho-project/botho/actions/runs/42360160073/job/128366639221),
  [`formal-drt-report` artifact 13569074425](https://github.com/botho-project/botho/actions/runs/42360160073/artifacts/13569074425).
  Seed 1707: 25 legs × 10,000 generated cases, 166 total replays, zero mismatches;
  `lottery_sim` has 25 replays. Report SHA-256:
  `c9d63e1aad427ac90e3be589e69798bba193c6ad98fbaed30164702c6666df19`.
- [Control run 42359239325 / job 128363877879](https://github.com/botho-project/botho/actions/runs/42359239325/job/128363877879),
  attempt 1, [`lottery-sim-mutation-control` artifact 13568309455](https://github.com/botho-project/botho/actions/runs/42359239325/artifacts/13568309455):
  source `8d053866e8831ad1db1684e2b2f2d3db00255955`, tree
  `f647c445ee4972197bf2f15d51e70c8b69d9b7b4`. The patch changes the actual simulator's
  `metrics.total_distributed` commit to `total_distributed.saturating_add(1)`.
  Generation (32 cases) detected and shrank one state-only witness
  (`0/total_distributed`, exit 1); saved replay (`--cases 0`, one replay) exited 1.
  Exact-tree restoration/rebuild made that replay pass (exit 0); 10,000 restored
  cases at seed 1707 plus one replay also passed. Draw/accounting projections
  remained equal during the failure.
- The control artifact retains `actual-simulator-mutation.patch`, `commands.json`,
  `identity.json`, reports/logs and the generated fixture. CI-local mutated commit
  `3f531d13a0d33e552305a1ed49df65b32615eb01` has tree
  `7b26de2382923cab615f4f0d9477c1debc1521b8`; restored commit
  `86081e715b231646e4a17ae8bbf3d87026236eb1` returns to the control tree above.
  SHA-256 digests: model
  `1279ccb239f25c08f44df2ba2d2e338b665365bc21c5d4fc29dc916081ed1635`;
  clean / mutated / restored runner respectively
  `34924ba09bba3a9ff63002f104bce5dcb30ca861ee0022660c913faa34c8e97e`,
  `3c9c51abab2c6c1d4d8ac673686e60dc47273efe120fe3e9e4da9da4003adb04`,
  `3c8372a9100775aaa19da1faf077f46040331d90451c9a2ed779cd5bc7f0123c`.
  Original artifact ZIP SHA-256:
  `e389841af219bc28be8f4792a69ad3241b8da2082dba8da93f8f9b3efb8142ad`.
- Saved witness SHA-256:
  `d4a2c1e3001e3a37c31622c56080061b7cbeefa699faec81f41d56fdfd59e9ce`;
  its input is among the 25 committed `lottery_sim.jsonl` regressions. It is an
  artificial mutation detector, not an observed production bug. The earlier
  [control attempt 42358155396](https://github.com/botho-project/botho/actions/runs/42358155396)
  timed out after 270 seconds in repeated generation/shrinking, restored/rebuilt
  in cleanup and **did not reach replay**. That attempt is not a successful
  mutation sequence; generic duplicate-shrink work remains #1975.

Retained operator copies are in `.loom/diagnostics/issue-1972-evidence/` and
`.loom/diagnostics/issue-1974-root/`, outside worktree cleanup; they are not new
tracked deliverables or substitutes for source-bound CI evidence.

### Running it (macOS or Linux)

Prerequisites: elan (see [Building locally](#building-locally)), the pinned
Rust toolchain, and `cmake` (RandomX). From the repository root:

```bash
(cd formal-drt/lean && lake build)          # builds the model and botho-model
cp Cargo.lock formal-drt/Cargo.lock         # build the node's exact dependency versions
cd formal-drt
cargo run --release -- --cases 10000        # the per-PR tier: seed 1707, all legs
cargo run --release -- --cases 1000000 --seed random --legs drawing,reward_cap
cargo run --release -- --list               # leg names
```

Options: `--cases N` (per leg), `--seed S` or `random`, `--legs a,b`,
`--batch B`, `--json PATH` (machine-readable report), `--regressions DIR`
and `--no-write-regressions`. The report prints, per leg, generated `cases`, `compared` generated inputs,
`shrink_attempts`, fixtures replayed, unique generated witnesses plus failing
replays, and a histogram of compared-input outcome classes (winner
counts, error kinds, saturated or exact sums), which shows which branches the
cases reached. On macOS the first `drawing` case builds a RandomX light cache
for each epoch of the parent headers (a few seconds); everything else needs no
special setup. The legs are independent, so a long run can be split across
processes with `--legs`.

CI (`.github/workflows/formal-drt.yml`) retains the per-PR tier: seed 1707,
10,000 cases per leg and a five-minute run budget. Lightweight nightly-helper
tests run alongside it; PRs do not run the million-case tier or receive an
issue-writing token. The monetary leg and bounded actual-simulator vectors are described in
[MONETARY.md](MONETARY.md#monetary-drt-domain-and-replay-1922); they do not
claim whole-simulator equivalence.

### Nightly tier and reproduction (#1817)

`.github/workflows/formal-drt-nightly.yml` schedules `main` at **03:23 UTC**.
It builds the pinned Rust runner and Lean model once, discovers a nonempty,
unique leg list through `--list`, resolves one random u64 seed, and requests
**1,000,000 generated cases per discovered leg**, plus every committed replay
fixture. The decimal seed remains a string, including 0 and `u64::MAX`.
Dispatch accepts the same defaults or a numeric seed, 1–1,000,000 cases and
a validated comma-separated subset of discovered legs. No fixed leg list is
embedded in the workflow.

`formal-drt/scripts/nightly.py` uses at most four runner/model process groups.
Each leg has a 60-minute deadline; the orchestration has a 160-minute total
deadline inside a 165-minute step and 240-minute job. Separate build budgets
are 15 minutes for Lean and 45 for Rust, leaving collection time. A leg that
cannot start or finish within those bounds is an explicit infrastructure
failure; its requested case budget is never silently reduced. These initial
budgets still require a full Linux measurement after registration. The
reporter has ten minutes. Repository-wide concurrency serializes scheduled
and dispatched runs with cancellation disabled.

To exercise the same helper locally, from the repository root after the
build commands above:

```sh
python3 formal-drt/scripts/nightly.py prepare --out /tmp/drt-nightly \
  --seed 18446744073709551615 --cases 100 --sha "$(git rev-parse HEAD)"
python3 formal-drt/scripts/nightly.py run --out /tmp/drt-nightly \
  --runner formal-drt/target/release/botho-formal-drt \
  --model formal-drt/lean/.lake/build/bin/botho-model \
  --regressions formal-drt/regressions --workers 4
python3 formal-drt/scripts/nightly.py summary --out /tmp/drt-nightly \
  --summary /tmp/drt-nightly-summary.md
```

Use a fresh output directory for each attempt. If Cargo uses a custom target
directory, pass the actual runner path reported by `cargo metadata`.
The helper leaves committed fixtures unchanged. The artifact contains
metadata (seed, source SHA/ref, run/attempt URL, budgets and selected legs),
original fixture copies, per-leg logs/reports/statuses, appended JSONL records
and diffs, and `failing/<leg>.jsonl` for every confirmed witness. Replayed
mismatches are included even when no new line was appended. Artifacts have
unique run/attempt names and 14-day retention on clean and failed runs.
Ordinary failures still collect and upload; hard cancellation or runner loss
can prevent final upload.

A clean leg requires exit 0, a valid report with the exact seed/leg/budget,
exact original replay count, no mismatches and the full generated count.
Missing, malformed, unwritable, truncated or inconsistent reports fail as
infrastructure even when the runner returned 0. A confirmed mismatch requires
exit 1 and complete unequal Lean/Rust observations; caught `rust_panic` values
are explicitly identified. `lean_err`, timeouts, model-start/protocol failures
and other statuses are infrastructure, never semantic-counterexample issues.
Other independently valid legs are still collected and may be reported.
`cases` counts generated batches; early stopping can leave part of the last
batch uncompared. `compared` excludes fixture replays and shrink probes;
`shrink_attempts` counts every generated shrink invocation, even when its
canonical witness duplicates an earlier one. Every stored fixture is replayed
first. Each failing replay consumes the internal allowance (default five),
saturating at zero; each generated shrink consumes one remaining attempt before
deduplication. Exhaustion stops before the next generated Rust comparison.
Distinct witnesses are still retained, and `--no-write-regressions` changes only
persistence. `--cases 0` remains replay-only. Each shrink tree retains its
5,000-check bound, not a claim of global minimality or a process timeout.

Historical reports without either new counter keep their previous contract.
When either counter is present, both must be natural integers with
`shrink_attempts <= compared <= cases <= cases_per_leg`; class totals cannot
exceed `compared`. Clean new reports require the full requested comparisons and
zero shrink attempts. Partial or inconsistent accounting is infrastructure,
not a semantic success. These operation bounds do not classify a model hang,
model error or missing report as a counterexample.

Only the separate reporter job has `issues: write`. It checks out the
**trusted default-branch helper**, reads artifacts as data, verifies their
run identity against the Actions context, and never executes artifact
binaries or branch scripts. Events are restricted to this repository's
schedule and authorized same-repository dispatches. No PAT or
`pull_request_target` is used. Reporting success cannot erase a failed test
job, and API/publication failures fail the reporter.

Witness identity v1 is SHA-256 of UTF-8 canonical JSON
`{"leg":...,"input":...}`: recursively sorted keys, compact separators,
array order preserved and exact integer/string values. Seed, outputs, SHA
and run identity are excluded. The exact issue marker is
`<!-- botho-drt:v1 sha256:HASH -->`. The reporter paginates all issue states
and matches the marker in its automation-generated issues, rather than
relying on search indexing or mutable labels. It creates absent findings
with `area:testing` and `loom:triage`, updates/reopens recurrent findings, and
makes the same run attempt a no-op. Issues include observed outputs, the
exact seed/revision, artifact link and build/replay commands. Large fixtures
are explicitly truncated in prose; the complete JSONL remains in artifacts.

Pre-merge local evidence is recorded in
[`formal-drt/nightly-validation.json`](../formal-drt/nightly-validation.json).
At source `de9b8c33` on 2026-11-03, a real macOS run completed 100 generated
cases in each of 15 dynamically discovered legs plus 27 fixture replays with
seed `18446744073709551615`. A temporary Rust-only `split_fees` mutation
incremented burn by one. Seed 0 found and shrank it to fees 0 / permille 800:
Lean `["0","0"]`, Rust `["0","1"]`. The saved fixture reproduced the mismatch
with no new appended record; restored Rust passed the same fixture. The
mutation was removed and the baseline rebuilt. The receipt preserves binary
and model hashes, the temporary patch and reports. Mock helper/API tests
exercise failure handling; they do not establish Rust correspondence.

**Registration handoff:** keep #1817 open after the implementation PR.
GitHub requires [default-branch registration for dispatch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch);
[scheduled workflows run from the default branch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule).
Once registered, attach a clean full-default Linux
dispatch (every discovered leg at 1M plus replay), measured duration/artifacts,
and the first scheduled main run. Then dispatch a small, isolated mutation
branch; retain its issue, seed, shrunk witness and reproduction, repeat to
verify updating the same issue, and remove the branch/close the clearly
identified validation issue. Local evidence does not claim those live
publication or schedule checks have happened. DRT remains finite test
evidence, not a proof of model equivalence.

### Adding a function

One generator, one Lean entry point and one comparison:

1. **Model.** Add the definition under `Botho/`, with a docstring naming the
   Rust function and file, and import it from `Botho.lean`.
2. **Lean entry point.** In `formal-drt/lean/Drt/Codec.lean`, add an arm
   `| "my_fn" => ...` to `dispatch` that decodes the arguments, calls the
   model and encodes the result (the decoders and encoders for the common
   types are in the same file).
3. **Rust leg.** In `formal-drt/src/legs.rs`, add a type implementing `Leg`:
   `NAME` (the same `"my_fn"`), an `Input` type, `strategy()` (the generator;
   reuse the edge-biased strategies in `src/gen.rs`), `args()` (the request
   arguments, in the wire format of `src/wire.rs`) and `rust()` (the Rust
   result in the same wire format). Optionally `classify()` for the
   histogram. List it in `LEGS` in `src/main.rs`.
4. Run `cargo run --release -- --legs my_fn --cases 100000`, then inject a
   bug into the Rust function (an off-by-one, a wrong saturation) and check
   that the leg catches it within the 10,000-case tier. Revert the bug.
5. If the leg needs the Rust files it mirrors to trigger CI, add them to the
   `paths` of `formal-drt.yml` and to the `pattern` in `formal.yml`.

A mismatch is either a model bug or a Rust bug. Fix a model bug in the
model. File a Rust bug as an issue with the shrunk fixture; do not change the
model to match it.

## Gates

CI (`.github/workflows/formal.yml`) runs two jobs on opposite sides of a trust
boundary (#1799; see [Limits](#limits)):

- **build** (untrusted) checks out the pull request, runs the gates below
  (which run `lake build`) and the self-tests, and uploads the compiled
  `.olean` files as the artifact `formal-lib`. Its results are fast feedback
  only: model code runs during the build and could fake them.
- **verify** (trusted) runs `scripts/verify-artifacts.sh` from the **base**
  commit over that artifact. See [The verify job](#the-verify-job).

The required check is `formal-gate`. It passes when both jobs succeeded, or
when both were skipped because nothing under `formal/`, the workflow, or a
mirrored Rust file changed.

The in-tree gates (#1770). The build job runs them in full; the verify job
runs the base commit's `check-sorry.sh --no-build`, which is the layout check
and the source scan below, over the pull request's sources as data:

- **Build configuration.** Before anything else, both gates check that
  there is no `lakefile.lean` (a Lean program that runs on every `lake`
  call) and that `lakefile.toml` passes the allow-list in
  `scripts/gate-common.sh`:
  - *Layout:* no package `srcDir`, one `[[lean_lib]]` named `Botho` with no
    `srcDir`, `roots`, `globs` or `root`, and no `[[lean_exe]]`. These are
    the keys the verify job's layout check compares with the base's
    `lakefile.toml`. So Lake compiles only `Botho.lean` and the modules it
    imports, all in `Botho/`.
  - *Keys:* every other key must be on an allow-list of keys that cannot
    load or run code: package metadata (`name`, `version`, `defaultTargets`,
    `description`, `keywords`, `homepage`, `license`) and `leanOptions`
    with plain values for a short list of options (`autoImplicit`,
    `relaxedAutoImplicit`, `maxHeartbeats`, `maxRecDepth`, the
    `synthInstance` limits, `linter.*`, `pp.*`). A `[[lean_lib]]` may carry
    only its `name`. Everything else is rejected, known or unknown:
    `[[require]]`, `moreLeanArgs`, `weakLeanArgs`, `moreServerOptions`,
    `moreLinkArgs`, `plugins`, `dynlibs`, `precompileModules`,
    `extraDepTargets`, `buildDir`, `testDriver`, and options such as
    `debug.skipKernelTC`.
  - *No dependencies:* `lake-manifest.json`, if present, must list no
    packages.

  `lean-toolchain` is not pinned by the gates, so toolchain bumps pass; see
  [The verify job](#the-verify-job).
- **Source scan** (`scripts/gate_scan.py`). Both gates run it before their
  first `lake` call, and stop if it fails, so the in-tree gates never build a
  tree the scan rejected. It checks:
  - *Layout.* The model is `Botho.lean` and `Botho/**/*.lean`, the manifest
    is `Theorems.lean`, all regular files (no symlinks under `Botho/`), and
    there is no other `*.lean` file outside `.lake/` and `scripts/`.
  - *Import allow-list.* Every scanned file may import only `Botho` modules
    that are scanned files (`import Botho.X.Y` must be `Botho/X/Y.lean`). Not
    `Lean`, `Std`, `Lake`, `Init.*`, Mathlib or anything else. A model that
    needs another import must extend the list in `gate_scan.py` in a reviewed
    diff. `prelude`, `module`, import modifiers (`public`, `meta`,
    `import all`) and an `import` after the header are rejected. Multi-line
    headers, comments between imports and `«»`-quoted names are parsed the
    way Lean parses them.
  - *Tokens*, as listed under the sorry gate below.
- **Sorry gate** (`scripts/check-sorry.sh`) fails if `lake build` warns that a
  declaration uses `sorry`, or if a sweep of the compiled library finds any
  declaration that reaches `sorryAx`. The sweep (`scripts/AxiomReport.lean`)
  walks declaration bodies directly, so it still works when the warning is
  hidden (for example by `#guard_msgs`). The gate also fails if the source
  scan finds `sorry`, `sorryAx`, `admit` or `native_decide` (bare,
  `_root_.`-qualified, or with explicit universes such as `sorryAx.{0}`)
  outside comments, string literals, raw strings (`r#"..."#`) and char
  literals. Code inside `{...}` string interpolation is scanned.
  `native_decide` trusts the compiler, so it is allowed only on a line marked
  `-- formal-gate: allow native_decide`, and the compiler-trust axioms it
  introduces (such as `Lean.ofReduceBool`) must also be on the allow-list.
  The scan also rejects commands that run code or hide messages at compile
  time (`#eval`, `#guard`, `#guard_expr`, `run_cmd`, `run_elab`, `run_meta`,
  `run_tac`, `by_elab`, `#guard_msgs`, `initialize`, and `#print` outside
  `Theorems.lean`), the evaluators that run compiled code while the model is
  elaborated (`native` in any form, such as `decide +native` or
  `native := true`, and `reduceBool`, `reduceNat`, `ofReduceBool`,
  `ofReduceNat`, `trustCompiler`), the prelude IO surface (`IO`, `EIO`,
  `BaseIO`, `ST`, `EST`, `EStateM`, `RealWorld`, `Void`, `System`,
  `FilePath` as a name or any dotted component, so `IO.FS`, `IO.Process`,
  `IO.Ref` and `ST.Ref` too; the `unsafe` keyword; and any name starting
  with `unsafe` or ending in `Unsafe`, such as `unsafeCast` or
  `unsafeBaseIO`), metaprogramming commands that can expand to
  them (`macro`, `macro_rules`, `syntax`, `elab`, `elab_rules`,
  `declare_syntax_cat`), simprocs (`simproc`, `dsimproc`), and the option
  `debug.skipKernelTC`. Inside `@[...]` and `attribute [...]` it rejects
  attributes that run code at build or import time, bind native or
  substitute code, or extend the elaborator: `init`, `builtin_*`, `extern`,
  `implemented_by`, `export`, `macro`, `*_elab`, `*_parser`, `tactic`,
  `delab` and similar.
- **Axiom gate** (`scripts/check-axioms.sh`) runs the layout check and the
  source scan, then checks every theorem listed in `Theorems.lean` in four
  layers:
  1. `Theorems.lean` must be a plain manifest: comments, `import` lines, then
     one `#print axioms NAME` per line, no duplicates, nothing else.
  2. `leanchecker` replays every declaration of the `Botho` library in the
     kernel, so a declaration admitted with kernel checking off fails.
  3. `lean --json Theorems.lean` must print only Lean messages and no errors.
     Exactly one result is accepted per manifest command, matched by source
     position and theorem name, so text printed elsewhere (by `#eval` or an
     imported initializer) cannot stand in for a result.
  4. `scripts/AxiomReport.lean` imports the compiled library without
     elaborating its syntax and computes each theorem's axioms itself. These
     must equal Lean's `#print axioms` results, which catches a model that
     redefines `#print axioms`. The independent result is checked against
     `axioms-allowlist.txt` and must not contain `sorryAx`. Like the sorry
     gate, it also fails if any library declaration reaches `sorryAx`.
- Both gates read their inputs (`axioms-allowlist.txt`, `Theorems.lean`,
  `scripts/AxiomReport.lean`) before the first `lake` call and use only those
  copies afterwards, and the axiom gate runs the independent walk before it
  compiles `Theorems.lean`. This keeps local runs honest against cruder
  tricks; it is not what the verdict rests on (see [Limits](#limits)).
- **Self-test** (`scripts/selftest-gates.sh`) prepares fresh packages from
  the current `Botho/Basic.lean`, injects a
  `sorry`, an `admit`, a `native_decide`, a bogus axiom, a `sorryAx` and an
  empty manifest, and asserts that the matching gate fails each time. It also
  checks that a cold complete real-library tree passes and that the tokens
  are ignored inside comments, strings, raw strings and char literals.
  Since #1724 it also covers
  `sorry` hidden after a `'"'` char literal or an `r#"a"b"#` raw string, a
  `_root_.sorryAx` term, and primed theorem names in the axiom gate. Since
  #1770 it covers each known bypass of the in-tree gates, and checks that the
  gate fails for the intended reason: a fake `#print axioms` line printed by
  `#eval` or an imported initializer, `#print axioms` redefined by
  `macro_rules`, a theorem admitted with `debug.skipKernelTC`, a `sorry`
  hidden by `#guard_msgs`, `sorryAx.{0}`, `s!"{sorry}"`, duplicate manifest
  lines, a `lakefile.lean`, an `@[init]` initializer that rewrites
  `axioms-allowlist.txt` during the build, one that erases its own `@[init]`
  line, each forbidden attribute and metaprogramming command, a simproc that
  overwrites a compiled `.olean`, a bare `import Lean`, each import-header
  form, symlinked or stray sources, and layout changes (`srcDir`, an extra
  `lean_lib`, a `lean_exe`, `roots`, `globs`). Since the #1783 review it
  also covers a `[[require]]` dependency whose `lakefile.lean` rewrites a
  model source (its code must never run), a manifest that lists a package,
  each disallowed lakefile key and option, and (#1810) the `decide +native`
  split, `#guard` and prelude `#eval` `.olean` swaps, with `--no-scan`
  controls that show each swap fools the later layers, plus each banned IO
  name and evaluator. Passing controls cover a second module with quoted
  multi-line imports, a commented lakefile, the verify job's benign lakefile
  edit, allow-listed metadata and `leanOptions` in both TOML spellings, a
  tree without `lake-manifest.json`, names that merely contain a banned
  name, and the verify job's `--no-build` call with `LAKE=false`.
- **Workflow self-test** (`scripts/selftest-workflow.sh`, #1804) takes the
  `run:` block of the verify job's verify step from
  `.github/workflows/formal.yml` and runs it with stub verifiers. It checks
  that the step fails, and runs no verifier, when the base commit has no
  `verify-artifacts.sh` (even if the pull request has one), and that
  otherwise it runs the base copy with the base allow-list and lakefile and
  no `--no-*` flag. It also checks that the base checkout includes
  `formal/statements.txt` (#1805) and `formal/Theorems.lean` (the base
  manifest, #1824), and that the verifier is never called with
  `--statements` or `--base-manifest`. It needs no Lean toolchain.
- **Verify self-test** (`scripts/selftest-verify.sh`) runs the verify job's
  checks against the fixtures in `scripts/fixtures/verify/`: the bypasses
  found in the reviews of PR #1783 (an `@[init]` that widens the allow-list,
  a simproc that swaps an `.olean`, a `srcDir` move with an initializer, and
  `decide +native` with an IO world-token cast), a pull request that edits
  its own allow-list, a module with an initializer, and an artifact that
  carries an `Init` module, a `srcDir` decoy that compiles a weakened copy
  of the model while the scanned `Botho/` stays unchanged, and (accepted) a
  benign lakefile edit. Each fixture is built the way the build job
  builds it, and the verifier must reject it with the clean allow-list, both
  with and without the source scan. The verifier reads a copy of the
  sources taken before the build, as the verify job reads its own checkout.
  Four `scan-only` fixtures leave an artifact that is an honest build of a
  different source, so only the base's scan or lakefile check can stop
  them: a `[[require]]` dependency whose `lakefile.lean` rewrites
  `Botho/Cheat.lean` before it is compiled, and (#1810) `decide +native`
  split across modules, `#guard`, and a prelude `#eval`, each overwriting
  compiled `.olean` files. For these the self-test shows both halves: the
  full verifier rejects them, and the checks without the scan accept the
  forged `one_eq_two`. Since #1805 the statement check rejects these four
  too, so the self-test shows three steps: the full verifier rejects them,
  the checks without the scan reject them for the swapped statement, and
  only with neither the scan nor the statement check does the forged
  `one_eq_two` pass. `simproc-swap` and `native-world-swap` must also be
  reported by the statement check. Two `statement-only` fixtures compile a
  different source than the one the verifier reads, as a detached
  build-time process could, and leave no other trace: `statement-swap`
  (`satAdd_le_cap` states `True`) and `definition-swap` (the body of
  `satAdd` changes, no theorem's type does). Every other check, the scan
  included, accepts them; the statement check rejects them.
  `statement-missing` (an honest new theorem with no record block) fails
  closed, `record-pending` (a second block for a theorem) passes, and the
  clean tree fails closed with no record and with no base manifest. Since
  #1824: `implicit-type-swap` (statement-only) states a theorem over
  `Fin 2` instead of the recorded `Nat`, a difference only an implicit type
  argument carries, and the statement check must report that the readable
  text differs too; `manifest-drop` (statement-only) deletes a bound
  theorem's `#print axioms` line and fails, `manifest-drop-retired` (the
  same with a `retired` line in the base record) passes, and
  `retired-swap` (statement-only) shows that a `retired` line does not
  unbind a theorem that is still listed. The self-test also checks that no
  fixture initializer runs inside the verifier, and, as a control, that the
  same initializer does run when the library is imported through the Lean
  frontend. The clean tree is also verified with no `lean`, `lake` or `elan`
  on `PATH`, as in the verify job.
  The default matrix discovers every fixture directory and uses fresh
  packages from current Basic, its actual manifest entries and the actual
  statement record; it shares no compiled output. Two additional cold
  complete-library cases (`real-clean` and `real-definition-swap`) retain
  integration coverage without multiplying full-library builds and walks
  by the fixture count. Preparation controls check independent copies,
  current-source/configuration binding and explicit failure on unsupported
  dependency/layout changes. Source-byte fingerprints still traverse the
  whole model to detect fixture contamination. See the
  [fixture contract](scripts/fixtures/verify/README.md) for the scope and
  isolation checks. The production build/gates and trusted verify job
  continue to cover the complete model.

### The verify job

`scripts/verify-artifacts.sh` is the authority for the verdict. The verify job
checks out `formal/scripts/` (this script, `AxiomReport.lean`,
`check-sorry.sh`, `statements.py`), `axioms-allowlist.txt`, `statements.txt`,
`Theorems.lean` and `lakefile.toml` from the base commit of the pull request (on a push,
the pushed commit), never from the pull request head. A pull request that
edits these files changes nothing the verify job reads until it is merged.
It then:

1. checks that the pull request's `lakefile.toml` compiles the same sources
   as the base's. Both are parsed with Python's `tomllib`, and the
   package-level `srcDir` and, for every `[[lean_lib]]` and `[[lean_exe]]`,
   `name`, `srcDir`, `roots`, `globs` and `root` must be identical (so the
   set of libraries and executables is too). Which other keys may change is
   decided in step 2 by the base's key allow-list (no `[[require]]`, no
   code-loading keys; see Build configuration above). A `lakefile.lean` on either
   side, or a `lakefile.toml` that does not parse, is rejected. Without this
   a pull request could leave the scanned `Botho/` as a decoy and compile a
   different copy (`srcDir = "src"`) whose theorems state something else;
2. runs the base commit's source scan (`check-sorry.sh --no-build`: the
   build-layout check and `gate_scan.py` described above) over the pull
   request's sources, read as data;
3. takes in the artifact: only regular files named `Botho.olean` or
   `Botho/**/*.olean` (and the `.olean.server`, `.olean.private` and `.ir`
   parts), copied to a private directory that is the only entry on
   `LEAN_PATH`, so the artifact cannot shadow a toolchain module;
4. replays every `Botho` declaration in the kernel with `leanchecker`;
5. walks the compiled library with `AxiomReport.lean`, and fails if a
   theorem listed in `Theorems.lean` (parsed as text, never compiled) is
   missing or depends on an axiom outside the base allow-list or on
   `sorryAx`; if any library declaration reaches `sorryAx`; if a library
   module declares an axiom outside the base allow-list, imports anything
   but `Init` and `Botho` modules, or declares an `[init]` initializer; or if
   the compiled modules differ from the source modules;
6. binds each theorem listed in `Theorems.lean` to the base commit's
   statement record (#1805; see [Statement binding](#statement-binding)):
   the block computed from the compiled library must equal a block of the
   base `statements.txt`. A missing or malformed record, or a theorem with
   no identical block, fails. So does (#1824) a theorem that the base
   `Theorems.lean` lists and the base record binds but the pull request's
   `Theorems.lean` no longer lists, unless the base record retires it (see
   [Statement binding](#statement-binding)); a missing base manifest fails
   closed. Steps 5 and 6 both report before the job fails.

It never runs `lake`, never compiles a model source, installs the toolchain
fresh (an official `leanprover/lean4` release only) and uses no cache.

## Statement binding

The verify job checks the `.olean` files the untrusted build job uploads, and
cannot rebuild them (see [Limits](#limits)). So each theorem the project
claims is bound to a reviewed record instead: `statements.txt`, read from
the base commit (#1805).

A block of the record looks like this:

```text
theorem Botho.satAdd_le_cap sha256:ef778d42...
  : ∀ (cap a b : Nat), @LE.le.{0} Nat instLENat (Botho.satAdd cap a b) cap
  def Botho.satAdd : Nat → Nat → Nat → Nat
    := fun (cap a b : Nat) => @Min.min.{0} Nat instMinNat (@HAdd.hAdd.{0, 0, 0} Nat Nat Nat (@instHAdd.{0} Nat instAddNat) a b) cap
```

- **What the hash covers.** `AxiomReport.lean` computes the theorem's
  *statement closure*: its type, and every declaration of the `Botho`
  library that the type reaches, followed through what each one means (a
  definition's type and value, an inductive's whole block, a theorem's type
  only, since proofs are irrelevant and `leanchecker` has checked them). It
  serializes the closure exactly (de Bruijn indices, structural names,
  universe levels, literals and every field of each declaration kind; only
  binder names and metadata are left out) and `statements.py` takes its
  SHA-256. Declarations from `Init` are referenced by name: they come from
  the toolchain the verify job installs, never from the artifact. So the
  hash changes if the artifact states a different type *or* changes the
  body of a definition the statement uses (`satAdd`), even when the type
  reads the same.
- **What the reviewer reads.** The indented lines are the same closure,
  pretty-printed by Lean's built-in delaborator without any extension (so
  without notation: `@LE.le.{0} Nat instLENat x y` rather than `x ≤ y`).
  Since #1824 the printer shows every argument, implicit type arguments
  and instances included (`pp.explicit`), universe levels
  (`pp.universes`), numeral types (`pp.numericTypes`; under `pp.explicit` a
  numeral prints as `@OfNat.ofNat.{0} Nat (nat_lit 0) (instOfNatNat
  (nat_lit 0))`), the binder types of `fun` and `let`, and names in full,
  in prefix form (`AxiomReport.lean`, `ppOptions`). So a statement over
  `Nat` and the same statement over `Fin 2` read differently; before #1824
  both read `Eq (instHAdd.hAdd 0 0) 0`. Only proofs still print as `⋯`. The
  verify job compares the whole block as text, so the readable part cannot
  drift from the hash, and when a block does not match it says whether the
  readable text differs from the recorded one or only the hash does.
- **Proposals.** `scripts/propose-statements.sh` builds the tree and prints
  the record it compiles to (`--check` compares with `statements.txt`,
  `--write` overwrites it). The build job runs `--check` for information and
  prints the proposal on a mismatch; the verify job prints the blocks it
  computed for any theorem that fails. Both come from an untrusted build: a
  reviewer checks the text against the sources, and recomputes the hash on
  a tree they have reviewed for build-time code before accepting a block.
- **Changing or adding a statement takes two pull requests**, like adding
  an allow-listed axiom. The verify job reads the base record, so the block
  must be on `main` before the model that needs it:
  1. a pull request that adds the new block to `statements.txt` next to the
     old one (the model is unchanged, so its verify job passes against the
     old block; a theorem may have several blocks);
  2. the pull request that changes the model, which passes against the new
     block and may delete the old one.

  A toolchain bump that changes how a definition elaborates changes the
  hash too, and takes the same two steps. So does a change to the printer
  options in `AxiomReport.lean` (it changes every block's text, not its
  hash) unless, as in #1824, the same pull request regenerates
  `statements.txt`: its verify job runs the base printer against the base
  record, and the next one runs the new printer against the new record.
- **Removing a theorem takes two pull requests too** (#1824). The verify
  job reads the base `Theorems.lean` as well, and fails if a theorem it
  lists, and the base record binds, is missing from the pull request's
  `Theorems.lean`, unless the base record holds a line `retired NAME`:
  1. a pull request that adds `retired Botho.old_theorem` to
     `statements.txt` (the theorem stays listed and must still match its
     block: retiring does not unbind it);
  2. the pull request that deletes the `#print axioms` line (and the
     theorem, if wanted), and may delete the block and the `retired` line.

  A block for a theorem the base manifest does not list is a pending
  addition, not a removal, and is not checked. `propose-statements.sh
  --check` lists `retired` lines; `--write` drops them with the pending and
  stale blocks.
- **Bootstrapping (#1805).** The pull request that introduced the record
  could not be checked against it: its verify job ran the base scripts,
  which had no statement check. The first push to `main` after it merged
  is the first verify run that reads `statements.txt`.

## Limits

**What the trust boundary guarantees.** No model code runs in the verify job,
so model code cannot change the verify job's inputs (the gate scripts, the
allow-list, the report program) or its output. This closes the bypasses
found in the reviews of #1783: each one ran model code during `lake build` to
rewrite something the gate read in the same job.

**Why no model code runs there.** `leanchecker` and `AxiomReport.lean` load the
`.olean` files with `importModules (loadExts := false)`. Lean runs `[init]`
and `initialize` declarations only from `finalizePersistentExtensions`, which
only `loadExts := true` calls (`finalizeImport` in `Lean/Environment.lean`,
v4.35.0). The Lean frontend does load extensions: elaborating any file that
imports the model (`import Botho`, `lean Theorems.lean`) runs the model's
initializers. So the verify job never passes a model file, or a file that
imports the model, to the frontend, and it does not compile `Theorems.lean`
(the build job's axiom gate does; the verify job parses it as text).
`AxiomReport.lean` itself is elaborated, but it imports only `Lean` and loads
the model at run time. `selftest-verify.sh` checks this with a canary
initializer and the frontend control described above.

**What it does not guarantee: the artifact matches the sources.** The build
job is untrusted, so the `.olean` files it uploads are whatever model code
left on disk. The verify job proves that the uploaded library is
kernel-valid, that the listed theorems, as compiled, depend only on
allowed axioms, and (#1805) that they state what the base record says. It
does not prove that the library is what the sources compile to.

**What statement binding closes.** Before #1805 a build-time attack that
replaced `one_eq_two : 1 = 2` with `one_eq_two : True` in the compiled
library, and rewrote the modules that betrayed it, left a library that
passed every artifact check (the `native-split-swap`, `guard-swap`,
`eval-swap` and `require-dep` fixtures). The verify job now requires each
listed theorem's statement closure to match a block of the base record, so
substituting what a listed theorem states fails however it was done: by
code run during the build, by a detached process, or by build
configuration (`srcDir`, `lean_lib` roots or globs, a decoy copy). The
`statement-swap` and `definition-swap` fixtures leave no other trace and
are rejected by this check alone.

**What remains.** The binding is only as good as the record and covers
only what the record names:

- *The record is reviewed, not computed by a trusted build.* A block is
  proposed by a build of the pull request's sources, which runs model
  code. A reviewer who accepts a block must check its text against the
  sources and, before trusting its hash, rebuild a tree they have reviewed
  for build-time code. A malicious block that a reviewer accepts is bound
  as faithfully as an honest one.
- *The readable text is close to, but not the whole, binding.* Since #1824
  the text shows implicit arguments, instances, universe levels and
  numeral types, so a swap of an implicit type argument (`Nat` for
  `Fin 2`) is visible to a reviewer. It still hides proofs (`⋯`; proof
  terms cannot change what a statement means, by proof irrelevance, but
  the text does not show them), and it shows binder names, which the hash
  ignores. The hash covers what the text hides; a reviewer who accepts a
  block still recomputes its hash on a reviewed tree.
- *Only listed theorems are bound.* A declaration that no listed theorem's
  statement reaches (a helper lemma, an unused definition) is not in any
  block. It cannot change what a listed theorem states, but it is not
  checked against the sources either. Since #1824 a pull request cannot
  stop binding a theorem by deleting its `#print axioms` line: the verify
  job fails unless the base record already retires it, so unbinding takes
  a reviewed record change first. That rule binds the base manifest's
  theorems only: a theorem never listed on `main` was never bound.
- *`Init` is referenced by name.* The verify job loads `Init` from the
  toolchain the pull request picks (an official release only); the record
  binds names such as `instLENat`, not their definitions. A toolchain
  change is under CODEOWNERS (see below).
- *More of Lean runs on the artifact.* To print a block, the verify job
  runs Lean's pretty-printer (the built-in delaborator, with type
  inference and reduction) over declarations read from the artifact. No
  model code runs, but it is more of Lean's own code exposed to a
  hand-crafted `.olean` than the kernel replay and the walk alone. A term
  built to make printing or hashing slow fails the job by timeout, closed.
- *Pending blocks.* A theorem may have several blocks. An old block left in
  the record after a statement changed still verifies; remove it in the
  second pull request (`propose-statements.sh --check` lists blocks that
  match no compiled theorem). Likewise a `retired` line left on `main`
  keeps allowing its theorem's removal; remove it with the theorem.
- *Code-owner review is advisory today.* The record is only as good as the
  review of the pull requests that change it, and `main` does not yet
  require code-owner review (see the branch-protection item below); the
  two-PR rules make every unbinding a separate, reviewable record change,
  but do not enforce who reviews it.

The base commit's source scan and review still stand in front of build-time
code: they keep it from running at all, and the statement check makes it
pointless for the listed theorems.

Statement substitution through build configuration alone, with no code run
during the build, is closed by step 1: Lake compiles only the sources the
base's layout names, and those are the sources the scan reads and the
intake's module-set check compares (the `lakefile-decoy` fixture; every
other check but the statement check passes it). That check covers the layout keys of Lake v4.35.0's
TOML configuration. Build configuration that loads or runs code is closed
by the base's key allow-list in step 2. A `[[require]]` dependency's
`lakefile.lean` is a Lean program that Lake elaborates, `#eval` included, on
every `lake` call, and its source is never scanned, so it could rewrite a
model source before Lake compiles it and leave an artifact that is an
honest build of the rewritten source (the `require-dep` fixture; only the
statement check, of the checks on the artifact, can tell). The allow-list rejects `[[require]]`, a manifest
that lists a package, `moreLeanArgs`, `weakLeanArgs`, `plugins`, `dynlibs`,
`precompileModules`, `extraDepTargets`, any unknown key and any
`leanOptions` entry outside a short list. Since #1805 the statement check
also rejects any of these routes that changes what a listed theorem states
(the `lakefile-decoy` fixture with its own lakefile as the base's fails
there).

Binding the whole artifact to the sources, beyond the listed statements,
needs a build that runs no model code,
which Lean does not offer (macros, `decide +native` and initializers all run
user code during elaboration), or a rebuild under a sandbox that confines
each module's elaboration to its own output.

**The in-tree gates.** In the build job, model code runs during
`lake build`, and from then on nothing that reads `.lake/` can be trusted:
build-time code can replace a module's `.olean` with a forged one, and then
`leanchecker`, the walk and `#print axioms` in that job all check the forged
output. The in-tree gates' snapshots, kernel replay and walk only defend
against cruder tricks. Their source scan is what carries over to the verify
job: there it runs from the base commit and decides, before any of the
artifact checks, whether the sources may run code at build time at all.
Without `import Lean` a model cannot define a simproc, elaborator, tactic or
`deriving` handler, so what is left is the prelude. The scan covers it in
two layers (#1810). First, it bans the evaluators that run compiled code
while a module is elaborated: `#eval`, `#guard` (which evaluates its term
with the compiler and needs no `native` token), `native` in every spelling
(`native_decide`, `decide +native`, `native := true`), `Lean.reduceBool`
and its relatives, and initializers. Second, it bans the prelude IO surface
(`IO`, `EIO`, `BaseIO`, `ST`, `EST`, `EStateM`, `RealWorld`, `Void`,
`System`, `FilePath`, `unsafe*`), so an evaluator missing from the first
list has no named effect left to run: the world-token trick (`cast` through
`axiom : Void Unit = Void IO.RealWorld`) and every file or process call are
spelled with these names. Both layers are denylists, and a denylist can
miss routes. A pure model needs none of these names, so the lists can stay
broad. Hence the review rule below.

**Other limits.**

- A human must review every change to the model (`Botho.lean`, `Botho/`,
  `Theorems.lean`, `lakefile.toml`, `lean-toolchain`, `lake-manifest.json`,
  `axioms-allowlist.txt`, `statements.txt`) for IO and metaprogramming, with
  the same care as a change to `formal/scripts/`. A green `formal-gate` does
  not replace that review.

- The `.olean` format is a memory image that Lean maps without validating
  it. A hand-crafted `.olean` that corrupts the memory of `leanchecker` or of
  `AxiomReport.lean` is out of scope. The verify job holds no secrets and a
  read-only token.
- The workflow file comes from the pull request, as it does for every
  `pull_request` workflow. A pull request can edit `formal.yml` itself,
  including the verify job. Such an edit, and any edit under
  `formal/scripts/`, is visible in review and needs a human reviewer.
  `.github/CODEOWNERS` (#1804) names a code owner for `formal/scripts/`,
  `formal/axioms-allowlist.txt`, `formal/statements.txt`,
  `formal/lakefile.toml`,
  `formal/lean-toolchain`, `.github/workflows/formal.yml` and
  `.github/CODEOWNERS` itself.
- **Two branch-protection settings are not in place.** They need a
  repository admin, and no pull request can set them. Until an admin turns
  them on, CODEOWNERS only requests a review and a red `formal-gate` does not
  block a merge:
  - *Require review from Code Owners* on `main`
    (`require_code_owner_reviews`, today `false`);
  - *Require status checks to pass*, with `formal-gate` as a required check
    (`main` has no required checks today).

  A ruleset that pins `formal.yml` to `main` ("Require workflows to pass
  before merging") would take the workflow out of the pull request's hands.
  That rule needs GitHub Enterprise Cloud; the `botho-project` organization
  is on the Free plan, so it is not available.
- The verify job runs only the base commit's `verify-artifacts.sh`. If the
  base commit has none, the job fails; it never falls back to the pull
  request's copy (#1804). `scripts/selftest-workflow.sh` runs the job's
  script against stub verifiers to check this.
- The pull request picks the toolchain version, so a toolchain bump is
  verified with the bumped toolchain. Only official `leanprover/lean4`
  releases are accepted.
- That includes old ones: a pull request can pin an older official release
  in `lean-toolchain`, and the verify job checks the artifact with that
  older `leanchecker`. The checks do not reject a downgrade; `lean-toolchain`
  is under CODEOWNERS, so a code owner reviews it.
- `leanchecker` trusts the toolchain's own library (`Init`). The artifact
  cannot replace it (step 3 above), and it is loaded from `--sysroot` through
  `LEAN_SYSROOT`, never from a `lean` found on `PATH`.
- The artifact download relies on `actions/download-artifact` (v4.1.3 and
  later) to refuse archive entries outside the target directory (zip-slip).
  Intake then accepts only regular files with library names.

## Adding a theorem

1. Prove it in a module under `Botho/` that `Botho.lean` imports. Do not
   declare anything in `Theorems.lean`; the axiom gate rejects it.
2. Add one line to `Theorems.lean`:

   ```lean
   #print axioms Botho.my_theorem
   ```

   Group lines under a comment naming the module and its label (proven /
   tested / trusted).
3. Run `scripts/check-axioms.sh`. If it reports a non-allow-listed axiom,
   either remove the dependency or follow the next section.
4. Run `scripts/propose-statements.sh` and land the new block in
   `statements.txt` in its own pull request first (see
   [Statement binding](#statement-binding)); the verify job fails a theorem
   with no block in the base record. Changing an existing theorem's
   statement, or a definition its statement uses, takes the same two
   steps.

## Adding an allow-listed opaque axiom

Some primitives (hashes, RandomX) cannot be modeled usefully. They enter as
opaque `axiom` declarations, for example
`axiom Botho.sha256 : ByteArray → ByteArray`, and theorems may depend on them.
To add one:

1. Declare the axiom in the model module with a docstring that says what it
   stands for and which Rust function it abstracts.
2. Add its fully-qualified name to `axioms-allowlist.txt` with a one-line
   justification comment on the line above it:

   ```text
   # SHA-256 as used by botho/src/block.rs; modeled as an opaque function.
   Botho.sha256
   ```

3. Label every theorem that depends on it **trusted** and name the axiom.

The verify job reads the allow-list from the base commit, so a new axiom
fails the verify job until its allow-list entry is on `main`. Land the
allow-list line (with the axiom's justification) in its own pull request
first, then the model that declares and uses it.

Only opaque functions belong on the allow-list. Never add an axiom that states
a property (for example "sha256 is injective") without a design discussion on
the relevant issue. Such an axiom can make the model inconsistent.
