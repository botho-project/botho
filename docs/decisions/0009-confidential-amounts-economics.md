# ADR 0009: Confidential-Amounts Economics — Reconciling Value-Dependent Anti-Hoarding with Hidden Amounts

**Status**: Proposed
**Date**: 2026-07-16
**Decision Makers**: Core Team
**Related**: ADR 0006 (PQ + confidential-amounts target — the decision this ADR discharges the open design work for), ADR 0007 (bridge-import cluster tagging — the value-free provenance precedent generalized here), ADR 0003 (factor-1 wrap + demurrage-settlement), ADR 0004 (bridge amount revelation); issues #902 (CT↔economics spec epic), #904 (CT-implementation epic), #925/#831 (reset-charge doors), #955/#980 (Path C lottery implementation), #985 (D2 calibration), #577 (value-free elapsed wiring)

## Context

ADR 0006 ratified **confidential amounts** as the normative design: sent amounts are Pedersen commitments `C = vH + rG` with Bulletproofs range proofs, **fees stay public**, and every output carries a universal ML-KEM-768 encapsulation. It also flagged the load-bearing open problem: Botho's anti-hoarding layer is **value-dependent** at every site —

- **demurrage** ∝ value × time × factor,
- **lottery odds** historically ∝ value × cluster tilt,
- **cluster-tag blending** is value-weighted,
- and the **factor curve** reads *cluster wealth*, itself a value aggregate.

When amounts go hidden, each of these must be verified by consensus over commitments rather than plaintext. ADR 0006 sketched the pessimistic gadget set that would be required: homomorphic scalar relations + range proofs for demurrage, blend-correctness proofs for tags, and — the single hardest construction — a **Merkle-sum / proof-of-liabilities weighted-sampling sort** for the value-weighted lottery. The Curator escalation on #902 (2026-07-14) further raised the stakes: the live CLSAG input struct publishes the real spent value per input (`pseudo_output_amount`), so under amount-matching analysis ring-20 collapses to ~1 candidate — **without CT the sender rings are decorative**, making this epic mainnet-critical rather than a privacy nicety.

Over four rounds of design research + deterministic calibration simulation (PRs #952, #975, #985, all reusing the shipped `ClusterFactorCurve` / `demurrage_charge` / `TagVector::mix` / `calculate_gini` kernels) the #902 epic resolved **all four** sub-problems — and the two hardest-stated gadgets (the weighted-lottery ZK sort and the tag-blend correctness proof) **dissolved entirely** under value-free reframes. This ADR records the ratified design as the canonical reference the #904 CT-implementation epic will cite. It is a design record, not a code change: implementation rides a later reset batch (see Consequences).

The three research memos this ADR ratifies:

- `docs/research/ct-compatible-lottery-selection.md` — Path C, the §7 realized-capture round, and the §9 reward-cap round (#952).
- `docs/research/ct-economics-gadgets.md` — the demurrage inequality and the tag-blend dissolution (#975).
- `docs/research/ct-provenance-factor-calibration.md` — the `EpochOrigin` factor calibration, decision D2 (#985).

## Decision

Confidential amounts reconcile with Botho's value-dependent economics using **no Merkle-sum tree, no ZK weighted-sampling sort, and no proof system beyond the ADR-0006 Bulletproofs**. Each of the four sub-problems is resolved value-free or with a single one-sided range-proof inequality:

### 1. Lottery selection — Path C (value-free uniform draw + circulation window + reward-cap)

**Problem under CT.** The incumbent redistribution lottery draws winners with weight `value × (max_factor − factor + 1)` (`SelectionMode::ClusterWeighted`). Its progressivity and its Sybil-resistance both rest on the weight being **linear in the hidden UTXO value** — splitting conserves value hence conserves weight. Reading `value` in a consensus weight forces the Merkle-sum weighted-sampling sort. A value-free reframe (key on public fee/age) recovers redistribution but **provably cannot** be split-invariant: any inverse-of-a-per-coin-signal is multiplied for free by fragmentation (a whale splitting into 1,000 coins jumps from ~0–1% to ~91% of tickets; `ct-compatible-lottery-selection.md` §5).

**Ratified resolution — Path C.** Retire the value tilt entirely (demurrage + emission already carry stock-level redistribution) and draw **uniformly over the last-N circulation window**:

- **Uniform** draw, **no value tilt**, **no fee floor** (a fee floor is regressive under CT — a fresh coin's fee is ≈ base regardless of hidden value, so a floor inverts to a whale subsidy; §7.2).
- **Circulation window** `N ≈ 10,000 blocks (~14 h)`; only coins spent-and-recreated in-window are eligible (idle whales self-exclude; base-fee floor is the only value gate).
- **Endogenous reward cap** `R = min(actual_fee_pool, ρ · base_fee)`, where `ρ` = the count of **all** in-window outputs (the whale's own splits included) and `base_fee = 0.25 BTH`.
- **Carry-forward** the over-cap excess in a lottery-drained reserve — **not burn**.

**Why it is value-free / ZK-light.** The draw reads only public fee/age/output-count signals — no hidden value anywhere, so **no proof**. Sybil-safety is **net-zero and regime-independent** and comes from arithmetic, not cryptography: because consensus must count the whale's own outputs in `ρ`, splitting pins `R = (ρ_o + k)·base_fee`, so a whale's winnings `k/(ρ_o + k)·R = k·base_fee` exactly equal its `k·base_fee` fee cost — realized net = 0 at every split factor `k` (strictly negative when the fee pool is thin). This is the regime-independent Sybil bound §7 showed only the ZK sort (Path B) could otherwise deliver. Redistribution is **preserved and improved**: carry-forward gives ~100% throughput and Δgini **+0.1144**, beating the value-weighted incumbent's +0.0382; the progressivity lives entirely on the **public-fee source side** (demurrage-heavy fees in), with a Sybil-neutral draw out. **Already implemented (#955/#980).**

### 2. Cluster-tag blending — dissolves (consensus enforces an upper bound, not the exact blend)

**Problem under CT.** The issue framing asked for a **ZK blend-correctness proof**: prove that an output's cluster-tag weights are the correct value-weighted mix of the inputs, over hidden values.

**Ratified resolution — no gadget.** This is a problem consensus **does not actually have**. `validate_cluster_tag_inheritance` (`transaction/core/src/validation/validate.rs`) enforces a mass **upper bound** — "output tag masses must not exceed the decayed input tag mass" — and rejects only `actual > expected` (variant `ClusterTagInflation`). It does **not** verify the exact value-weighted blend: **deflation is legal** and is separately *priced* (the ring-floor order statistic + the #925 reset charge), so a holder gains nothing by under-attributing. The inflation bound is value-free — `weight_out(c) ≤ max_i weight_i(c)` over **public tags only** — and is over-permissive solely in the fee-*raising* direction, which is safe against rational fee-dodging.

**Why it is value-free / ZK-light.** The load-bearing anti-deflation floor is already a value-free **order statistic** over ring members' implied factors, the same reframe the demurrage age leg is intended to use. No blend-correctness proof, no committed inequality, **no ZK** (`ct-economics-gadgets.md` §5). This is the second of the two hardest-stated gadgets to dissolve.

### 3. Factor-curve input — EpochOrigin (value-free provenance factor)

**Problem under CT.** Both the demurrage factor and the progressive-fee factor trace to **cluster wealth** `W(c) = Σ v·weight(c)` — a value-weighted aggregate, and the *one* quantity that genuinely goes hidden under CT. This is the irreducible core both gadgets bottom out on (`ct-economics-gadgets.md` §4). Two resolutions were on the table: **Option A**, an octave-bucket range proof over a committed cluster-wealth aggregate (the curve is already log2/octave-structured, ~2 range proofs, ~1–2 KB/spend, exact-ish, real crypto); or **Option B**, a value-free provenance factor keyed on public signals.

**Ratified resolution — Option B / `EpochOrigin`** (decision D2, #985), the direct generalization of ADR 0007's bridge-import tagging to *domestic* coins:

- The factor keys on **public coinbase/mint-epoch pool wealth** — `ClusterFactorCurve(Σ public coinbase in [mK, (m+1)K))` — because PoW-bound minting makes coinbase amounts public.
- It **decays only by real circulation** (the value-weighted `TagVector::mix`, in which self-spends contribute zero incoming background value and are therefore a no-op).
- **Floored at `F = 1.5×`, `K = 17,280 blocks (1 day)`** — both inherited unchanged from ADR 0007.

**Why it is value-free / ZK-light.** No proof at all — the input is already public. Calibration (#985) shows it recovers **100.2%** of the value-weighted baseline's Δgini (+0.1019 vs +0.1017) **and** holds the Sybil bar: split / churn / self-hop leave the factor unchanged (5.745× → 5.745×); only genuinely acquiring background value lowers it (→ 1.474×, at real cost). The intuitive alternatives are **disqualified**: `Age` and `HopCount` score *higher* on honest Δgini (118–119%) but are **gameable for free** — a whale churns to age-0 or self-hops to wash, faking a 1× factor and flipping the lottery regressive (gamed Δgini **−0.025**). That is the identical "structure the holder controls for free" failure Path C found in the lottery; `EpochOrigin`'s circulation-gated decay is what closes it.

### 4. Demurrage over hidden value — one-sided Bulletproof inequality

**Problem under CT.** The demurrage charge is `d = k·V`. A naive public relation `fee = base + k·V` with public `fee, base, k` reveals `V = (fee − base)/k` exactly, defeating CT for every wealthy coin. Consensus must verify the charge covers what is owed without reading `V`.

**Ratified resolution — a committed-demurrage term + a one-sided inequality.** Every factor of the charge **except `V`** is public: the coefficient `k` is a public function of factor, elapsed, rate, and horizon. The prover commits `C_d` and proves two one-sided inequalities as a single aggregated Bulletproof over 2 scalars:

- `q·d − p·V ≥ 0` — the committed charge is **at least** the owed demurrage (`charge ≥ owed`);
- `fee − base − d ≥ 0` — the **public** fee **covers** the charge (`fee covers charge`).

**Why it is value-free / ZK-light.** It reuses the ADR-0006 range-proof machinery directly — no new crypto — at **~0.7 KB/spend**, a few ms to verify, batchable per block. A value-free reframe does *not* dissolve it (the charge is intrinsically linear in `V`), but it is trivial and cheap. The **#925 capitalized-reset charge and the #831 settlement charge fold into the same `k*·V` gadget** — one construction prices all three (`ct-economics-gadgets.md` §2).

### Companion policy decisions

- **D1 — mandate demurrage-fee quantization.** Recommended *mandate* (not optional overpayment): a hard privacy floor for wealthy coins, so a privacy-seeking prover overpays to a coarse public bucket and the leak is capped at the fee-quantization granularity. Finalized in the CT normative spec.
- **D3 — inflation bound = value-free max-weight** (§2 above), not the exact homomorphic committed inequality. Default; the committed inequality stands only if a maintainer later wants exact mass conservation.

## Bottom line

Confidential amounts (ADR 0006: Pedersen commitments + Bulletproofs, public fees) reconcile with Botho's value-dependent economics using **NO Merkle-sum tree, NO ZK weighted-sampling sort, and no proof system beyond the ADR-0006 Bulletproofs**. The two hardest-stated gadgets — the weighted-lottery ZK sort and the tag-blend correctness proof — **both dissolved** under value-free reframes. What remains is exactly **one** linear-inequality gadget (the demurrage charge, ~0.7 KB/spend, reusing the existing range-proof path) plus the value-free `EpochOrigin` provenance factor. The full picture:

| Sub-problem | Ratified resolution | Ratified parameters | ZK cost |
|---|---|---|---|
| Lottery selection | **Path C** — value-free uniform draw + circulation window + reward-cap (net-zero splitting) | `N ≈ 10,000 blocks`; `R = min(fee_pool, ρ·base_fee)`; `base_fee = 0.25 BTH`; carry-forward excess (not burn); implemented #955/#980 | **none** |
| Cluster-tag blending | **Dissolves** — consensus enforces a mass **upper bound** only (`validate_cluster_tag_inheritance` rejects `actual > expected`, variant `ClusterTagInflation`; deflation legal + priced) | value-free `weight_out(c) ≤ max_i weight_i(c)` | **none** |
| Factor-curve input | **EpochOrigin** — value-free provenance from public coinbase/mint-epoch wealth, circulation-only decay | `F = 1.5×`, `K = 17,280` (from ADR 0007); 100.2% Δgini recovery; Sybil-invariant | **none** |
| Demurrage over hidden value | One-sided **Bulletproof inequality** (`charge ≥ owed` + `fee covers charge`) | coefficient `k` public, only `V` hidden; ~0.7 KB/spend; #925/#831 fold in | **~0.7 KB/spend** |

## Consequences

- **#902 is fully designed and ZK-light.** The CT-implementation epic (#904 CT half) is decomposable with a known, minimal cryptographic surface: the existing Bulletproof range-proof path plus one committed-demurrage inequality. No Merkle-sum insertion/update cost, no weighted-sampling grinding analysis against the finalized-hash seed, no blend-proof aggregation.
- **The disclosure triad is Botho's stated privacy model.** Who — hidden (rings + stealth); how much — hidden (commitments); which wealth-lineage — public **by design** (the economics prices it). The value-free reframes are what make lineage-public / amount-hidden a coherent, cheap posture rather than a contradiction.
- **Consensus-breaking but independent of amount-hiding.** Path C and the `EpochOrigin` factor change consensus, but the current value-weighted lottery and value-weighted factor work fine while amounts remain public. These changes are only *needed* when CT amounts land.
- **Implementation rides a later reset batch, not 6.0.0.** The CT output-format break (#904 CT half) pairs with nothing pending on the imminent 6.0.0 reset, so it rides the **CT-amounts batch** on a later reset. This ADR records the design; it ships no code. (All pre-mainnet — testnet is ephemeral, no fork concerns.)
- **Generalizes the ADR-0007 precedent inward.** ADR 0007 gave *imported* wealth a value-free factor from public bridge-boundary amounts and explicitly "sidesteps the #902 problem" for imports; `EpochOrigin` applies the identical epoch-keyed, circulation-decayed, `F`-floored construction to *domestic* mint-origin wealth. The bridge and domestic factors are now the same mechanism keyed on two public origin sources (unwrap amounts / coinbase amounts).

## Residuals (disclosed)

Recorded honestly, in the spirit of ADR 0007's residual disclosure:

1. **EpochOrigin drip-mine across epochs.** A whale can accumulate a large low-factor position by minting slowly across many epochs (spreading origin wealth so no single epoch pool crosses a high curve tier). This is **bounded by `K`** (each epoch costs `K` blocks of wall-clock) and **throttled by emission economics** (you can only mint so fast). It is the **identical** residual ADR 0007 already ratified for the bridge — time-as-cost replacing provenance-as-cost — and is accepted on the same grounds.
2. **Demurrage public-fee leakage (factor-conditional).** The public fee is the intrinsic amount-leakage channel under CT, and it is **factor-conditional**:
   - **Factor-1 (background / commerce) coins pay `k = 0` → zero value leakage** — a strict improvement over the current public-amounts table.
   - **Wealthy coins at the minimum fee reveal value nearly exactly** (the irreducible public-fee leak). The one-sided inequality lets a privacy-seeking prover **overpay to a coarse bucket**, bounding the leak at the **fee-quantization granularity** (D1 mandates the quantization so this floor is guaranteed, not optional).

   The CT normative spec declares this as the accepted leakage budget and updates the whitepaper §9 `tab:metadata-leak` row, rather than discovering it at audit.

## Alternatives considered

1. **ZK weighted-sampling sort for the lottery (Path B — Merkle-sum / proof-of-liabilities).** Unavoidable *iff* strict split-invariance is required from the lottery weight. Rejected because Path C delivers a **stronger** (net-zero, regime-independent) Sybil bound from public signals alone, with no cryptography, while improving redistribution.
2. **Octave-bucket range proof for the factor (Option A).** The curve is already log2/octave-structured, so ~2 range proofs place committed cluster wealth in a public octave (~1–2 KB/spend, exact-ish). **Stands as the fallback** for a maintainer who rejects the `EpochOrigin` drip-mine residual and wants the exact live aggregate; rejected as the default because `EpochOrigin` holds both bars with zero proof.
3. **Value-free `Age` / `HopCount` provenance factors.** Score higher on honest Δgini (118–119%) but are **gameable for free** (churn → age 0 → 1×; self-hop wash → 1×), flipping redistribution regressive (gamed Δgini −0.025). Disqualified on the Sybil axis.
4. **Fee floor as a lottery value gate.** Regressive under CT — a fresh coin's fee is ≈ base regardless of hidden value, so a floor clears only aged/consolidated coins and at `F = 1.0` inverts to hand the whale 100%. Dropped; the base-fee floor is the only value gate.
5. **Burn the over-cap lottery excess.** Sybil-safe but throttles redistribution to a 1–9% trickle (Δgini collapses to +0.0038). Rejected in favor of carry-forward, which is Sybil-safe *and* redistributes in full.

## Implementation note

Two citation nits from the CT-gadgets research must **not** be reintroduced when the normative `anvil:spec` authoring proceeds:

- The value-free `ring_elapsed_quantile` order statistic (the intended CT-clean `elapsed` signal) is **not yet wired** — the live path still uses the value-weighted `ring_elapsed_centroid` (#577). `elapsed` is *designed* value-free but is not CT-clean until #577 lands.
- The tag-mass inflation error variant is **`ClusterTagInflation`** (not `MassInflation`).

## References

- ADR 0006 — PQ & privacy ratification (confidential amounts = the open design work this ADR discharges), decision 1
- ADR 0007 — bridge-import cluster tagging (the value-free provenance precedent `EpochOrigin` generalizes; source of `F = 1.5×`, `K = 17,280`)
- ADR 0003 / ADR 0004 — bridge peg + amount revelation (public bridge-boundary amounts)
- `docs/research/ct-compatible-lottery-selection.md` — Path C, §7 realized-capture, §9 reward-cap (#952)
- `docs/research/ct-economics-gadgets.md` — demurrage inequality + tag-blend dissolution (#975)
- `docs/research/ct-provenance-factor-calibration.md` — `EpochOrigin` calibration, decision D2 (#985)
- #902 (CT↔economics spec epic — the four sub-problem ratifications), #904 (CT-implementation epic), #925 / #831 (reset-charge doors folded into the demurrage gadget), #955 / #980 (Path C implementation), #577 (value-free `elapsed` wiring)
