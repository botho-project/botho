# Lottery theorems (#1708)

This file says in plain words what each theorem about the LotteryV2 model
guarantees and what it does not. The model is `Botho/Lottery/` (#1707). Its
definitions are **tested** against the Rust code by the DRT legs in
`formal-drt/`. The theorems are **proven (model)**: they hold for the Lean
model, for every hash function (`Prims` is a parameter, and no axiom is
used), and say something about the Rust code only through that DRT. See
`README.md` for the labels.

#1708 is split into four parts:

| Part | Content | Status |
|---|---|---|
| A | Pool and fee conservation, and Rust widths | **proven (model), `Botho/Lottery/Conservation.lean`** |
| B | Seed independence and canonical-order determinism | #1850 |
| C1 | Sybil split without probability: the counterexample, the inert cap, profit as a function of split count, the R3 premise | **this file, `Botho/Lottery/Sybil.lean`** |
| C2 | Sybil split with ideal-roll expectation: expected winnings of `k` tickets = `k · payout / ρ`, the modulo-bias bridge, the production-shaped counterexample | #1851 |

## Conservation (part A)

`Conservation.lean` proves seven claims about the existing lottery model.
It reuses the fee arithmetic in `Botho/Supply/Accounting.lean`; it does not
assume the whole-ledger premises of that module.

`splitFees_conserves` proves pool plus burn equals the fee total whenever
`poolFractionPermille ≤ 1000`. Its canonical instance uses 800 permille.
Consensus fixes that value in `canonical_config` and does not read an
environment override, so the DRT generator's `> 1000` configurations are
outside this theorem's premise and outside consensus. For checked u64 fee
totals the pool cast is lossless (`Botho.Supply.splitFees_exact`); rounding
residue goes to burn (`Botho.Supply.splitFees_rounding`). The conservation
identity itself holds even without the width premise.

These are **proven (model)** claims. The corresponding extraction claims
are tracked separately in [#1838](https://github.com/botho-project/botho/issues/1838)
as `BothoAeneas.PoolAccounting.split_fees_permille_spec` and
`compute_pool_accounting_permille_conserves`. A theorem about this model is
not a proof of that extraction boundary or the Rust implementation.

`totalPaid_drawEligible` proves that any successful, nonempty draw pays its
entire pool. Each selection step contributes at most one share, so even an
underfilled draw whose remaining weight becomes zero cannot overpay before
`addToLast` assigns the remainder. The theorem requires neither an exact
winner count nor a positive-weight premise. `drawing_paid` connects this to
the complete successful drawing: the sum of actual winner payouts equals
`res.poolAmount`, which is at most `acc.payout` and `acc.available`.
The zero-payout and no-candidate branches pay zero. The drawing guard rules
out positive-payout, nonempty-candidate draws returning no winners.

`drawing_conserves` needs only a successful drawing and its checked fee sum
`F`. With the exact saturation loss

```text
loss = (r.lotteryPool + b.emissionShare + acc.feePool) - acc.available
```

it proves

```text
totalPaid res.winners + res.burnAmount
  + acc.carryoverAfter res.poolAmount + loss
  = r.lotteryPool + b.emissionShare + F
```

There is no ledger-reachability or no-saturation premise in this identity.
`drawing_conserves_no_loss` separately proves that loss is zero, and removes
it from the identity, when raw pool inflow fits u128. The whole-block supply
proof can discharge that premise from its reachable-state bounds; this
module does not establish those bounds.

`drawing_widths` retains the Rust-domain premises: reward, emission and
each fee fit u64, and the stored pool fits u128. For a successful drawing
and checked fee total it proves `F * 800 < 2^96`, available pool at most
`U128_MAX`, payout at most `U64_MAX`, and both each winner's payout and their
sum at most `U64_MAX`. Checked summation and the reward cap provide the
output bounds, so some input bounds are stronger than the proof needs.

The Rust ties remain the existing `split_fees`, `draw_winners_with_seed`,
`compute_pool_accounting` and `drawing` differential-test legs. They provide
finite test evidence, not a proof of all Rust executions. These proofs
change no executable model definition, consensus code, candidate ordering,
or cryptographic assumption; the seed and LMDB boundaries below still apply.

## Seed and ordering proof boundaries (part B)

The executable model is in `Botho/Lottery/`; its Rust correspondence and
existing differential tests are described in [README.md](README.md#the-lottery-model).
The theorems below are **proven (model)**, exported in `Theorems.lean` and
checked against the statement records. They quantify over the hash functions
in `Prims`; no cryptographic property is assumed and no hash axiom is added.

### Seed-independence

`SeedIndependence.lean` proves:

- `seed_body_independent`: changing the drawn block's fees, reward or emission
  share preserves the entire seed result when height and parent link agree.
- `seed_reads_tip_only`: rows and stored pool do not influence the seed.
- `seed_factors`: a successful seed is exactly
  `lotterySeedV2 P (P.powHash r.tip) r.tip.prevBlockHash b.height`.
  `seed_eq_of_pow_fields` compares successful seeds with equal PoW output,
  parent previous hash and height. It does not remove parent-link validation:
  `headerHash` still reads all parent header fields before a seed succeeds.
- `candidates_of_postState`: every successful drawing uses exactly
  `lotteryCandidates P r.rows b.height s canonicalConfig.draw`, with the
  successful seed `s`, the checked fee sum and its resulting accounting.
  The input rows and tip are the pinned state after block N−1, used at block
  N's height. This is the [#1689](https://github.com/botho-project/botho/issues/1689)
  boundary; a change to that state timing must update this theorem and the
  production correspondence.
- `winners_ids_independent_of_amounts`: successful drawings with the same
  tip, rows, height and parent link select the same ordered winner IDs when
  both payouts are nonzero. Fees, reward, emission share and stored pool may
  differ. Payout values may differ; a zero payout may suppress the draw, and
  changing fees may instead cause a fee-overflow error. The hypotheses retain
  these restrictions. The supporting `drawLotteryWinnersSeeded_ids` theorem
  applies to any fixed draw configuration. `pickWinners` uses payout only to
  populate winner amounts; the canonical winner count is `min(4, eligible.length)`.

**Model-abstraction caveat.** `DrawBlock` omits block N's timestamp, nonce,
transaction root and minter keys. Their inability to reach the seed is true
by construction of this model. The evidence that Rust reads no more than
modeled is the existing `drawing` DRT leg, which calls Rust's drawing through
an in-memory `DerivationReads` double. It is finite test evidence, not a proof
about all Rust executions. Source inspection at `bef3be4b` confirms both
production `DerivationReads::lottery_candidates` implementations pass only
`block.height()` from the current block to the common reader:
[`store/v2.rs:633`](../botho/src/ledger/store/v2.rs#L633) (call at line 658) and
[`experimental/validated.rs:241`](../botho/src/ledger/experimental/validated.rs#L241)
(call at line 247). The strict readers also check stored row context; those
read failures are outside the decoding-success model.

### Determinism

`Determinism.lean` adds `sortById`, a stable merge sort by the existing
`bytesLt` comparator. This is a specification of LMDB's default key ordering
(`memcmp`, with shorter keys first), **trusted** as a correspondence to LMDB;
it is not an additional Rust sort or a change to the existing drawing model.
The existing `lottery_candidates` DRT leg runs the production reader over a
real LMDB ledger and supplies finite evidence for that correspondence.

The byte comparator is proven irreflexive, transitive and trichotomous on
all byte strings, including equal-length 36-byte IDs. `sortById_sorted`
proves strict ordering when IDs are unique; `sortById_of_sorted` proves the
existing strictly ordered model inputs are unchanged. `sortById_perm`
uses core Lean's sorted-permutation uniqueness theorem, with ID uniqueness
to identify entire rows. `drawing_perm_invariant` then proves the complete
drawing result, including errors, is invariant under any permutation of
uniquely keyed rows after canonical sorting. Equal IDs carrying different
row data are excluded, matching LMDB's unique keys.

Canonical ordering affects outcomes. `draw_order_dependent` uses two
eligible, distinct 36-byte IDs and constant-zero hashing, making every roll
zero. A pool of 3 pays `[a:1, b:2]` in one order and `[b:1, a:2]` in the other:
selection visits the rows in order, and `addToLast` assigns the remainder to
the final winner. This is an explicit mathematical witness, not a model of
production SHA-256. `scan_cap_order_dependent` gives a second witness at the
actual 10,000-candidate cap: with 9,999 already collected, reversing the
next two eligible rows changes which one gets the last slot.

These results establish determinism conditional on canonical rows and a
fixed `Prims`. They establish neither hash unpredictability nor a theorem
about LMDB, Rust, corruption handling or the timing issue in #1689. The
executable drawing and DRT dispatch are unchanged by these proofs.

## The Sybil split (part C1)

### What the design claimed, and what is false

`sybil_reward_cap`'s doc comment and research §9.1
(`docs/research/ct-compatible-lottery-selection.md`) said: a splitter that
creates `k` of the `ρ` eligible outputs wins expected `k/ρ · R = k ·
base_fee`, "exactly the `k · base_fee` it pays", so splitting is net-zero.

Two separate claims are in that sentence:

- **"Winnings per draw rise by at most `k · base_fee`."** This holds, apart
  from a negligible modulo bias, because `payout ≤ ρ · base_fee`. Part C2
  (#1851) proves it for the model's draw.
- **"So splitting is net-zero."** This is false. No consensus rule charges
  `LOTTERY_BASE_FEE_PICO`; it appears only in the cap. A ticket is paid for
  once, through the ordinary fee floor (870 to 117,032 pico, or 6.25e6 pico
  with the mempool-only `MIN_TX_FEE`, #1684), and then stays eligible for
  9,281 draws.

### Theorems

**Counterexample.** `ce_owner_wins_three_base_fees` and `sybil_net_zero_refuted`.

The pre-state has four eligible rows at the dust floor, created at height 0.
The owner holds three of them. The block is epoch-1 shaped: block reward
25 BTH, lottery emission share 2.5 BTH, an empty stored pool and no fees.
For every hash function, and at every height from 720 to 10,000 (the 9,281
draws the rows are eligible in):

- `drawing` succeeds;
- all four rows win, because there are no more of them than the four winner
  slots and the rolls only choose the order (`drawEligible_uniform_all_win`);
- the owner receives exactly `3 · LOTTERY_BASE_FEE_PICO` = 7.5e11 pico.

So the per-draw equality of the design holds, and if a ticket cost
`c < 2.5e11` once, the owner is ahead after a single draw. Parameters:
ρ = 4, `LOTTERY_BASE_FEE_PICO` = 2.5e11, cap `min(4 · 2.5e11, 25e12)` =
1e12, payout 1e12, and `MAX_LOTTERY_CANDIDATES` = 10,000, which is not
reached.

The per-ticket cost `c` is a hypothesis (`hfee`), not a modelled fee,
because the consensus fee floor is not in the model (#1684).

This theorem does not say:

- that the same four rows stay the only eligible ones across those heights.
  Each height is a separate pre-state;
- anything about production-sized sets. That is part C2's
  production-shaped counterexample (ρ = 10,000).

**The cap is inert.** `rewardCap_inert`, `lotteryCandidates_length`,
`countEligible_lotteryCandidates` and `rewardCap_saturated`.

- `rewardCap` is the block reward once `ρ · base_fee ≥ block_reward`, which
  takes ρ ≥ 100 at 25 BTH.
- The candidate read returns `min(10,000, eligible rows)` candidates, and ρ
  is their number.
- So with at least 10,000 eligible rows, the cap is the block reward for
  every reward up to 2,500 BTH, which covers every block reward of the
  schedule. The schedule itself is an input here (#1710).
- `sybil_reward_cap` never binds at production parameters.

**Profit as a function of split count.** `splitUnprofitable_iff`,
`split_never_pays_iff`, `split_never_pays_iff_fixed`, `break_even_reference`,
`break_even_backlog_reference` and `split_pays_at_reference`.

These are about a **stated economy model**, not the node: the closed form of
the #1761 sim. The organic economy creates ρ_o outputs per block and the
coinbase adds one free ticket. The splitter creates `k` tickets at `c` each.
Its stationary share is `k/(ρ_o + k + 1)`. The payout is
`R(k) = R0 + pm/1000 · k · c`, where `pm` is the share of the splitter's own
fees that returns to the pool.

- Net ≤ 0 at `k` iff `1000 · R0 ≤ c · (1000 · (ρ_o + 1) + (1000 − pm) · k)`.
- Net ≤ 0 for every `k` iff it holds at `k = 1`.
- With a fixed payout (`pm = 0`, the block-reward clamp binds), net ≤ 0 for
  all `k` iff `R ≤ c · (ρ_o + 2)`, that is `c* = R/(ρ_o + 2)`. This is the
  sim's `break_even_matches_closed_form`.
- Reference point (epoch 1, ρ_o = 20, no backlog, `R0` = 2.5008 BTH, 80% fee
  recycling): `c*` = 117,962,264,151 pico = **0.118 BTH**, which is
  `R0/(ρ_o + 1.2)`, not `R0/(ρ_o + 2)` = 0.1137 BTH. The 80% of the
  splitter's own fees that return to the pool account for the difference.
- Backlog drain (payout clamped at the 25 BTH reward): `c*` = 25/22 BTH
  ≈ 1.136 BTH.
- At the reference point with the `MIN_TX_FEE` cost (6.25e6 pico per ticket)
  and k = 1,584, the split pays.

The bridge from this model to the node is "expected share = ticket share"
for the uniform draw, which is part C2.

**The R3 premise (#1837 option 1).** `r3_no_split_profit`.

Suppose:

- (i) the holder paid at least `p` per ticket (`hfee`);
- (ii) every draw in its tickets' lives pays `payout · L ≤ N · p`, where `N`
  is the untruncated eligible count and `L` is the ticket life (`hprice`);
- each ticket is eligible in at most `L` draws (`hlife`);
- in each draw the holder's expected winnings are at most its ticket share
  of the payout (`hshare`).

Then its expected winnings over its tickets' lives are at most its outlay,
whatever the split. Part C2 discharges `hshare` for the model's draw when
`N ≤ MAX_LOTTERY_CANDIDATES`; until then it is an explicit hypothesis. R3
requires (i) for **every** eligible output, free ones included. Otherwise
the holder of free tickets (the coinbase and payout outputs, #1690) does not
meet `hfee`.

**Current code violates both premises.** `r3_eligibility_unpriced` and
`r3_pricing_violated`.

- (i) A row is eligible on its age and value alone. Nothing in the
  eligibility test or the candidate read prices a ticket, so `hfee` cannot
  be discharged from consensus rules today (#1684, #1690).
- (ii) Take a saturated set at the epoch-1 parameters. The payout is at
  least the 2.5 BTH emission share, and `payout · 9,281 > 10,000 · p` for
  every `p ≤ LOTTERY_BASE_FEE_PICO`. R3 pricing would need `p` ≥ 2.32 BTH
  per ticket. The cap has no division by the ticket life, and ρ is the
  truncated count.

### Not covered (simulation questions)

These are not covered by any theorem here, and stay with the sims (#1683,
#1761):

- variance and timing of a splitter's winnings;
- profitability above 10,000 eligible outputs, where window inclusion
  depends on the seeded offset and key gaps, and output placement can be
  ground against the hash;
- the joint dynamics of fee recycling, backlog and free tickets beyond the
  stationary formulas above.
