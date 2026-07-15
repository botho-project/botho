# CT-Economics Gadgets: Demurrage and Tag-Blend over Hidden Amounts

**Issue:** [#902](https://github.com/botho-project/botho/issues/902) (the CT-reconciliation epic; ADR 0006 decision 1). This memo covers the **two remaining** value-dependent economics gadgets. The third — the lottery — is already solved and ratified: [`ct-compatible-lottery-selection.md`](ct-compatible-lottery-selection.md) (Path C + endogenous reward-cap, no ZK sort).
**Status:** Design research. **No consensus code change.** The constructions here are candidate designs and crisp maintainer decisions for the CT normative spec; the normative authoring is parked on the `anvil:spec` loop (as #902 records), the same way the lottery design was.
**Scope:** ADR 0006 confidential amounts = Pedersen commitments `C = vH + rG` + Bulletproofs range proofs, **fees public**, universal ML-KEM outputs. Every output already carries a commitment `C_i` and a Bulletproof `v_i ∈ [0, 2^64)`; balance is the homomorphic identity `Σ C_in = Σ C_out + fee·H`. The transfer value used by demurrage is the output sum `V = Σ_out v_i`, whose commitment `C_V = Σ_out C_i` is publicly computable.
**Reads:** ADR 0006 (`docs/decisions/0006-*`), ADR 0007 (`0007-bridge-import-cluster-tagging.md`), `cluster-tax/src/demurrage.rs`, `cluster-tax/src/fee_curve.rs`, `botho/src/ledger/store.rs::consensus_fee_floor`, `botho/src/mempool.rs::effective_cluster_wealth_from_outputs`, `transaction/core/src/validation/validate.rs` (inheritance mass bound), `transaction/types/src/cluster_tags.rs`, whitepaper §9 (`tab:metadata-leak`).

---

## 0. TL;DR

- **Gadget 1 (demurrage over hidden value): SOLVED, cheaply, by a committed-demurrage term + a one-sided Bulletproof inequality** — no new crypto beyond the range proofs ADR 0006 already mandates. The demurrage charge is `d = k·V` for a scalar `k` that is a **public** function of factor, elapsed, rate, and horizon. The prover commits `C_d`, proves `q·d − p·V ≥ 0` (charge is *at least* the owed demurrage) and `fee − base − d ≥ 0` (the public fee covers it). ~1 aggregated Bulletproof over 2 scalars, ≈0.7 KB, a few ms to verify, batchable per block.
- **Gadget 2 (tag-blend over hidden value): LARGELY DISSOLVES via a value-free reframe.** Consensus **already does not** verify the exact value-weighted blend — it enforces a mass **upper bound** (`validate.rs`: "output tag masses must not exceed the decayed input tag mass"). That bound is homomorphic (weights are public, values are commitments) → a per-cluster committed-inequality (≤16 aggregated range proofs), *or* it relaxes to the value-free `weight_out(c) ≤ max_i weight_i(c)` with **no proof at all** (over-permissive only in the fee-*raising* direction, so economically safe). No blend-correctness ZK proof is needed.
- **Both gadgets bottom out on the SAME irreducible core:** the cluster-**factor curve's input is cluster wealth, a value aggregate** `W(c) = Σ v·weight(c)`. This is the one quantity that genuinely goes hidden under CT. Everything else is either already value-free (`elapsed` is an order statistic; ADR 0007 import wealth is public at the boundary) or reduces to a Bulletproof inequality.
- **The factor core has a clean, cheap resolution:** the production curve is **already log2/octave-structured** (`LOG2_WMID_FP`; 1× below `W_MID≫12`, 6× above `W_MID≪12`, ~12 octaves between). So evaluating it in ZK = **placing committed `W(c)` in a public octave bucket = 2 range proofs**, and the factor is a public step-function of the octave. Alternatively — and this is the **highest-value maintainer question** — a **value-free provenance factor** keyed on public mint/import origin sizes (generalizing ADR 0007) would dissolve even the octave proof.
- **Residual leakage is factor-conditional and prover-tunable.** A factor-1 (background/commerce) coin has `k = 0` → the fee is value-independent → **zero value leakage**. A wealthy-cluster coin at the minimum fee reveals its value nearly exactly (the intrinsic public-fee leak); the committed term lets a privacy-seeking prover **overpay to a coarse bucket**, capping the leak at the fee-quantization granularity. Whether to *mandate* quantization is a policy decision.

---

## 1. The problem, stated precisely

Botho's anti-hoarding layer reads value in the fee path. Under ADR 0006 the value is a Pedersen commitment; the fee is public. The three value-reading sites in `consensus_fee_floor` (`store.rs:2013`):

1. **`elapsed`** — the ring age used by demurrage. Already **value-free**: `ring_elapsed_quantile` is an *order statistic over public creation heights* (the audit-cycle-6 H2 fix, `demurrage.rs:415`), deliberately ignoring value. Nothing to do — CT-clean today.
2. **`factor`** — the 1×–6× multiplier. `demurrage_factor = max(ring_centroid_implied_factor, import_floor)`, and `claimed_factor = cluster_factor(cluster_wealth)`. Both trace to **cluster wealth**, a value-weighted aggregate. **This is the hard core.**
3. **`value` (V) as the linear multiplicand** in `demurrage = k·V` and in the balance identity. Hidden, but the multiplicand only ever appears *linearly*, which is exactly what commitments handle well.

A naive `fee = base + k·V` with public `fee, base, k` reveals `V = (fee − base)/k` exactly. That defeats CT for every wealthy coin. The gadgets below fix (3) cheaply and (2) with a bounded, structured leak, and show (via the reframe) that the *tag blend* never needed a ZK proof at all.

---

## 2. Gadget 1 — Demurrage over a hidden amount

### 2.1 The charge is linear in the hidden value with a public coefficient

From `demurrage.rs`, the accrued charge is

```
demurrage = V × rate_bps/10000 × (factor − 1000)/5000 × elapsed/blocks_per_year   =   k · V
```

where `rate_bps` (policy), `elapsed` (public order statistic), `blocks_per_year` (policy), and `factor` (see §4) are **all public**. So `k` is a public non-negative rational, write it `k = p/q` with public integers `p, q` (from the fixed-point constants: `q = 10000 · 5000 · blocks_per_year`, `p = rate_bps · (factor−1000) · elapsed`).

The #925 capitalized-reset term is *also* linear with a public coefficient:
`capitalized = demurrage_charge(V, input_floor, H) − demurrage_charge(V, output, H) = (k_floor − k_out)·V`, both `k`'s public (factors public, horizon `H` public). And `spend_demurrage_charge = max(accrued, capitalized) = max(k_acc, k_cap)·V` since `V ≥ 0` — still one public coefficient `k* = max(k_acc, k_cap)`. **So the whole demurrage obligation, including the #925 downgrade charge and #831's settlement charge, is a single `k*·V` with a publicly-computable `k*`.** One gadget covers all three.

### 2.2 The construction: a committed demurrage term + a one-sided inequality

The consensus requirement is only that the fee collects *at least* the owed demurrage: `fee ≥ base + k*·V`. It does **not** require reading `V`. So:

1. **Prover** publishes a committed demurrage term `C_d = d·H + s·G` with `d ≥ ⌈k*·V⌉` (any sufficient integer; see §3 on the freedom here).
2. **Sufficiency proof** `q·d − p·V ≥ 0`. Homomorphically, `q·C_d − p·C_V = (q·d − p·V)·H + (q·s − p·R)·G` where `C_V = Σ_out C_i`, `R = Σ_out r_i`. The prover proves this commitment opens to a **non-negative** value — a single Bulletproof range proof on `δ = q·d − p·V ∈ [0, 2^n)`. (One-sided: there is no upper bound on `d`, which is what lets a privacy-seeking prover round `d` up — §3.)
3. **Coverage proof** `fee − base − d ≥ 0`. `fee, base` public; form `C_Δ = (fee − base)·H − C_d`, prove it opens to `Δ ∈ [0, 2^64)` — a standard 64-bit Bulletproof range proof.

Both (2) and (3) are range assertions on linear combinations of existing commitments. Bulletproofs **aggregate** them: one proof over two committed scalars.

### 2.3 Cost

| item | size | verify |
|---|---|---|
| single 64-bit Bulletproof | ~672 B | ~1–2 ms (one multiexp) |
| **aggregated over 2 scalars (this gadget)** | **~736 B** | **~2–3 ms, batchable across the block** |

This is *per spending transaction*, and it **reuses the exact proof system ADR 0006 already requires** for output range proofs — no new trusted setup, no new assumption, same verifier code path (batch the demurrage proof with the output range proofs in one block-level multiexp). Factor-1 spends (`k* = 0`) skip the gadget entirely: the fee is just the public size/output base fee.

### 2.4 Interaction with the other value-reading charges

- **#925 capitalized reset & #831 settlement:** subsumed — both are `k·V` terms with public `k`, folded into `k* = max(...)` (§2.1). No separate gadget.
- **Base minimum fee:** size/outputs/memos are public; its only value-dependence is through `cluster_factor(cluster_wealth)` — i.e. the **factor core (§4)**, not `V` directly. So once §4 supplies a public (or bucket-proven) factor, the base fee is public too.
- **Ring-centroid factor floor:** value-dependent today (value-weighted centroid of ring members). Reframed to value-free in §4.

---

## 3. Residual leakage from the public fee (updating whitepaper §9)

The committed term does **not** eliminate the public-fee leak; it makes it **factor-conditional and prover-tunable**. Two regimes:

**Factor-1 coins (the common case — background/commerce): zero value leakage.** `k* = 0 ⇒ demurrage = 0`, and the base fee depends only on size/outputs/memos. The public fee reveals nothing about `V`. This is a *strict improvement* over the current §9 table row ("Fee amount: 2–4 bits leaked") for the majority of transactions.

**Wealthy-cluster coins (factor > 1): a prover-controlled upper bound.** If a rational actor pays the *minimum* fee `fee = base + ⌈k*·V⌉`, the observer inverts `V ≈ (fee − base)/k*` to a resolution of `1/k*` base units — i.e. **essentially the exact value** (e.g. factor-6, 2%/yr, 1yr held ⇒ `k* = 0.02` ⇒ `V` pinned to ±50 pico). The gadget's one-sided inequality is what buys the mitigation: because only `d ≥ k*·V` is proven (no upper bound on `d`), a privacy-seeking prover may set `d = bucket(k*·V)` rounded **up** to a coarse granularity `g` and still pass. The observer then learns only `V ≤ d·q/p`, an **upper bound at granularity `g`**, leaking `≈ log2(V_range / g)` bits instead of the full value.

**Proposed §9 `tab:metadata-leak` replacement row** (supersedes "Fee amount 2–4 / Min fee policy / 1"):

| Metadata | Bits leaked | Mitigation | Residual |
|---|---|---|---|
| Fee — factor-1 spend | 0 | k = 0 (value-independent base fee) | 0 |
| Fee — wealthy spend, min-fee | ≈ full V | committed demurrage term (`d ≥ k·V`) | upper bound on V |
| Fee — wealthy spend, bucketed | tunable | mandated fee quantization to granularity g | ≈ log2(V/g) bits |

**Crisp decision this surfaces (D1):** *does the protocol mandate demurrage-fee quantization (a hard privacy floor for wealthy coins) or leave overpayment optional (privacy-conscious users self-protect, min-fee users leak)?* Quantization costs a small revenue-precision loss (the extra `d − k·V` flows to the same lottery/redistribution pool, so it is not burned — it is a rounded-up contribution). Recommendation: **mandate a modest quantization** (e.g. round `d` up to a fixed number of significant bits) so privacy does not depend on user sophistication, mirroring the "no opt-in tiers" logic ADR 0006 §2 used for ML-KEM.

---

## 4. The irreducible core: the factor over hidden cluster wealth

Both gadgets, and the base fee, ultimately need `factor = cluster_factor(W(c))` where `W(c) = Σ_utxo v·weight(c)` is a **value aggregate**. Under CT the per-UTXO `v` are commitments, so `W(c)` is only available as a **homomorphic commitment** `Ĉ(c) = Σ weight·C_utxo` (weights public), unless the protocol accepts a per-spend value leak to maintain a cleartext total. This is the one genuinely hard thing left.

Three resolutions, in increasing order of how much they dissolve:

### 4.1 Option A — Bucketed curve evaluation over a committed wealth (recommended default)

The production curve (`fee_curve.rs`) is **already an octave/log2 step structure**: `factor ≈ f(log2(W) − log2(W_MID))`, pinned to exactly 1× for `W ≤ W_MID≫12` and 6× for `W ≥ W_MID≪12`, monotone across ~12 octaves (`LOG2_WMID_FP`, `W_MID = 100k BTH`). So the factor is (up to a fixed-point quantization already present) a **public step-function of the octave index** `j = ⌊log2 W(c)⌋`.

Evaluating it in ZK is therefore just **placing the committed `Ĉ(c)` in a public octave bucket**: prove `2^j ≤ W(c) < 2^{j+1}` (two range proofs, or one bit-length argument), yielding a **public** factor `f_j` that both parties agree on. Cost: 2 range proofs per distinct cluster read in a spend — bounded by `MAX_CLUSTER_TAGS = 16`, aggregatable, ≈1–2 KB total. Leakage: reveals only the **octave of an aggregate over an entire cluster origin** (potentially thousands of coins) — a coarse, non-individual statistic, categorically weaker than a per-UTXO amount. This is a natural fit because the curve was *designed* as a log2 sigmoid.

### 4.2 Option B — Value-free provenance factor (highest-value; dissolves even Option A)

ADR 0007 already set the precedent: the **bridge-import** factor is derived from the epoch's **public** unwrap wealth (amounts are revealed at the bridge boundary, ADR 0004) with a floor `F`, and decays *only by circulation* — no ZK gadget for imports (ADR 0007 §Consequences explicitly notes it "sidesteps, for imported coins, the #902 problem"). The **domestic** analog: key the factor on public provenance signals rather than the live value-weighted wealth —

- **mint origin size is public** (coinbase reward amounts are public);
- **import origin size is public** (ADR 0007);
- **decay hops / age are public** (tag `DecayState`, creation heights).

A coin's lineage mass is *bounded* by its public origin size decayed by its public hop count. If the factor is defined from these public quantities — the same way the age leg was redefined as a value-free order statistic and the lottery was redefined value-free (Path C) — then **cluster wealth never needs to be read, committed, or proven, and both gadgets fully dissolve.** The cost is precision: a value-free provenance factor is a coarser proxy for "how concentrated is this lineage" than the live sum. Whether that precision loss is acceptable is the key open question below.

### 4.3 Option C — Public cluster-wealth aggregate (rejected)

Maintain `W(c)` as cleartext. Requires each spend to reveal its `v·weight(c)` contribution, which for a single-cluster coin **reveals `v`**. Collapses CT for exactly the wealthy coins CT most needs to protect. Rejected; listed for completeness.

**Crisp decision this surfaces (D2):** *domestic factor under CT — Option A (octave-bucket proof over a committed cluster-wealth aggregate, exact-ish, ~1–2 KB/spend) or Option B (value-free provenance factor from public mint/import/hop signals, zero proof, coarser)?* This is the single highest-leverage decision in the whole epic: **Option B dissolves the last hard gadget the way Path C dissolved the lottery.** It should be tested with a calibration sim (does a provenance-only factor preserve the Δgini floor and Sybil-resistance the live-wealth factor delivers?) before ratification — the same empirical bar the lottery and bridge-import decisions cleared.

---

## 5. Gadget 2 — Cluster-tag blending over hidden amounts

### 5.1 The reframe: consensus never verified the exact blend

The tag-blend *appears* to need values: `weight_out(c) = Σ_in v_i·weight_i(c) / Σ v_i`. But this value-weighted average is the **wallet's default construction**, not a consensus invariant. What consensus actually enforces (`validate.rs:451–455`, `validation/error.rs:168` `MassInflation`) is a **one-sided mass bound**:

> "The sum of output tag masses for each cluster must **not exceed** the (decayed) input tag mass for that cluster."

Deflation (claiming *less* wealthy provenance than the blend) is **legal** and instead **priced** — by the ring-centroid factor floor and the #925 downgrade charge (`demurrage.rs` module docs are explicit: `check_cluster_tag_inheritance` "only rejects inflation"). So there is **no exact-blend equality to prove in zero knowledge.** Gadget 2, as posed ("prove the output tag vector is the correct value-weighted blend"), is a problem consensus does not have.

What remains is two much smaller obligations, and both go value-free:

### 5.2 The inflation bound — homomorphic, or value-free

The mass bound `Σ_out weight_out(c)·v_out ≤ decay · Σ_in weight_in(c)·v_in` is, under CT, an inequality between two homomorphic commitment sums (weights and decay public, values committed). Two ways to discharge it:

- **(a) Keep it exact — committed inequality.** Per cluster, form `Ĉ_gap(c) = decay·Σ_in weight_in(c)·C_in − Σ_out weight_out(c)·C_out` and prove it opens to `≥ 0` (a Bulletproof range proof). ≤16 clusters (`MAX_CLUSTER_TAGS`), aggregatable → ~1–2 KB, a few ms. Same machinery as Gadget 1.
- **(b) Relax to value-free — `weight_out(c) ≤ max_i weight_i(c)`.** The value-weighted average never exceeds the largest input weight, so `max_i weight_i(c)` is a conservative, **value-free** upper bound checkable from **public tags alone — no proof.** It is more permissive (lets an output claim up to the max input weight even where value-weighting would give less), but claiming *more* wealthy-cluster attribution only *raises* the spender's own factor/fee, so a rational fee-dodger never exploits the slack. The one direction that matters — *deflation* to dodge the factor — is not governed by this bound at all; it is caught by the ring floor (§5.3).

Recommendation: **(b) as the default** — it removes a per-cluster proof from every spend at zero security cost against rational fee-avoidance, and keeps the honest-blend as an unenforced wallet convention. Keep (a) available if a future threat model needs exact mass conservation (e.g. to bound a non-economic provenance-laundering attack).

### 5.3 The anti-deflation floor — the actually load-bearing piece, value-free via order statistic

The floor that stops a spender under-declaring output tags (`ring_centroid_implied_factor`, `demurrage.rs:482`) is today a **value-weighted centroid** of ring members' cluster wealth — value-dependent, and under CT the ring members' values are hidden too. The fix is the **exact move already made for the age leg**: replace the value-weighted mean with a **value-free order statistic over ring members' implied factors** (e.g. the max, or a high quantile). The real input is one ring member; its true factor is `≤` the max member factor, so flooring at (say) the max member factor over-charges only honest spenders whose real input is not the wealthiest decoy — the identical over-charge/leak tradeoff the **decoy-quantile sweep** (`decoy_quantile_sweep`) already characterizes for ages. This makes the floor value-free **except** that each member's factor still needs that member's cluster wealth — i.e. it reduces, once more, to the §4 core. No new hard problem; the same octave-bucket-proof or value-free-provenance resolution serves it.

### 5.4 Does bridge-import tagging suggest a fully value-free propagation rule?

Yes, and it is the strongest parallel in the codebase. ADR 0007 handles the bridge boundary with a rule that is **value-free-by-public-data**: epoch key `c_import(m)`, factor from the epoch's *public* unwrap wealth, decay **only by circulation**. It explicitly notes it "sidesteps the #902 problem" for imports. Generalizing its principle — *provenance and public origin size, not live hidden value, drive the factor* — is exactly Option B (§4.2). The bridge already lives without a value gadget; the open question is whether the domestic path can adopt the same stance.

---

## 6. What is decided vs. what needs the `anvil:spec` loop

**Design-decided here (no further ratification needed to start speccing):**
- The demurrage gadget (§2): committed term + one-sided Bulletproof inequality, folding accrued/#925/#831 into one `k*·V`. Reuses ADR-0006 Bulletproofs.
- Gadget 2 does **not** need a blend-correctness ZK proof (§5.1) — consensus enforces a bound, not the blend. Inflation bound → value-free max-weight (§5.2b); anti-deflation floor → value-free order statistic (§5.3).
- The `elapsed` leg is already value-free (nothing to do).

**Crisp decisions requiring maintainer ratification (blocking the spec):**
- **D1 — Demurrage-fee quantization (§3):** mandate coarse fee buckets for wealthy spends (hard privacy floor) vs optional overpayment (sophistication-dependent). Recommend mandate.
- **D2 — Domestic factor under CT (§4):** Option A (octave-bucket proof over committed cluster wealth) vs **Option B (value-free provenance factor, dissolves the last gadget)**. Recommend a calibration sim on Option B first — if it holds the Δgini/Sybil bars, the whole factor core dissolves.
- **D3 — Inflation bound (§5.2):** value-free max-weight (default) vs exact homomorphic committed inequality. Recommend max-weight.

**Needs the `anvil:spec` normative loop (once D1–D3 land):** the exact commitment layout, the `p/q` fixed-point derivation and rounding rule, the Bulletproof aggregation grouping (fold demurrage + inflation-bound + output range proofs into one block-level batch), the octave-bucket proof statement (if Option A), and a `code_ref` consistency audit against `demurrage.rs` / `fee_curve.rs` / `consensus_fee_floor`. Park exactly as #902 records and the lottery memo did.

---

## 7. Bottom line for #902

- **Lottery:** solved and ratified (Path C, value-free, no ZK sort).
- **Demurrage:** solved by a cheap, standard gadget (committed term + one Bulletproof inequality, ~0.7 KB/spend, reuses ADR-0006 crypto). Residual leak is factor-conditional (zero for background coins) and prover-tunable (bounded by fee quantization for wealthy coins). The only open piece is the **factor**, shared with:
- **Tag blend:** the ZK blend-correctness proof #902 asked for is **not needed** — consensus enforces a bound, not the exact blend, and both the inflation bound and the anti-deflation floor go value-free. This gadget **largely dissolves**, exactly the way Path C dissolved the lottery.
- **The one hard residue** is the factor curve's input, cluster wealth. It has a cheap exact resolution (octave-bucket proof over a committed aggregate, ~1–2 KB) *and* a candidate full dissolution (value-free provenance factor generalizing ADR 0007), gated on one calibration sim.

**#902 is close to buildable.** With D1–D3 ratified, none of the three sub-problems requires a Merkle-sum tree, a weighted-sampling ZK sort, or any proof system beyond the Bulletproofs ADR 0006 already commits to. The epic's hardest-stated gadgets (weighted lottery sort, blend-correctness proof) have both evaporated under value-free reframes; what remains is one linear-inequality gadget and one factor-input decision. The next concrete step is the Option-B factor calibration sim — if it clears the bar, the CT economics reconcile with **no ZK beyond range proofs at all**.
