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
| A | Pool and fee conservation, and Rust widths | #1849 |
| B | Seed independence and canonical-order determinism | #1850 |
| C1 | Sybil split without probability: the counterexample, the inert cap, profit as a function of split count, the R3 premise | **this file, `Botho/Lottery/Sybil.lean`** |
| C2 | Sybil split with ideal-roll expectation: expected winnings of `k` tickets = `k · payout / ρ`, the modulo-bias bridge, the production-shaped counterexample | #1851 |

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
