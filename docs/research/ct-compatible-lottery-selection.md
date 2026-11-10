# CT-Compatible Lottery Selection: Can a Value-Free Rule Avoid the ZK Sort?

**Issue:** [#902](https://github.com/botho-project/botho/issues/902) (the lottery sub-problem of the CT-reconciliation epic; ADR 0006 decision 1)
**Status:** Historical design research and expected-value models; reconciled 2026-11-07 after #1977. No consensus change. The value-free selection direction is ratified, but that structural decision does not establish economic neutrality. Current quantitative production profitability, throughput, Gini and stability conclusions from these sweeps **need production-mode re-run**; the historical findings and scoped formal refutations remain. See the [claim ledger](#evidence-and-claim-reconciliation).
**Reproduce:**
```
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim -- lottery-selection-sweep
```
Sim source for §§3–4: `cluster-tax/src/simulation/lottery_selection_sweep.rs` (expected values, no RNG; `SEED=0xB07A_0902` is documentary). §7 uses `lottery-sybil-brake-sweep`; §§9.1–9.3 use `lottery-reward-cap-sweep`, except §9.1.1, which uses `lottery-reward-cap-sweep --faithful`. The [surface ledger](#evidence-and-claim-reconciliation) records their distinct premises. It reuses the shipped kernels: the real `ClusterFactorCurve`, `demurrage_charge`, the real `LotteryCandidate::weight` / `SelectionMode` for the Uniform and ClusterWeighted yardsticks, and the shared `simulation::metrics::calculate_gini`.

## Evidence and claim reconciliation

**Audit date/source:** 2026-11-07 UTC, after #1977 / [PR #1979](https://github.com/botho-project/botho/pull/1979), merged at `9b39bf2c3637588291ead26ca5c44e957b6e51a3`, tree `a4a07e0974b2b5c776d89fdfbf368dde55a5049e`. Locations in the coverage ledger refer to that base. This pass changes attribution and disposition, not numerical tables, software, theorem premises or policy. In particular, #1800's restatement, the separate M2/redesign measurements and rankings, and operator decision #1837 remain intact.

### Surfaces and assumptions

The IDs below are the existing entries in [`sim-divergences.json`](../../formal-drt/sim-divergences.json), not a new registry. Run commands as `cargo run --locked --release -p bth-cluster-tax --features cli --bin cluster-tax-sim -- <arguments>`; the checked-in labels and the existing research checker bind each command to its inventory ID.

| Key / existing surface | Memo sections and exact arguments | Model, parameters and fee/RNG premises | Disposition |
|---|---|---|---|
| R1 `sweep.selection` | §§3–4; `lottery-selection-sweep` | Fixed 100-holder population in §4.1, 10-year annual redistribution, 100k-BTH splitter and tested K values; hypothetical base fee plus demurrage proxy. No RNG; documentary `SEED=0xB07A_0902`. | Retain expected-value weight/Gini comparisons and structural signal analysis. Current production economic conclusions: **needs production-mode re-run**. |
| R2 `sweep.sybil_brake` | §7; `lottery-sybil-brake-sweep` | Stationary capture/window model, ρ=2/20, R=3.8/20 BTH and N=100/1k/10k/100k; fee-floor and owner-cap scenarios as tabulated. Recurring hypothetical per-output base fee; honest redistribution is annual demurrage. No RNG; documentary `SEED=0xB07A_0902_2`. | Retain conditional capture, fee-floor and honest-population findings; no current production safety/Gini guarantee. **Needs production-mode re-run** for those quantitative conclusions. |
| R3 `sweep.reward_cap` | §9.1, §9.2 and historical §9.3; `lottery-reward-cap-sweep` | Per-block arrival-rate cap, assumed 0.25-BTH charge for every created ticket; ρ=2/20/200, k=1/5/50/500/5000, N=1k/10k/100k; annual-lump throughput and 10-year Gini model. No RNG; documentary `SEED=0xB07A_0902_3`. | Retain conditional algebra and historical tables. Withdraw current neutrality/throughput/Gini/stability verdicts: **needs production-mode re-run**. |
| R4 `sweep.reward_cap.faithful` | §9.1.1; `lottery-reward-cap-sweep --faithful` | Stationary economics plus synthetic candidate-window sampling; exact grid and assumptions below. Whale fee mode varies; organic fees do not. | Retain historical 192/192, +2.47-BTH/block, backlog and remedy results. `CurrentCode` is an approximation; other rules are unimplemented candidates. Current quantitative production economics: **needs production-mode re-run**. |
| S structural selection | §§1–2, 4.4–6; shared lottery primitives | Uniform weight reads no hidden amount; public signals and a value-free sort are structural properties. ClusterWeighted is the historical comparison, not the current default. | Retain structural CT/public-signal facts and ratified direction; they do not establish economic neutrality. |
| F formal models | [Sybil](../../formal/Botho/Lottery/Sybil.lean), [expectation](../../formal/Botho/Lottery/SybilExpectation.lean), [theorem guide](../../formal/lottery.md) | Four-row hash-independent counterexample; separate stationary-economy results; ideal-roll and IID-u128/modulo-bias premises. | Retain the refutation of unconditional neutrality with its actual hypotheses; no Rust-equivalence or production-economics proof. |
| B `production.transition` behavior | [paired fixture](../../botho/tests/sim_code_parity.rs), `cargo test --locked -p botho --test sim_code_parity` | Actual `new_production` / `apply_production_block`, canonical node comparison, external ordered candidates/owners, fees/emission/reward and already-derived V2 seeds. | Retain only the executed carryover/fresh-maturity behavior below. No profitability, equilibrium, discovery or seed-derivation claim. |

R1–R3 do **not** implement the production 800-permille fee split. Their hypothetical `LOTTERY_BASE_FEE_PICO` is not an enforced per-ticket consensus floor. R3's arrival rate must not be equated with the implemented eligible-window count. Source modules are `cluster-tax/src/simulation/{lottery_selection_sweep,lottery_sybil_brake_sweep,lottery_reward_cap_sweep}.rs`.

R4 uses `lottery_reward_cap_faithful.rs`: epochs {0,1,3,tail} × backlogs {0,1000 BTH} × organic rates {2,20,200} × whale modes {`ConsensusOnly`,`ConsensusPlusRelayMin`} × factors {1×,6×} × capacities {proposer slot,20-MiB block bound} = 192 points. Organic transaction fees always use `ConsensusPlusRelayMin`. It models an 800-permille pool, Uniform/four winners, age 720, minimum 1,000,000 pico, circulation window 10,000, a truncated 10,000-candidate set, a free-ticket fixed point and the stated backlog/epoch/capacity assumptions. SplitMix64 generates synthetic transaction-key ordering and offsets with `0x1682_FA17_0902 ^ ((rho_o as u64) << 32) ^ (k as u64)`; each of four settings uses 400 offsets. It does not execute stateful payout transitions or discover ledger candidates. The present code includes shared primitives and parity guards; neither that reuse nor current reproduction converts R4 into a production economic experiment.

F keeps distinct proof scopes. `sybil_net_zero_refuted` gives four eligible rows, three owned by the splitter: every hash function pays the owner three base fees in one draw, exceeding its stated total ticket cost when per-ticket cost is below the base fee. The per-height statement is **not** repeated payment of the same four persistent outputs. `split_never_pays_iff` and `break_even_reference` concern a stated stationary economy. Ideal-roll expectation has its own uniform-roll premises; the bridge to modulo-hash sampling assumes IID-u128 inputs and includes multiplicative bias slack. DRT supplies finite conditional correspondence, not a proof that Rust implements every theorem or that cryptographic outputs satisfy those assumptions.

### Consumer and conclusion coverage

All seventeen consumers were checked, including repeated local citations. “Historical” here refers to the dated studies below; the 2026-11-07 reproduction and B behavior run have separate identities. Each R-key incorporates the exact model/parameter/fee/RNG row above. A linked historical finding is retained; interpreting it as a quantitative current production conclusion is deferred as **needs production-mode re-run**.

| Consumer / locations at audit base | Claim and evidence class | Local disposition |
|---|---|---|
| This memo: introduction/default/method; §§4–8; §9 cap, faithful grid, remedies, throughput/Gini and terminal verdict | S, R1–R4, F, B | Correct mapping and no-RNG description; retain historical rows and conditional refutation; withdraw unconditional neutrality and current throughput/Gini/stability verdicts. |
| [CT economics gadgets](ct-economics-gadgets.md): opening, D2 empirical bar, §7 lottery/closing analogy | S, R3–R4 | “Solved” applies only to avoiding the hidden-value sort; economic resolution is not established. Other gadget findings unchanged. |
| [Research index](README.md): lottery and redesign rows | R1–R4 | Label historical/model findings and link this reconciliation. |
| [Redesign study](redistribution-redesign-study.md): inputs, summary table context, §2 churn, §4 baseline/“node”/churn, §7 and §8 ranking | R4 inherited evaluator plus separate candidate experiments | Qualify splitting as modeled node-rule evaluation; preserve all candidate measurements, table headings, rankings and other axes. |
| [ADR 0009](../decisions/0009-confidential-amounts-economics.md): Path C and source links | S, conditional R3, future CT1 gate | **No source change:** already labels historical calibration and per-created-ticket premises, requires fresh CT1 simulation, and preserves structural ratification. |
| [Cluster-tilted design history](../design/cluster-tilted-redistribution.md): status grid, payout-supersession paragraph, 2026-11-01 history | R4; separate M2 evidence | Qualify grid and re-run attribution; retain #1800 restatement and the historical tilted-draw results. |
| [Lottery design](../design/lottery-redistribution.md): current-status header and historical context | R4, S | Attribute profitability to the historical model; preserve mechanism and original proposal analysis. |
| [Tokenomics](../concepts/tokenomics.md): fee-schedule caveat, Redistribution Status and fee-redistribution FAQ | R4 | Qualify both profitability claims; retain M2 results and the operator gate. |
| [Threat model](../security/threat-model.md): residual/mitigation, defense list, Redistribution Economics status and ticket subsection | R4, S, F | Scope repeated profit citations; retain unresolved capture risk, free-ticket facts, separate mitigations and risk/audit judgments. |
| [Monetary chapter](../../whitepaper/sections/07-monetary.tex): 192-point mechanism discussion and summary | R4, S | Historical-model attribution; mechanism and mathematical descriptions unchanged. |
| [Economics chapter](../../whitepaper/sections/10-economics.tex): status list, repeated re-run citation, grid and candidate gate | R4; separate M2 evidence | Separate the splitting model from M2 and current production evidence; preserve redesign criteria. |
| [Executive summary](../../whitepaper/botho-executive-summary.tex): redistribution status | R4 | Replace present-tense production profitability with historical-model finding. |
| [Security chapter](../../whitepaper/sections/09-security.tex): splitting conclusion | R4 | Qualify the lottery finding; fee and wash-trading conclusions unchanged. |
| [Progressive fees](../concepts/progressive-fees.md): status and splitting attack | R4 | Qualify both lottery claims; preserve fee-rate and M2 conclusions. |
| [External audit scope](../security/external-audit-scope.md): project summary and lottery scope | R4 | Attribution only; scope, readiness, freeze and engagement judgments unchanged. |
| [M2 verdicts](m2-production-verdicts.md): “No lottery farming” assumption | R4 imported premise | Identify historical model provenance; preserve every M2 result, assumption direction and operator ruling. |
| [Asymmetric fees history](../design/asymmetric-utxo-fees.md): superseded-status paragraph | R4, S | Qualify profit attribution; original proposal analysis unchanged. |

### Historical record and current reproduction

[PR #1761](https://github.com/botho-project/botho/pull/1761) reviewed head `08182f61ad4ad433aeaff7396ad2e6391dcb18ae`, tree `be42218797af51e614501a4fb45965b1ac5d51dc`, merged as `8b1873580b51ebce3d410e30fada2b9652310443` on 2026-11-01 UTC. It **reported** 19 cluster-tax tests, two node parity tests and two byte-identical sweep runs. Original runtime/binary and config hashes were not retained here and remain **unavailable**; current CI is not retroactive evidence for those executions.

Original snapshots arrived in [#1794](https://github.com/botho-project/botho/pull/1794), commit `2fb255d24619c5a5433aaa30fe6c129435a63e17`, tree `6bb29621557d0dd03683e3fe899396f2b641fcc7`. The legitimate brake repair [#1913](https://github.com/botho-project/botho/pull/1913) is commit `64b318bcc586c4b62004935a0f1da4849da7bd10`, tree `0fb6ec14d317a8fb783965936ac0bf26fdee4cec`. Its repaired brake values are preserved, not reverted to #1794. All 22 numerical table blocks across the four current reports match that post-repair source. SHA-256s of the historical raw snapshots are:

- R1: `60ac03be2ffe79714bbf7b205870ee69fbe5aa18fd8d567753dd4dc0af8f6087` (both commits).
- R2: `11e16ee3281b6fb48bdb01869a92ef1f44fa4d577558f0eb3bff927c758e4b2a` at #1794; `407f532f458614ad5e809a009bef8c290917e0f3671fc172644ba1e65eba67c4` after #1913.
- R3: `16387d3abc2c7d2a4818581fa9cd3bef93695892479a1a39eb982ff40bc7daa7` (both).
- R4: `5950175ffa0a4e6509e8698331a875cc326229ce31bd267da408647b405d933b` (both).

#1977's final reviewed head is `fd1a108e464ff29077eb5fd5484ad8fdeb77ed63`. Its two final runs checked out merge commit `b67a7337b963ce8c929af786bd42ab8f77d969a5`, tree `a4a07e0974b2b5c776d89fdfbf368dde55a5049e`, subsequently merged as the audit base above. The [Research run 42414359424](https://github.com/botho-project/botho/actions/runs/42414359424), attempt 1, job `128529118719`, passed all 22 output/verbatim checks and the committed-file drift guard (651 library tests passed / 1 ignored; nine helper tests passed). [Artifact 13590778917](https://github.com/botho-project/botho/actions/runs/42414359424/artifacts/13590778917) contains the actual CLI Cargo build receipt, original goldens, raw outputs and update/check receipts. Its context is clean; all 74 recorded source/config hashes match the reviewed Git objects. Archive SHA-256: `1fd8851b49a246689049352c1f62126d29ee080be1aa3ed2a3f6824e793e5c78`; check receipt: `5c819a2089b1d4e583fe2469ebe826d1c9015a72e4a20361be25e6dcf0d1d6ec`; CLI executable: `1c506db5cfff88c57e503a3fc8d0070dad26b5d93f8be9654300b3a804c9fcc2`.

The four labeled current raw-output SHA-256s are R1 `d224be2c9f267408dc7e3e2b0257a866eefac92b2a8129f391ecf1b1f5965c6a`, R2 `effe3c0df8f5b59d3a6d7f7bcde7bff43e9c647224b75f476b60a5d7203354b0`, R3 `08fe8324aec476674f2133a9f89a14f057379eb129bc69fb120dc1d87688a23b`, R4 `cdec3de1915705de759d558246bbbab727a20bf274e9e0e898f401eb30657dbe`. Their numerical tables, and each pre-label body after its title, are unchanged. The earlier [generation run 42374615026](https://github.com/botho-project/botho/actions/runs/42374615026) at `d795afe1036d4a127bb9b08e556274466018a3c8` built/generated/checked successfully but failed the retained tracked-golden diff guard before label import; it is not the green final-source run.

### Executed two-block behavior example

[Workspace run 42414359411](https://github.com/botho-project/botho/actions/runs/42414359411), attempt 1, job `128529118580`, executed the full 15-test parity target at the same reviewed head/merge checkout/tree above. [Artifact 13591866333](https://github.com/botho-project/botho/actions/runs/42414359411/artifacts/13591866333) retains its Cargo test-executable build, raw streams, fresh `production-lottery.json` and execution receipt. SHA-256s: archive `eec2c75a9d058e51c1f4c77f6947132008de7a4ff9ebab2dc24a95ea10f2aa06`; test executable `990f332187b7d3e1bc3ac8aa0586118520743ab17d7c636ab3823594e171fbdf`; execution receipt `6d9aee725278ea6c346de3a7c9b3a134c25067cd633f8441311df0137c639acf`; payload `b7f110aa1130c4209fb6e8690becfb170cfb073048a701a5b5d5beda34eed407`; inputs `37a317dd73b1777616d1f8d4c3d687cd12306dd97da078925691ee70d932de51`; config `933df63aebc7eb0c2b5fab5fe1a04feddb1bc770332920d8fcb26740d6e43399`.

The production context was **dirty**, with identical build/execution contexts and all 74 recorded source/config hashes equal to Git. Read the [original handoff](https://github.com/botho-project/botho/pull/1979#issuecomment-6713332419) with the [exact-patch supplement](https://github.com/botho-project/botho/pull/1979#issuecomment-6713360218): same-run [CT1 artifact 13591931304](https://github.com/botho-project/botho/actions/runs/42414359411/artifacts/13591931304) reconstructs the entire recorded patch `f446863680c9e1251ab8637934e6eff528a517d987206505602e1af8c6479634`. Only `scripts/research/ct-economics/{marginal-summary,summary,workload-summary}.json` differ; matching the runner's seven-digit Git patch-header abbreviations resolves the earlier nonmatching local reconstruction. This explains the context without turning the example into economic evidence. Original ZIPs, source archives, receipts and the complete reconstructed patch are retained outside disposable worktrees in `.loom/diagnostics/issue-1977-evidence/` and `.loom/diagnostics/issue-1979-root/`.

Each trace starts independently with four registered outputs and 10,000,000 pico carryover. Config: Uniform, four winners, minimum age 720, minimum value 1,000,000 pico, circulation window 10,000, fee pool 800 permille. Block one is height 720, fees 1,250,000 pico, emission 2,000,000, reward limit 4,000,000, supplied V2 seed `[73; 32]`: fee pool 1,000,000, burn 250,000, available 13,000,000, four fresh 1,000,000 awards and carryover 9,000,000. Block two offers only those awards, with fees 2,500,000, emission 3,000,000, reward limit 4,000,000 and supplied seed `[74; 32]`:

| Independent trace: second height / age | Available (pico) | Paid (pico) | Carryover (pico) | Final registered outputs | Each owner's final balance / winnings (pico) |
|---|---:|---:|---:|---:|---|
| 1439 / 719 | 14,000,000 | 0 | 14,000,000 | 8 | 4,000,000 / 2,000,000 |
| 1440 / 720 | 14,000,000 | 4,000,000 | 10,000,000 | 12 | 6,000,000 / 4,000,000 |

Both traces collect total fees 3,750,000 and burn 750,000 pico. Original outputs remain unchanged; later awards have fresh IDs and creation heights. The no-winner result's zero seed is distinct from supplied `[74; 32]`. Ordered candidate snapshots, owner mapping, collected fees, emission/reward schedule and derived V2 seeds are **external inputs**. This finite synthetic test observes actual transition behavior and node parity; it does not measure profitability, prove economic equilibrium, discover ledger candidates or validate seed derivation.

### Remaining obligations

The original [#1683](https://github.com/botho-project/botho/issues/1683) criteria remain: premise enforcement/acceptance, realistic strategy pass criteria, parity CI, broader whitepaper/audit reconciliation, and any required single protocol bump/testnet reset. This ledger does not satisfy its anti-profitability exit criterion or change it. [#1709](https://github.com/botho-project/botho/issues/1709) still requires independent audit of its production-mode, DRT, divergence and provenance obligations. [#1970](https://github.com/botho-project/botho/issues/1970) maps this citation work and #1977's labels/actual behavior into that audit; policy [#1837](https://github.com/botho-project/botho/issues/1837) remains open. Current quantitative economics identified above is **needs production-mode re-run**, rather than inferred from table reproduction, conditional model theorems or behavior parity.


---

## 1. The problem: value-weighted selection breaks under CT

The redistribution lottery (`cluster-tax/src/lottery.rs`, `botho/src/consensus/lottery.rs`) draws winners with weight

```
weight = value × (max_factor − factor + 1)      // SelectionMode::ClusterWeighted (historical default)
```

This was the historical incumbent; the current canonical draw is Uniform. In the historical comparison, `docs/design/cluster-tilted-redistribution.md` describes ClusterWeighted as the **only split-invariant progressive mode** among the studied weights. Its power comes from one structural fact: the weight is **linear in the UTXO value**. Splitting a position into many outputs conserves total value, hence conserves total weight — a whale gains nothing by fragmenting. The `(max_factor − factor + 1)` tilt keys off cluster provenance, which inherits through splits, so the progressive direction is Sybil-safe too.

Under **confidential amounts** (ADR 0006 decision 1: Pedersen commitments + Bulletproofs, fees public) the `value` is hidden. A consensus weight that reads it forces a **ZK weighted-sampling proof over hidden amounts** — a Merkle-sum tree / proof-of-liabilities sort where the seed selects a cumulative-value ticket and the winner proves coverage without revealing its amount (#902 §3). This is the single hardest gadget in the CT epic: it needs a specified insertion/update cost, a proof size, and a grinding analysis against the finalized-hash seed.

## 2. The value-free reframe (and the fee-as-value-proxy argument)

The lottery is hard under CT **only because the selection rule reads the hidden value.** A rule keyed exclusively on signals that stay public under CT needs no proof over amounts. Three such signals exist:

1. **The transaction fee** — public by ADR 0006. Under demurrage the public fee already encodes value:
   ```
   fee ≈ base_fee + value × rate × (factor − 1)/(max_factor − 1) × time_held
   ```
   (`cluster-tax/src/demurrage.rs`). So the public fee is a **monotone proxy for `value × factor`** — the very quantity the progressive tilt wants. A rule that favours **low-fee** coins favours the poor / low-factor / freshly-circulated, recovering the progressive direction *for free* from already-public data.
2. **Circulation / age** — public from creation heights. A coin that has recently been spent-and-recreated (commerce) versus one held idle.
3. **Cluster factor / tags** — public chain state.

The reframe's promise: if a value-free rule *recovers* ClusterWeighted's redistribution while staying Sybil-resistant, the lottery sub-problem of #902 **dissolves** — no ZK sort, no Merkle-sum tree, no grinding analysis. This memo tests that promise.

## 3. Method

This historical selection study computes deterministic expected values, with no random draws. It compares selection weights rather than executing multi-agent production payouts. Five rules are scored:

| rule | weight | keys on | notes |
|---|---|---|---|
| **Uniform** | `1` | nothing | the "rely on statistics" null baseline |
| **ClusterWeighted** | `value × (max_f − f + 1)` | **hidden value** | incumbent; yardstick only, NOT CT-compatible |
| **FeeInverse** | `max_fee − fee + 1` | public fee | value-free |
| **CirculationGated** | `boost` if circulated else `1` | public age | value-free |
| **Hybrid** | `(max_fee − fee + 1) × circ_boost` | public fee + age | value-free |

Three scoring axes:

- **Redistribution** — a 10-year collect-and-redistribute loop over a realistic population (factors from the real curve). Each year every holder pays one year of ordinary `demurrage_charge` into a pool; the pool is redistributed by the rule's expected-value weights. Score = Δgini vs a **burn baseline** (same demurrage collected, redistributed to nobody). Higher Δgini = more equalizing. "Recovery" = Δgini ÷ ClusterWeighted's Δgini.
- **Sybil resistance** — a splitting attack. A 100k-BTH whale fragments its coin into K fresh equal coins; we measure its **share of total lottery weight** in a fixed 100-holder reference population, unsplit vs split. The share is bounded in [0, 1], so it is directly interpretable and captures the natural self-limit (splitting grows the denominator). Split-invariant ⇒ the share barely moves.
- **CT-compatibility / leakage** — does the rule read the hidden value (⇒ needs the ZK sort)? And does keying on the fee add leakage beyond what ADR 0006 already makes public?

The Uniform and ClusterWeighted weights are computed by calling the real `LotteryCandidate::weight` — the yardsticks are the exact consensus numbers, not a re-derivation. The whole selection report uses expected values (no RNG). Historical test/two-run claims are reported evidence; current reproduction is separately identified in the [evidence record](#evidence-and-claim-reconciliation).

## 4. Results

### 4.1 Population (factors from the real `ClusterFactorCurve`)

| tier | count | wealth (BTH) | cluster wealth (BTH) | factor | circulated |
|------|------:|-------------:|---------------------:|-------:|:----------:|
| background (poor) | 90 | 1000 | 1000 | 1.265x | yes |
| 50k-cluster wealthy | 4 | 5000 | 50000 | 3.023x | no |
| 500k-cluster wealthy | 3 | 50000 | 500000 | 4.617x | no |
| 10M-cluster whale | 3 | 100000 | 10000000 | 5.745x | no |

### 4.2 Track 1 — redistribution (Δgini vs a burn baseline)

10-year collect-and-redistribute. Initial Gini 0.7773; burn baseline Gini 0.7525. Δgini = burn − rule (higher = more equalizing).

| rule | final Gini | Δgini | recovery vs CW | CT-clean |
|------|-----------:|------:|---------------:|:--------:|
| Uniform | 0.6508 | +0.1017 | 266.3% | yes |
| ClusterWeighted | 0.7144 | +0.0382 | 100.0% | **no** |
| FeeInverse | 0.6455 | +0.1070 | 280.1% | yes |
| CirculationGated | 0.6396 | +0.1130 | 295.8% | yes |
| Hybrid(fee×circ) | 0.6389 | +0.1136 | 297.5% | yes |

**Reading it — the surprise is which direction the gap points.** Every value-free rule redistributes **more** than ClusterWeighted (recovery 266%–298%), not less. The reason is instructive: ClusterWeighted's weight is `value × (max_f − f + 1)`, and in a population where whales hold 100× the value of a poor holder, the **value term dominates the inverse-factor tilt** — a 100k-BTH whale at 5.745× still gets weight `100000 × 1255 = 1.25e8`, far above a poor holder's `1000 × 4735 = 4.7e6`. So ClusterWeighted routes a large slice of the pool *back to the whales*. The value-free rules don't read value, so they can't do that: FeeInverse gives the whale (highest fee) essentially zero weight, and Uniform/Circulation give it one ticket. **In this annual expected-value population model, the value-free rules have the larger redistribution score; their CT compatibility is a separate structural fact.** ClusterWeighted's split-invariance is *not* bought with superior honest-population redistribution — it was validated against a *gamed* whale (see §4.3), which is exactly where the value term earns its keep.

### 4.3 Track 2 — Sybil resistance (ticket share under splitting)

A 100k-BTH whale (factor 5.745×) fragments into K fresh coins, measured against the fixed 100-holder population. Share = attacker's fraction of total lottery weight (bounded [0,1]).

| rule | unsplit share | share @K=1000 | split-invariant | CT-clean |
|------|--------------:|--------------:|:---------------:|:--------:|
| Uniform | 0.990% | 90.909% | **no** | yes |
| ClusterWeighted | 8.625% | 8.625% | yes | **no** |
| FeeInverse | 0.000% | 91.260% | **no** | yes |
| CirculationGated | 0.110% | 91.659% | **no** | yes |
| Hybrid(fee×circ) | 0.000% | 91.695% | **no** | yes |

Ticket share by split count K (the [0,1] self-limit saturating):

| rule | K=10 | K=100 | K=1000 |
|------|-----:|------:|-------:|
| Uniform | 9.091% | 50.000% | 90.909% |
| ClusterWeighted | 8.625% | 8.625% | 8.625% |
| FeeInverse | 9.454% | 51.080% | 91.260% |
| CirculationGated | 9.901% | 52.356% | 91.659% |
| Hybrid(fee×circ) | 9.943% | 52.472% | 91.695% |

**Reading it — this is the whole story.** ClusterWeighted's attacker share is a flat **8.625%** whether the whale holds one coin or a thousand: value-linear weight is split-invariant by construction. **Every tested value-free weight loses split-invariance in this fixed-population comparison.** The whale climbs from ~0–1% of the tickets to **~91%** by fragmenting into 1,000 coins — it captures the payout stream. FeeInverse is the sharpest illustration: it gives a whale holding a single high-fee coin **0.000%** of the tickets (maximally progressive!), then hands it **91.26%** once it splits into fresh, base-fee coins. The progressive direction and the Sybil hole are the *same* per-coin term.

**Historical proxy-cost calculation: FeeInverse does not meaningfully self-limit under the assumed per-created-output base fee.** This hypothetical charge is not a consensus ticket floor. The only incremental cost of a 1,000-way split is `(K−1) × base_fee ≈ 249.8 BTH` (the demurrage the whale sheds is churn-invariant, so it is not an *extra* cost of splitting). Against capturing ~91% of an ongoing lottery pool, 250 BTH of one-time base fees is trivial — the base-fee floor is a linear tax on a super-linear prize. The self-limit that *does* exist is only the [0,1] share cap (you can't win more than 100% of tickets), which still lets the whale reach ~91%.

**Why this is structural, not a tuning failure.** Split-invariance requires the weight to be **linear in a conserved, per-position quantity**. Value is conserved under splitting; a *count* of coins is not; and the inverse of any per-coin signal (fee, age) is a per-coin term the holder multiplies for free by splitting. The only public quantity that is *linear in value* is the fee itself — but weight ∝ fee is **regressive** (favours high-value/high-factor coins; the ValueWeighted mode, zero redistribution). The moment you invert the fee to get progressivity, you get a per-coin term and lose split-invariance. This is exactly the design-doc principle restated under CT:

> Every progressive term must key off cluster provenance, the only split-invariant signal in an anonymous-value system. Nothing may be weighted by UTXO count, account count, or any structure the holder controls for free. — `cluster-tilted-redistribution.md`

A per-coin inverse-fee weight *is* weighting by a structure the holder controls for free.

### 4.4 CT-compatibility & incremental leakage

| rule | CT-clean | reason | leakage beyond ADR-0006-public |
|------|:--------:|--------|--------------------------------|
| Uniform | yes | reads nothing about value (equal weight) | none |
| ClusterWeighted | **no** | weight ∝ hidden value → needs a ZK sort over amounts | N/A |
| FeeInverse | yes | reads only the public fee (already public under ADR 0006) | none beyond the already-public fee |
| CirculationGated | yes | reads only public creation age / circulation | none |
| Hybrid | yes | reads only public fee + public age | none beyond the already-public fee + age |

**Reading it — leakage is a non-issue for the value-free rules.** They key only on the fee and the creation age, both already public by ADR 0006. Keying the lottery on them adds **nothing** to the §9 metadata-leakage table — the fee already reveals its `value × factor` proxy whether or not the lottery reads it. So the value-free path is CT-clean *and* leakage-neutral. The problem is never leakage; it is Sybil resistance.

## 5. Recommendation

**Historical recommendation for the studied weights:** none recovers ClusterWeighted's split-invariance. Redistribution improved in the annual model; CT compatibility follows from the signals read. These findings neither establish production economic neutrality nor exclude every possible public-signal remedy.

The decision therefore reduces to a single, crisp question for the maintainer:

> **Is per-block lottery selection a place Botho is willing to accept a bounded Sybil subsidy, or must it stay strictly split-invariant?**

Three coherent paths follow:

### Path A — Accept a value-free rule + a *structural* Sybil brake (dissolves #902's lottery gadget)
Adopt **FeeInverse** (or Hybrid) and stop the splitting attack with something *other* than the weight formula, since the formula can't do it:
- **Eligibility floors, not weight tilts.** The existing lottery already gates on `min_utxo_age` (720 blocks) and `min_utxo_value` (1 microBTH). A 1,000-way split of a 100k-BTH whale still clears both — but a materially higher `min_utxo_value` **denominated against the public fee** (e.g. "eligible only if the coin's public fee ≥ F") caps how finely a position can be shattered into *eligible* tickets. This is value-free (keys on the public fee) and is the natural CT analogue of the min-value gate.
- **Per-winner caps.** The consensus lottery already caps payout at one block reward for anti-grinding. A per-UTXO or per-stealth-cluster payout cap bounds what any single fragmented position can extract per block regardless of ticket count.
- **What it gives up:** strict split-invariance. It bets that eligibility floors + payout caps keep the *realized* subsidy small even though the *weight* subsidy is large. **§7 evaluates that bet in a stationary proxy-cost model** — and finds it holds only *conditionally*: the realized subsidy is 0% on a busy, mature chain but positive (and unbounded by any window) when the chain is quiet or the reward is fat, the fee floor is counterproductive, and the payout cap is CT-evadable. Path A is viable only under the standing invariant `reward ≤ organic-circulation-cost`; see §7.5.

### Path B — Keep ClusterWeighted and pay for the ZK sort (the path the reframe was trying to avoid)
If split-invariance is non-negotiable — and §4.3 shows it is the *only* thing ClusterWeighted buys — then the value must be read in zero-knowledge, and the Merkle-sum-tree sort of #902 §3 is **unavoidable**. Rough scope of what that costs (for the spec author to size properly):
- **Tree:** a Merkle-sum tree over amount commitments (proof-of-liabilities style). Insertion/update on each UTXO create/spend is `O(log N)` hashes; the running root is consensus state.
- **Proof:** the seed picks a cumulative-value offset; the winner proves its leaf covers that offset (a Merkle path + a range/consistency proof that the committed subtree sums are correct). Proof size and verify cost are `O(log N)` group elements — concretely a few KB and a few ms per winner with Bulletproofs-class gadgets, ×4 winners/block.
- **Grinding:** the finalized-hash seed already caps grinding economically (one PoW solution shifts ≤ one reward). The sort adds a *prover* grinding surface — a winner could bias which offset it can cheaply prove — that must be closed by binding the offset to the seed *before* the prover acts. This is the delicate part and the reason #902 flags it explicitly.

### Path C — Retire the value tilt from the lottery entirely
Given §4.2 (ClusterWeighted routes pool back to whales in honest populations) and the fact that **demurrage + emission routing already do the stock-level redistribution**, the lottery's progressive *tilt* may be carrying less weight than assumed. A **Uniform** lottery is trivially CT-clean and its honest redistribution here (+0.1017 Δgini) beats ClusterWeighted — it fails *only* Sybil, identically to the others. If the lottery's job is reframed as "distribute the demurrage pool broadly" rather than "tilt progressively," the tilt (the thing that needs the ZK sort) can be dropped and the Sybil brake handled by eligibility floors (Path A). This is the cheapest path if the maintainer agrees the progressive tilt is redundant with demurrage.

**Bottom line for #902:** the value-free reframe **does dissolve the hardest-stated version of the lottery gadget** (the ZK weighted-sampling sort) — *if* Botho accepts Path A or Path C, i.e. a value-free rule whose Sybil resistance comes from eligibility floors + payout caps rather than from the weight formula. It does **not** let a value-free *weight* substitute for value-linear split-invariance; that is mathematically impossible from public signals. If strict weight-level split-invariance is required, the ZK sort (Path B) is unavoidable and should be scoped as above.

## 6. Open questions for maintainer ratification

1. **Split-invariance: hard requirement or soft?** Is a bounded, floor-limited Sybil subsidy on *lottery selection* acceptable, given demurrage/emission carry the load-bearing redistribution and the payout is already reward-capped per block? This single answer chooses between Path A/C and Path B.
2. **Is the lottery's progressive tilt still load-bearing?** §4.2 suggests ClusterWeighted under-redistributes in honest populations and only shines against gamed whales. If demurrage already delivers the Δgini floor (per `cluster-tilted-redistribution.md`), can the lottery drop the tilt (Path C) and keep only broad, Sybil-floored distribution?
3. **Eligibility-floor calibration (if Path A/C).** ~~What `min public fee` / per-position payout cap keeps the *realized* whale capture small under a 1,000-way split?~~ **Historically evaluated in §7, under its proxy-cost premises.** The expected-payout follow-up (`lottery_sybil_brake_sweep`) shows: a last-N window is capture-neutral (realized capture = `1 − √(ρ·base_fee/R)`, independent of N); a fee floor is regressive and inverts to a 100% whale subsidy past a threshold (drop it); a payout cap is CT-evadable via unlinkable clusters. The only real Sybil brake is the economic invariant `reward ≤ organic-circulation-cost`, which fails during bootstrap/quiet periods. Recommend N ≈ 10k blocks and **no** fee floor *if* Path A/C is chosen; see §7.5.
4. **ZK-sort budget (if Path B).** Is a few-KB, few-ms per-winner proof ×4/block acceptable at target block times, and can the offset be bound to the finalized seed tightly enough to kill prover grinding? This is the gadget #902 §3 asks the spec to specify.
5. **Consistency with the demurrage/tag gadgets.** #902 items 1 (demurrage inequality proof) and 2 (tag-blend proof) face the same "read hidden value" wall. If the epic adopts a committed-value machinery for those anyway, Path B's Merkle-sum tree may be nearly free to add — changing the cost-benefit against Path A/C. The three sub-problems should be sized together, not independently.

## 7. Structural Sybil brakes — do candidate-pool restrictions bound the *realized* subsidy?

§4.3 measured the Sybil attack as **ticket share in a fixed 100-holder population**, which overstates it: it freezes the denominator and reads the weight share, not the money the whale actually takes home. Open question 3 asked for the *realized-payout* follow-up: with a modeled organic candidate stream (an assumed economy creating fresh outputs every block), a **rolling** last-N-blocks window (so the whale must re-split forever, paying base fees each cycle), and three **structural brakes that are NOT weight tilts** — a last-N candidate window, a fee-denominated eligibility floor, and a per-block payout cap — does the *realized* whale capture stay small enough to make a value-free rule (Path A/C) viable?

**Reproduce:**
```
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim -- lottery-sybil-brake-sweep
```
Sim source: `cluster-tax/src/simulation/lottery_sybil_brake_sweep.rs` (expected values, no RNG; `SEED=0xB07A_0902_2` is documentary). The historical reproducibility claims and current reproduction are separated in the evidence record. This model charges recurring hypothetical per-output base fees; its honest track redistributes annual demurrage rather than applying the production 800-permille split. It reuses the shipped kernels — the real `demurrage_charge` (the public-fee-vs-age curve), the real `ClusterFactorCurve` (tier factors), `calculate_gini`, and the `public_fee_pico` proxy + constants from `lottery_selection_sweep`. The rule under test is **uniform weight over eligible candidates** — the representative value-free rule, since §4.3 showed every value-free rule shares the same per-ticket hole; the brakes, not the weight, are the variable here.

### 7.1 The organic stream turns "ticket share" into "realized capture"

Model a whale that fragments its 100k-BTH position and *continuously* refreshes the pieces to keep them inside the last-N window, competing against an organic stream of ρ fresh outputs/block. At stationarity the whale's profit-maximising realized capture (fraction of the *payout* it takes home over a long run, confirmed by a deterministic K-scan) is

```
s* = max(0, 1 − √(ρ · base_fee / R))
```

where **ρ** = organic outputs/block, **base_fee** = hypothetical per-output charge (0.25 BTH), not an enforced consensus floor, **R** = per-block lottery reward. The headline is what is *absent* from that formula: **the window size N.** The window changes the recency semantics and the whale's refresh cost, but *not* the capture ceiling.

| regime (ρ, R) | N=100 | N=1k | N=10k | N=100k | cost/captured | attack? |
|---|---:|---:|---:|---:|---:|:--:|
| busy chain, steady reward (ρ=20, R=3.8) | 0.00% | 0.00% | 0.00% | 0.00% | n/a | **deterred** |
| quiet chain, steady reward (ρ=2, R=3.8) | 64.5% | 63.6% | 62.8% | 64.6% | 0.35–0.37× | pays ~2.8× |
| busy chain, fat reward (ρ=20, R=20) | 49.8% | 48.9% | 50.8% | 49.9% | 0.49–0.51× | pays ~2× |

**Reading it — the brake is the economy, not the window.** The gate is the ratio `ρ·base_fee / R`, the honest economy's own per-block circulation cost against the reward:

- **Within this stationary model, when `ρ·base_fee ≥ R`, the attack is unprofitable at every tested window size** and the whale does not split at all (K\*=0, 0% capture). On a *busy* chain (ρ=20 ⇒ ρ·base_fee=5 BTH) with the #351 steady reward (~3.8 BTH/block), the modeled uniform lottery is **non-profitable to split under that cost premise** — no ZK sort, no floor, no cap. The organic circulation the whale must out-ticket is simply too cheap for it and too expensive to fake.
- **When the chain is quiet or the reward is fat, no window saves it.** A quiet chain (ρ=2, thin denominator) lets the whale capture ~63% and the attack pays for itself ~2.8× (it burns 0.35 BTH in fees per 1 BTH of pool it wins). A *bootstrap* chain with fat early emission (R=20) hands it ~50% even at ρ=20. These are exactly the adversary-favourable regimes — low activity, or high reward — and the window is powerless in all of them.

The window's real contribution is therefore **not** a capture bound: it is (i) the *recency* semantic that makes "circulated" meaningful, and (ii) forcing the attack to be a *recurring* cost rather than a one-time `(K−1)·base_fee`. Both are worth having, but neither is the Sybil bound the reframe needed.

### 7.2 The fee floor is regressive and *non-monotone* — it can invert into a whale subsidy

A fee-denominated floor (`eligible only if public fee ≥ F`) is the CT analogue of `min_utxo_value`. But under CT a **fresh** coin has fee ≈ base_fee *regardless of its hidden value* — demurrage has not accrued — so a floor above base can only be cleared by an **aged** coin, in direct tension with a *recency* window. The aging tax (blocks to clear F, whale factor):

| floor F (BTH) | 100-BTH coin | 1k | 10k | 100k |
|---:|---:|---:|---:|---:|
| 0.25 (≈base) | 0 | 0 | 0 | 0 |
| 0.50 | 830,770 | 83,077 | 8,308 | 831 |
| 1.00 | 2,492,308 | 249,231 | 24,924 | 2,493 |

At a 10k-block window (~14 h), a 100-BTH coin needs **830k blocks** to clear a 0.50-BTH floor — it can *never* be both fresh-enough and fee-heavy-enough. So the floor excludes the organic poor first. Worse, it is non-monotone (quiet regime, N=10k):

| floor F (BTH) | whale K\* | realized capture |
|---:|---:|---:|
| 0.00 | 33,733 | 62.8% |
| 0.50 | 11 | 8.6% |
| 1.00 | 1 | **100.0%** |

F=0.50 does cap the whale (large pieces ⇒ 11 tickets ⇒ 8.6%) — but only by disenfranchising every organic tier that cannot clear it. Pushed to F=1.00, **no in-window coin clears the floor except the whale's own large, aged, consolidated position**, which then wins 100% of a pool nobody else can enter. A fee floor under a recency window does not bound the whale; past a threshold it *hands the whale everything*. It is disqualified.

### 7.3 The payout cap bounds capture — but is CT-evadable

A per-block per-owner payout cap directly clamps realized capture to the cap (quiet regime, N=10k): 25% cap → 25.0%, 10% → 10.0%, 5% → 5.0%. This is the one brake that *does* bound capture on paper. Its fatal flaw is CT-specific: the cap must be keyed to an **unforgeable identity**, and under confidential amounts the whale's fresh coins share no tags, so they form **distinct, unlinkable stealth clusters**. A per-cluster cap is evaded by splitting across clusters — the very same "structure the holder controls for free" wall the *weight* hit in §4.4. The cap is a real bound only against an attacker who cannot manufacture identities, which is not the CT threat model. It is a best-case ceiling, not a guarantee.

### 7.4 Honest redistribution *survives* the window (indeed improves)

In the historical 10-year annual collect-and-redistribute model, restricting eligibility to *circulated* coins gives the following result over the §4.1 population:

| policy | final Gini | Δgini |
|---|---:|---:|
| Uniform (all holders) | 0.6508 | +0.1017 |
| Windowed (circulated only) | 0.6381 | **+0.1144** |

The window *concentrates* the pool on actively-circulating holders — here the poor commerce tier, exactly the intended beneficiaries — while idle whales self-exclude by not transacting. In this modeled population, windowed redistribution is **at least as equalizing** as the unrestricted lottery; that is not a measured current production Gini effect. The only casualty is the idle *poor* holder, also excluded; that is acceptable precisely because demurrage already redistributes the stock (per `cluster-tilted-redistribution.md`), so the lottery is free to reward circulation.

### 7.5 Verdict — do the structural brakes rescue the value-free path?

**Historical model verdict: partly and conditionally. Current production economics needs production-mode re-run.** The expected-capture lens deflates the §4.3 scare: the "always ~91% ticket share" becomes **0% on a busy steady chain** and a *bounded, self-limiting* subsidy even when positive. And honest redistribution is unharmed (improved) by a last-N window. That is real progress for Path A/C. But the bound that does the work is **economic** (`ρ·base_fee ≥ R`), and it degrades exactly in the adversary-favourable regimes:

- The **last-N window** is *capture-neutral* — it fixes recency and imposes a recurring cost, but does not bound the subsidy.
- The **fee floor** is *counterproductive* — regressive, and past a threshold it inverts to a 100% whale subsidy. Drop it.
- The **payout cap** is *CT-evadable* — a real bound only against an attacker who cannot mint unlinkable clusters, which CT lets it do.
- The residual Sybil safety therefore rests entirely on the standing invariant **reward ≤ organic circulation cost** (`R ≤ ρ·base_fee`). That holds on a busy, mature chain at the #351 steady reward, but **fails during bootstrap** (fat early emission) and **whenever activity thins** (which an adversary can help along by quieting the chain).

**Historical recommendation under the assumed recurring ticket charge:** a value-free rule (Path A/C) with a **last-N-blocks window (recommend N ≈ 10k blocks, ~14 h — a natural "recently circulated" horizon), no fee floor, and the base-fee floor kept as the only value gate** is viable **iff** the protocol is willing to bound the per-block lottery reward against a floor of organic activity (`R ≤ ρ·base_fee`) and to accept a bounded residual subsidy in quiet/bootstrap regimes. If instead a **hard, regime-independent** Sybil bound is required — safe even when the chain is quiet or the reward is fat — then the structural brakes do **not** deliver it, and **Path B (the ZK weighted-sampling sort) stands** as the only mechanism that bounds capture independent of activity and reward. The follow-up does not overturn §5's core result (no value-free *weight* is split-invariant); it maps precisely which *deployments* can live with that and which still need the sort.

## 8. What this deliverable is (and isn't)

This is the **design-research + calibration simulation** for the lottery sub-problem, giving the eventual `anvil:spec` authoring a concrete starting point: which rule, which properties it recovers, and the crisp Path A/B/C decision it surfaces. It does **not** change any consensus code, and it does **not** by itself pick the rule — that is a maintainer ratification (and, for Path A/C, needs the realized-payout follow-up sim of open question 3). The first three sweeps are committed deterministic expected-value models, not seeded draw simulations. Their numbers regenerate under the stated populations and fees; §9.1.1 alone adds candidate-window sampling. No historical model is sufficient evidence for current quantitative production economics.

## 9. Path C ratification gate — the endogenous reward-cap (`R = min(fee_pool, ρ·base_fee)`)

§5–§7 left the value-free path (A/C) viable only *conditionally*: the residual Sybil safety rested on the **external** invariant `R ≤ ρ·base_fee` (reward ≤ organic circulation cost), which §7.5 flagged as failing during bootstrap/quiet regimes. The historical **Path C model** tried to remove the "external" caveat by defining its per-block lottery reward as

```
R = min(actual_fee_pool, ρ · base_fee)
```

The historical design prose called **ρ** the last-N-window output count. The algebra and sweep below instead use a **per-block output arrival rate**, including whale splits. That rate is not the implemented eligible-window count. Combined with a hypothetical charge for every created ticket, it yields a conditional model bound; it is not a production neutrality guarantee. This section preserves the original cap and throughput experiments; §9.1.1 records the violated premises.

**Reproduce:**
```
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim -- lottery-reward-cap-sweep
```
Sim source: `cluster-tax/src/simulation/lottery_reward_cap_sweep.rs` (expected values, no RNG; `SEED=0xB07A_0902_3` is documentary). This is rate-based cap algebra and annual-lump accounting, with a hypothetical per-output charge; it does not implement the production 800-permille fee split. Reuses the shipped kernels — the real `demurrage_charge` (the value×factor×time fee curve that sets the lump sizes), the real `ClusterFactorCurve` (tier factors), the shared `calculate_gini`, and the §4.1 honest population. That command regenerates §9.1 and §9.2 tables; the inserted §9.1.1 tables require `--faithful`.

### 9.1 Historical claim 1 — net-zero under rate and per-ticket-cost premises

A whale creates `k` outputs/block and refreshes them to stay inside the last-N window, competing against `ρ_o` organic outputs/block under a uniform draw. Its stationary ticket share is `k/(ρ_o+k)` (both whale and organic coins live N blocks, so the window fraction equals the creation-rate fraction — **N cancels**). With the cap binding (`pool ≫ cap`), `R = (ρ_o+k)·base_fee`:

| ρ_o | k | share | R cap (BTH) | winnings/blk | cost/blk | **net/blk** | net if uncapped | net if organic-only cap |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 2 | 1 | 33.333% | 0.750 | 0.2500 | 0.2500 | +0.00e0 | +6.4167 | -0.0833 |
| 2 | 5 | 71.429% | 1.750 | 1.2500 | 1.2500 | +0.00e0 | +13.0357 | -0.8929 |
| 2 | 50 | 96.154% | 13.000 | 12.5000 | 12.5000 | +0.00e0 | +6.7308 | -12.0192 |
| 2 | 500 | 99.602% | 125.500 | 125.0000 | 125.0000 | +0.00e0 | -105.0797 | -124.5020 |
| 2 | 5000 | 99.960% | 1250.500 | 1250.0000 | 1250.0000 | +0.00e0 | -1230.0080 | -1249.5002 |
| 20 | 1 | 4.762% | 5.250 | 0.2500 | 0.2500 | +0.00e0 | +0.7024 | -0.0119 |
| 20 | 5 | 20.000% | 6.250 | 1.2500 | 1.2500 | +0.00e0 | +2.7500 | -0.2500 |
| 20 | 50 | 71.429% | 17.500 | 12.5000 | 12.5000 | +0.00e0 | +1.7857 | -8.9286 |
| 20 | 500 | 96.154% | 130.000 | 125.0000 | 125.0000 | +0.00e0 | -105.7692 | -120.1923 |
| 20 | 5000 | 99.602% | 1255.000 | 1250.0000 | 1250.0000 | +0.00e0 | -1230.0797 | -1245.0199 |
| 200 | 1 | 0.498% | 50.250 | 0.2500 | 0.2500 | +0.00e0 | -0.1505 | -0.0012 |
| 200 | 5 | 2.439% | 51.250 | 1.2500 | 1.2500 | +0.00e0 | -0.7622 | -0.0305 |
| 200 | 50 | 20.000% | 62.500 | 12.5000 | 12.5000 | +0.00e0 | -8.5000 | -2.5000 |
| 200 | 500 | 71.429% | 175.000 | 125.0000 | 125.0000 | +0.00e0 | -110.7143 | -89.2857 |
| 200 | 5000 | 96.154% | 1300.000 | 1250.0000 | 1250.0000 | +0.00e0 | -1230.7692 | -1201.9231 |

**Reading it — Claim 1 holds in this model.** The **net/blk** column is 0 (to floating-point epsilon) for *every* `(ρ_o, k)`: the whale breaks precisely even at any split factor. The mechanism is algebraic — winnings `= k/(ρ_o+k) · (ρ_o+k)·base = k·base = cost`. Splitting inflates ρ and thus raises the cap `R` by `k·base`, but the whale collects only its share `k/(ρ_o+k)` of that increase, which is exactly its own added fee outlay. The threat-model subtlety — that consensus is *forced* to count the whale's own outputs in ρ — gives the conditional cancellation in this model: the whale cannot inflate the cap without proportionally inflating the denominator it must win against. The *uncapped* counterfactual column is positive at the whale's optimal (modest) split (it would profit from a fat 20-BTH pool absent the cap), confirming the cap is load-bearing; the *organic-only-cap* column is ≤ 0 everywhere, showing that counting-all is not a loophole but the tightest (break-even) choice.

The net is **window-invariant** (ρ_o=20, k=500) — N sets recency and refresh cost, not the capture ledger:

| N (blocks) | share | R cap (BTH) | net/blk (BTH) |
|---:|---:|---:|---:|
| 1000 | 96.154% | 130.000 | +0.00e0 |
| 10000 | 96.154% | 130.000 | +0.00e0 |
| 100000 | 96.154% | 130.000 | +0.00e0 |

Under these rate/cost premises, `net ≤ 0` is the algebraic bound, with `net = 0` its ceiling: when the actual fee pool is *below* the cap, `R = pool` and the whale goes strictly negative (ρ_o=20, k=500, cap=130 BTH):

| fee pool (BTH) | R = min(pool,cap) | winnings/blk | cost/blk | net/blk (BTH) |
|---:|---:|---:|---:|---:|
| 32.5 | 32.50 | 31.250 | 125.000 | -93.7500 |
| 65.0 | 65.00 | 62.500 | 125.000 | -62.5000 |
| 130.0 | 130.00 | 125.000 | 125.000 | +0.0000 |
| 520.0 | 130.00 | 125.000 | 125.000 | +0.0000 |

**Claim 1 verdict: CONFIRMED for the model above, REFUTED for the implemented rules (#1682, §9.1.1).** Within its premises (per-block `ρ`, every split output pays `base_fee`), the reward-cap makes splitting net-zero at best and net-negative whenever the pool is thin. The protocol-7.0.0 source audited in #1761 met neither premise. Its stationary expected-value/candidate-window study found profitability at every grid point (§9.1.1); this is historical model evidence, not stateful production execution.

### 9.1.1 Historical faithful study (#1682 / #1761): violated premises and modeled profitability

**Reproduce:**
```
cargo run --release -p bth-cluster-tax --features cli --bin cluster-tax-sim -- lottery-reward-cap-sweep --faithful
cargo test --release -p bth-cluster-tax --lib lottery_reward_cap_faithful
cargo test -p botho --lib faithful_sweep_parity
```
Sim source: `cluster-tax/src/simulation/lottery_reward_cap_faithful.rs`. It uses stationary expected-value economics plus candidate-window Monte Carlo for synthetic key ordering and offsets. The mixed SplitMix64 seed is `0x1682_FA17_0902 ^ ((rho_o as u64) << 32) ^ (k as u64)`, with 400 offsets for each of four settings. `Rules::CurrentCode` is an approximation, not a call to the production block transition; every other `Rules` variant is an unimplemented candidate remedy. Current output reproduction is separately attributed in the evidence record. It calls the shipped kernels directly: `consensus_fee_breakdown` (the pure half of `Ledger::consensus_fee_floor`, golden-pinned to the ledger by #1695), `sybil_reward_cap`, `LotteryDrawConfig::default()` (what `ledger::lottery_v2::canonical_config` uses), and `MonetaryPolicy::lottery_emission_share` / `calculate_tail_reward_u128`. It mirrors `consensus::lottery::reward_cap`, `compute_pool_accounting`, `MAX_LOTTERY_CANDIDATES`, the 80% pool share and the mainnet schedule. A shared golden table pins the mirror to the node: `faithful_sweep_parity_*` in `botho/src/ledger/lottery_v2.rs` runs it against the real functions. Analysis only: this changes no consensus code.

**Premises violated in the historical protocol-7.0.0 audit.** The table preserves that record; derived shares and pool claims are stationary-model results. Its “never binds” observation concerns the studied saturated 10,000-candidate sets and block rewards, not every production candidate set.

| §9.1 premise | Implementation (protocol 7.0.0) | Effect |
|---|---|---|
| `ρ` = outputs created **per block** | `ρ` = `count_eligible` over the seeded-offset candidate set: every output aged 720..=10,000 blocks, truncated at `MAX_LOTTERY_CANDIDATES` = 10,000 (`ledger/store/validation.rs`). Above about 1.08 eligible outputs/block the count is always 10,000. | The Sybil cap is a constant 10,000 × 0.25 = 2,500 BTH and never binds. The live clamp is the anti-grinding `block_reward`, so `R = min(avail, block_reward)`. Splitting no longer raises the cap in step with the whale's cost. |
| each split output pays `base_fee` = 0.25 BTH | No rule charges `LOTTERY_BASE_FEE_PICO`; it appears only in the cap. The consensus floor is 1 pico/byte × size × factor × `min(n_out,10)²` plus demurrage: **870–117,032 pico per ticket**. The relay `MIN_TX_FEE` adds `1e8/n` pico per ticket (6.25e6 at n = 16), but it is enforced in the mempool only (#1684). | A ticket costs 2.1e6× to 2.9e8× less than modelled with the floor alone, and about 2,500× to 40,000× less with `MIN_TX_FEE`. |
| the only tickets are paid-for outputs | Coinbase and the 4 payout outputs per block are eligible tickets that cost nothing (#1690). | The whale's stationary share is `k/(ρ_o+k+1)`: it wins its share of the payout tickets back as new tickets. |
| `R` = the fee pool | `R = min(carry-forward + emission share + 0.8·fees, cap)`. The emission share is 0% in epoch 0 and rises 10 points per epoch to 50%. The backlog drains at the full block reward (#1691). | From epoch 1 on, `R` is mostly emission, which no fee pays for, so the "net-negative when the pool is thin" branch never occurs. |
| — | 80% of the whale's own fees return to a pool it mostly owns. A ticket stays live for L = 9,281 blocks, not N. | Both are modelled. |

**Model.** These are stationary expected values, not stateful payout transitions. Whale fee modes are `ConsensusOnly` and `ConsensusPlusRelayMin`; organic transaction fees always use `ConsensusPlusRelayMin`. The modeled pool receives 800 permille of fees. Canonical draw inputs are Uniform, four winners, minimum age 720, minimum value 1,000,000 pico and a 10,000-block circulation window. Every eligible output stays in the eligible set for L = 9,281 blocks, so window shares equal creation-rate shares.
- The organic economy creates `ρ_o` eligible outputs per block in 2-output txs, each tx paying the wallet fee `max(floor, MIN_TX_FEE)` = 1e8 pico (the x1 run observed 1e8–4e8 per tx).
- The whale creates `k` tickets per block, each worth `min_utxo_value`. It mints them by spending an aged-out ticket into an `n`-output tx (`n` = 1..=16, chosen per `k` to minimise cost) at cluster factor 1× or 6×. The fee mode is either the consensus floor only (what a whale-miner or bribed proposer pays) or the floor plus `MIN_TX_FEE` (what a default node relays).
- Capacity is either the default proposer slot (`max_txs_per_slot` = 100 minus the minting tx, × 16 outputs = 1,584 tickets per block) or the 20 MiB block-size bound. Neither the slot limit nor the 16-output cap was a validity rule when this was written; the 16-output cap is one since protocol 10.0.0 (#1684), the slot limit is still not.
- Two choices understate the whale: organic demurrage is left out of the pool, and the tail reward ignores burns.

The grid is epochs {0, 1, 3, tail} × backlog {0, 1000 BTH} × `ρ_o` ∈ {2, 20, 200} × fee mode × factor {1×, 6×} × capacity, which is 192 points.

| epoch | block reward (BTH) | lottery emission share (BTH/blk) |
|---|---:|---:|
| 0 | 50.0000 | 0.0000 |
| 1 | 25.0000 | 2.5000 |
| 3 | 6.2500 | 1.8750 |
| tail | 2.4219 | 1.2109 |

**Per-ticket cost under the implemented floor** (epoch 1):

Fee of one split tx (1 input, n outputs of min_utxo_value) and the cost per new eligible output, epoch 1, vs the modelled base_fee of 2.5e11 pico.

| n outputs | factor | floor fee/tx (pico) | floor per ticket | floor+MIN_TX_FEE per ticket | base_fee ÷ cheapest |
|---:|---:|---:|---:|---:|---:|
| 1 | 1× | 870 | 870 | 1.000e8 | 2.9e8 |
| 1 | 6× | 5251 | 5251 | 1.000e8 | 4.8e7 |
| 2 | 1× | 3960 | 1980 | 5.000e7 | 1.3e8 |
| 2 | 6× | 23823 | 11912 | 5.000e7 | 2.1e7 |
| 4 | 1× | 19680 | 4920 | 2.500e7 | 5.1e7 |
| 4 | 6× | 118206 | 29552 | 2.500e7 | 8.5e6 |
| 8 | 1× | 109440 | 13680 | 1.250e7 | 1.8e7 |
| 8 | 6× | 656893 | 82112 | 1.250e7 | 3.0e6 |
| 10 | 1× | 195000 | 19500 | 1.000e7 | 1.3e7 |
| 10 | 6× | 1170317 | 117032 | 1.000e7 | 2.1e6 |
| 16 | 1× | 267000 | 16688 | 6.250e6 | 1.5e7 |
| 16 | 6× | 1602507 | 100157 | 6.250e6 | 2.5e6 |

**Whale net vs k**, historical `Rules::CurrentCode` approximation at the reference point:

| k (out/blk) | n/tx | share | cap (BTH) | R (BTH) | winnings/blk | cost/blk | **net/blk** | net/day |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 16 | 4.55% | 25.0 | 2.5008 | 0.1137 | 0.000006 | +0.1137 | +1964 |
| 10 | 16 | 32.26% | 25.0 | 2.5008 | 0.8067 | 0.000063 | +0.8067 | +13939 |
| 100 | 16 | 82.64% | 25.0 | 2.5013 | 2.0672 | 0.000625 | +2.0666 | +35710 |
| 200 | 16 | 90.50% | 25.0 | 2.5018 | 2.2641 | 0.001250 | +2.2628 | +39102 |
| 1000 | 16 | 97.94% | 25.0 | 2.5058 | 2.4543 | 0.006250 | +2.4480 | +42302 |
| 1584 | 16 | 98.69% | 25.0 | 2.5087 | 2.4759 | 0.009900 | +2.4660 | +42612 |

**Historical CurrentCode model: the whale's optimum over the grid.**

R0 = payout/blk with no whale. c* = break-even per-ticket cost (splitting is profitable iff the per-ticket cost is below c*). c = the whale's actual cheapest per-ticket cost at k*. Columns at the realistic-expensive end (floor+MIN_TX_FEE, 6×, proposer slot); the last column is the cheapest end (floor only, 1×, block-size bound).

| epoch | backlog | ρ_o | R0 (BTH) | c* (BTH/ticket) | c (BTH/ticket) | k* | share* | **net*/blk** | net*/day | net*/blk (floor, 1×, block-size) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 2 | 0.0001 | 2.500e-5 | 6.25e-6 | 9 | 75.0% | +0.0000 | +1 | +0.0001 |
| 0 | 0 | 20 | 0.0008 | 3.774e-5 | 6.25e-6 | 89 | 80.9% | +0.0005 | +8 | +0.0008 |
| 0 | 0 | 200 | 0.0080 | 3.976e-5 | 6.25e-6 | 862 | 81.1% | +0.0046 | +79 | +0.0079 |
| 0 | 1000 | 2 | 50.0000 | 1.250e1 | 6.25e-6 | 1584 | 99.8% | +49.8956 | +862196 | +49.9975 |
| 0 | 1000 | 20 | 50.0000 | 2.273e0 | 6.25e-6 | 1584 | 98.7% | +49.3359 | +852524 | +49.9895 |
| 0 | 1000 | 200 | 50.0000 | 2.475e-1 | 6.25e-6 | 1584 | 88.7% | +44.3598 | +766538 | +49.9181 |
| 1 | 0 | 2 | 2.5001 | 7.813e-1 | 6.25e-6 | 1584 | 99.8% | +2.4934 | +43085 | +2.4999 |
| 1 | 0 | 20 | 2.5008 | 1.180e-1 | 6.25e-6 | 1584 | 98.7% | +2.4660 | +42612 | +2.5000 |
| 1 | 0 | 200 | 2.5080 | 1.247e-2 | 6.25e-6 | 1584 | 88.7% | +2.2227 | +38409 | +2.5036 |
| 1 | 1000 | 2 | 25.0000 | 6.250e0 | 6.25e-6 | 1584 | 99.8% | +24.9428 | +431012 | +24.9986 |
| 1 | 1000 | 20 | 25.0000 | 1.136e0 | 6.25e-6 | 1584 | 98.7% | +24.6630 | +426177 | +24.9937 |
| 1 | 1000 | 200 | 25.0000 | 1.238e-1 | 6.25e-6 | 1584 | 88.7% | +22.1750 | +383184 | +24.9580 |
| 3 | 0 | 2 | 1.8751 | 5.860e-1 | 6.25e-6 | 1584 | 99.8% | +1.8695 | +32306 | +1.8749 |
| 3 | 0 | 20 | 1.8758 | 8.848e-2 | 6.25e-6 | 1584 | 98.7% | +1.8492 | +31954 | +1.8752 |
| 3 | 0 | 200 | 1.8830 | 9.359e-3 | 6.25e-6 | 1584 | 88.7% | +1.6681 | +28825 | +1.8796 |
| 3 | 1000 | 2 | 6.2500 | 1.562e0 | 6.25e-6 | 1584 | 99.8% | +6.2283 | +107625 | +6.2495 |
| 3 | 1000 | 20 | 6.2500 | 2.841e-1 | 6.25e-6 | 1584 | 98.7% | +6.1583 | +106416 | +6.2478 |
| 3 | 1000 | 200 | 6.2500 | 3.094e-2 | 6.25e-6 | 1584 | 88.7% | +5.5363 | +95668 | +6.2379 |
| tail | 0 | 2 | 1.2110 | 3.784e-1 | 6.25e-6 | 1584 | 99.8% | +1.2067 | +20852 | +1.2109 |
| tail | 0 | 20 | 1.2117 | 5.716e-2 | 6.25e-6 | 1584 | 98.7% | +1.1938 | +20629 | +1.2113 |
| tail | 0 | 200 | 1.2189 | 6.058e-3 | 6.25e-6 | 1584 | 88.7% | +1.0788 | +18642 | +1.2166 |
| tail | 1000 | 2 | 2.4219 | 6.055e-1 | 6.25e-6 | 1088 | 99.7% | +2.4084 | +41617 | +2.4216 |
| tail | 1000 | 20 | 2.4219 | 1.101e-1 | 6.25e-6 | 1584 | 98.7% | +2.3803 | +41131 | +2.4208 |
| tail | 1000 | 200 | 2.4219 | 1.199e-2 | 6.25e-6 | 1584 | 88.7% | +2.1393 | +36966 | +2.4159 |

**Candidate window.** Does splitting into more than 10,000 outputs interact with `MAX_LOTTERY_CANDIDATES`? Not in the whale's favour or against it. Below is seeded Monte Carlo of synthetic key ordering and candidate-window offsets modeled after the node's walk; it does not discover a ledger candidate set. UTXO ids are `tx_hash ‖ index`, so a split tx's outputs sit next to each other in key order. The walk starts at a seeded offset and takes the first 10,000 eligible outputs:

Monte Carlo of the node's walk (seed 0x1682fa170902, 400 offsets each): ids are `tx_hash ‖ index`, so a split tx's outputs sit together in key order.

| ρ_o | k | n/tx | eligible set | window count | global share | mean window share | sd (window) | sd (binomial) |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 20 | 20 | 16 | 371236 | 10000 | 49.999% | 49.950% | 1.470% | 0.500% |
| 20 | 200 | 16 | 2041828 | 10000 | 90.909% | 90.910% | 0.468% | 0.287% |
| 20 | 200 | 1 | 2041820 | 10000 | 90.909% | 90.924% | 0.389% | 0.287% |
| 200 | 200 | 16 | 3712408 | 10000 | 50.000% | 50.002% | 1.449% | 0.500% |

**Reading it.**
1. **Splitting is profitable at 192 of 192 historical expected-value grid points.** This confirms the issue's rough consequence. At the reference point (epoch 1, `ρ_o` = 20, floor + `MIN_TX_FEE`, 6×, proposer slot), the whale fills the slot (k\* = 1,584 tickets per block), takes 98.7% of the tickets, and nets **+2.47 BTH/block, about +42,600 BTH/day**. Honest holders would otherwise receive 2.50 BTH/block, so the whale captures almost all of it. The issue's example of k = 200 nets +2.26 BTH/block here. That is close to its +3.3 estimate; the difference is that R is 2.5 BTH at epoch 1, not 3.8.
2. **In this model, the worst regime is a carry-forward backlog draining.** The payout is then the full block reward, so the whale nets up to +49.9 BTH/block in epoch 0 and +24.9 in epoch 1 for as long as the backlog lasts (#1691).
3. **The epoch-0 steady state is small.** With no backlog, R is only organic fees, so the whale nets ≤ 0.008 BTH/block. It is still positive at every grid point.
4. **Break-even.** Splitting is profitable whenever the per-ticket cost is below c\*, where R0 is the payout with no whale and k = 1 is the smallest split. When the payout does not move with k (a backlog drain, clamped at the block reward), **c\* = R0/(ρ_o+2)**. Without a backlog, 80% of the whale's own fees return to the pool, and c\* = R0/(ρ_o+1.2). At the reference point c\* = 2.5008/21.2 = 0.118 BTH per ticket. Both closed forms are proven about the stated economy model in `formal/Botho/Lottery/Sybil.lean` (`split_never_pays_iff`, `break_even_reference`; see `formal/lottery.md`). The actual cost is 6.25e-6 BTH with `MIN_TX_FEE`, about 19,000× below c\*, and 870 pico (8.7e-10 BTH) with the floor alone at 1×. At every grid point the actual cost is far below c\*.
5. **The synthetic candidate-window sample gives no mean-share protection in the studied settings.**
   - The whale's mean share inside the 10,000-candidate window equals its global share.
   - Contiguous split outputs make the window share vary about 3× more from block to block (sd 1.47% vs 0.50% binomial), but the mean does not move.
   - Because `count_eligible` saturates at 10,000, the Sybil cap is a constant 2,500 BTH, more than the largest block reward. In the studied saturated 10,000-candidate scenarios, `sybil_reward_cap` does not bind below the block-reward clamp. It can bind for smaller eligible sets; this is not a general production claim.
   - The unit test `test_reward_cap_makes_splitting_net_zero` (`consensus/lottery.rs`) passes only because it assumes both premises (`block_reward = u64::MAX`, cost = k·base_fee). It says nothing about the implemented rules.

**Remedy options (historical model results; none implemented — a consensus decision for the maintainer).** No row establishes current production profitability or a deployed fix; those conclusions need production-mode re-run.

Whale optimum over all 192 grid points (4 epochs × 2 backlogs × 3 ρ_o × 2 fee modes × 2 factors × 2 capacities). Reference = epoch 1, backlog 0, ρ_o 20, floor+MIN_TX_FEE, 6×, proposer slot (1,584 out/blk).

| rule set | profitable points | max net*/blk (BTH) | max net*/day | reference net*/blk | honest R0 at reference (BTH/blk) | honest regimes with inflow > cap | worst point |
|---|---:|---:|---:|---:|---:|---:|---|
| current code | 192/192 | +49.9975 | +863957 | +2.4660 | 2.5008 | 0/12 | epoch 0, backlog 1000, ρ_o 2, floor, 1×, block size (20 MiB) (k*=76565) |
| (a) per-block ρ in cap | 192/192 | +49.9975 | +863957 | +2.4660 | 2.5008 | 2/12 | epoch 0, backlog 1000, ρ_o 2, floor, 1×, block size (20 MiB) (k*=76565) |
| (a') ρ = window count / L (10k truncation kept) | 192/192 | +0.2693 | +4654 | +0.2576 | 0.2694 | 9/12 | epoch 0, backlog 1000, ρ_o 2, floor, 1×, block size (20 MiB) (k*=23873) |
| (b) base_fee per eligible output | 96/192 | +38.5000 | +665280 | +0.2421 | 6.5008 | 4/12 | epoch 0, backlog 1000, ρ_o 2, floor, 1×, proposer slot (1,584 out/blk) (k*=21) |
| (a)+(b), free tickets kept | 96/192 | +0.9844 | +17010 | +0.2421 | 6.2500 | 7/12 | epoch 0, backlog 1000, ρ_o 2, floor, 1×, block size (20 MiB) (k*=189) |
| (a)+(b)+free-ticket rule | 0/192 | +0.0000 | +0 | +0.0000 | 5.0000 | 9/12 | — |
| min_utxo_value = 1 BTH | 192/192 | +49.9975 | +863957 | +2.4575 | 2.5008 | 0/12 | epoch 0, backlog 1000, ρ_o 2, floor, 1×, block size (20 MiB) (k*=76565) |

How to read the remedy table: all economic outcomes below are conditional on this stationary model, free-ticket fixed point and stated capacity/backlog assumptions. The table labels are preserved from the historical output.
- **"Honest regimes with inflow > cap"** counts the 12 honest regimes (4 epochs × 3 `ρ_o`, no whale, no backlog) in which the per-block pool inflow (emission share + 80% of fees) exceeds the cap. In those regimes the carry-forward backlog grows without bound and honest redistribution is throttled to the cap. This is the §9.2 stability condition.
- **(a) per-block `ρ`, alone**, changes nothing measurable. The cap `(ρ_o+k+5)·base_fee` grows with the whale's k for free, so it reaches `block_reward` at the whale's optimum. Profit is the same as current code at every point.
- **(a') window count / L with the 10k truncation kept** pins `ρ` at about 1.08, so the cap is about 0.27 BTH. The whale still captures it at a profit, and honest redistribution is throttled in 9 of 12 regimes. This shows that any per-block `ρ` must be computed without the `MAX_LOTTERY_CANDIDATES` truncation.
- **(b) `base_fee` per eligible output, alone**, removes the profit wherever R0 < 0.25·(ρ_o+2), which is the c\* condition. It leaves 96 of 192 points profitable, worst +38.5 BTH/block during an epoch-0 backlog drain. It is also a large fee increase for every user: 0.25 BTH per output. <!-- stale-literal-ok: hypothetical remedy (b), not the shipped fee -->
- **(a)+(b) with free tickets still eligible** still leaks up to +0.98 BTH/block. The 5 free tickets per block inflate the cap by 1.25 BTH that nobody paid for.
- **(a)+(b)+free-ticket rule** is the only option with net\* ≤ 0 at all 192 points. Here coinbase and payout outputs are neither eligible nor counted. It restores the §9.1 algebra exactly: `s·R ≤ k/(ρ_o+k)·(ρ_o+k)·base = k·base ≤ cost`. The CI test `combined_remedy_is_non_positive_everywhere` asserts it. **Its cost:** it throttles honest redistribution in 9 of 12 regimes. The cap `ρ_o·base_fee` (0.5 BTH at `ρ_o` = 2) is below the emission share, and organic base fees alone put 0.8·ρ_o·base into the pool. So the excess needs a separate channel that cannot be gamed by splitting: burn it, route it back to the miner, or send it to a per-capita rebate. Carry-forward would grow without bound.
- **Raising `min_utxo_value` to 1 BTH** has no effect. The capital is locked, not spent, and demurrage on a ticket is zero at 1× and negligible at 6× over 9,281 blocks.
- **Age weighting** was analysed but not simulated. A whale ticket lives as long in the window as an organic one, so weighting by age leaves the whale's share unchanged. If organic outputs are spent sooner, it tilts toward the patient whale.



### 9.2 Claim 2 — does the cap throttle redistribution into irrelevance?

In the historical rate/cost model, the cap clamps payout at `ρ·base_fee` — a few BTH/block. Production directs 80% of fees to its pool, but this section's annual-lump experiment does not execute that split. Its modeled revenue is dominated by large **demurrage** fees (`fee ∝ value×factor×time`) from wealthy spends, which arrive in rare, huge lumps: a single wealthy spend's demurrage fee is hundreds–thousands of BTH, hundreds of times the per-block cap. So when `actual_fee_pool > ρ·base_fee`, what happens to the excess?

**Throughput** (§4.1 population; each tier's annual demurrage realised in realistic lumps — whale ≈ 1 big spend/yr, retail ≈ weekly; total revenue 8121 BTH/yr, mean inflow 0.0013 BTH/block):

| ρ (outputs/blk) | cap (BTH/blk) | excess policy | through (BTH/yr) | **throughput** | mean inflow/blk (BTH) |
|---:|---:|---|---:|---:|---:|
| 2 | 0.50 | burn excess | 107.9 | 1.33% | 0.0013 |
| 2 | 0.50 | carry-forward | 8121.4 | 100.00% | 0.0013 |
| 20 | 5.00 | burn excess | 220.4 | 2.71% | 0.0013 |
| 20 | 5.00 | carry-forward | 8121.4 | 100.00% | 0.0013 |
| 200 | 50.00 | burn excess | 707.2 | 8.71% | 0.0013 |
| 200 | 50.00 | carry-forward | 8121.4 | 100.00% | 0.0013 |

**Reading the historical annual-lump model.** Under **burn-the-excess** the capped lottery is a *trickle*: 1.3%–8.7% of demurrage revenue reaches the lottery, the rest destroyed. The large fees that dominate revenue exceed the per-block cap by hundreds of times, so all but `cap/lump` of each is burned. Under **carry-forward** — the excess waits in an accumulating pool drained at ≤ cap per block — ~100% of revenue is eventually redistributed, because the annual drain capacity `cap·BLOCKS_PER_YEAR` (≈ 31.5M BTH/yr at cap=5) dwarfs the 8121 BTH/yr inflow. Carry-forward keeps the per-block payout at `R = cap`, so **the model's conditional Claim 1 cancellation is preserved, not a production Sybil guarantee** — the whale still faces `R = ρ·base_fee` per block regardless of how large the backlog grows.

Burn's throttling is worst exactly where reality lives — big holders making few big spends. Smoothing spending (higher granularity) lets more revenue clear the cap, but the realistic low-granularity end is a trickle (ρ=20, cap=5):

| granularity ×mult | burn throughput |
|---:|---:|
| 1 | 2.71% |
| 4 | 5.38% |
| 16 | 12.03% |
| 64 | 38.63% |
| 256 | 77.17% |

**Capped-lottery Δgini** (10-year collect-and-redistribute; burn baseline Gini 0.7525; ρ=20 ⇒ cap=5 BTH/block):

| policy | final Gini | Δgini |
|---|---:|---:|
| Uncapped windowed (uniform) | 0.6381 | +0.1144 |
| Capped + carry-forward | 0.6381 | +0.1144 |
| Capped + burn excess | 0.7488 | +0.0038 |
| ValueWeighted (incumbent, NOT CT) | 0.7144 | +0.0382 |

**Reading this annual expected-value result.** Carry-forward reproduces the uncapped windowed lottery's Δgini **exactly** (+0.1144, matching §7.4 — a consistency check) and beats the value-weighted incumbent's +0.0382 (which matches §4.2's ClusterWeighted — the incumbent routes much of the pool back to the wealthy via `value·tilt`). The capped-**burn** lottery collapses to +0.0038 — essentially the burn baseline: because most demurrage revenue is destroyed rather than handed to the poor, the equalizing benefit is lost (the whale still sheds the demurrage, which is deflationary, but the poor never receive it).

**Historical Claim 2 verdict: conditional on this annual-lump model.** Burn-the-excess yields the reported 1–9% throughput and Δgini +0.0038; carry-forward yields ≈100% and +0.1144 under its rate/cost premises. The modeled inflow 0.0013 BTH/block is below the 5 BTH cap at ρ=20 (as is the separately cited #351 steady ~1.9 BTH/block). These comparisons do not measure current production throughput, Gini or stability. Such quantitative production conclusions are withdrawn here as **needs production-mode re-run**. A persistent mean inflow above a fixed cap grows backlog in the model; any alternative channel remains a policy proposal, not a finding about expected live operation.

### 9.3 Disposition of the historical Path C verdict

> **Historical status (2026-10-31, #1682):** the cap argument requires a per-block arrival rate and a charge for every created ticket. The implementation did not satisfy those premises; #1761's stationary study found positive profit throughout its grid. The model refutation and the pending remedy decision remain.

The former unconditional “survives both claims” verdict is withdrawn. Structural Uniform selection avoids a hidden-value weighted sort; it does not establish economic neutrality.

- **Claim 1:** retain the conditional rate/cost algebra and the scoped formal counterexamples in the [ledger](#evidence-and-claim-reconciliation). Do not use it as a current production neutrality claim.
- **Claim 2:** retain the historical ≈100% carry-forward throughput and Δgini +0.1144 versus the 1–9% burn throughput and +0.0038, under §9.2's annual-lump assumptions. Current throughput, Gini and stability conclusions are **needs production-mode re-run**.
- **Historical parameter proposal:** uniform last-N selection, `N ≈ 10,000` (~14 h at the 5 s reference), and `R = min(actual_fee_pool, ρ · base_fee)` with `base_fee = 0.25 BTH`. The old proposal called ρ the window count; the proof used an arrival rate. The 0.25 BTH amount is not a charged consensus ticket floor. These values are recorded history, not new ratification.
- **Excess handling:** carry-forward and the watch condition `mean_fee_inflow/block ≤ ρ·base_fee` belong to the historical proposal. No Sybil-safety or full-redistribution guarantee follows for current production; a different excess channel remains an operator decision.

The #1800 restatement, separate M2/redesign results and policy #1837 remain unchanged. Neither current-source regeneration, DRT correspondence nor the two-block behavior example above closes those economic or policy obligations.
