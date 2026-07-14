# Findings — botho-from-the-basics.2 (audit)

Spec oracle: `spec_ref: ../sections/*.tex` → 18 files under `whitepaper/sections/` (resolved, active). Code consulted where the spec's gloss is compressed: `cluster-tax/src/demurrage.rs`.

Severity legend: **critical** = fires an audit flag (none); **minor** = precision note, no false belief created; **obs** = observation / operator-facing.

## Per-claim table — v2 delta (new quantitative claims)

| # | Claim (primer location) | Kind | Verified? | Evidence / cited source |
|---|---|---|---|---|
| D1 | Factor-6 lineage idle one year owes ~2% of value moved (§9.4) | factual + spec-consistency | **verified** | WP §10 operating point "2%/yr demurrage at factor 6" (`10-economics.tex:286`); `demurrage.rs`: value × 200 bps × (6−1)/(6−1) × 1 yr = 2.00% |
| D2 | Factor-3.5 lineage owes "roughly half that" (§9.4) | factual | **verified** (exactly half) | `demurrage.rs` affine law: (3.5−1)/(6−1) = 0.5 |
| D3 | Factor-1 coins exempt from demurrage (§9.4, capstone step 2) | spec-consistency | **verified** | WP §7 "Demurrage is exempt for factor-1 coins" (`07-monetary.tex:254`); code returns 0 at progressivity 0 |
| D4 | Charge "keeps accruing for as long as the coins sit idle" (§9.4) | factual | **verified** | `demurrage.rs` elapsed term; churn-invariance documented in module header |
| D5 | ~6.3M blocks/year at the 5-second reference pace (§9.1) | spec-consistency | **verified** | WP §7: halving period 6,307,200 blocks ≈ 1 year at 5 s (`07-monetary.tex:492`); tail table row "High, reference (5s) & 6,307,200" (`:214`) |
| D6 | Year one ≈ 315M BTH (§9.1) | factual + spec | **verified** | 50 × 6,307,200 = 315.36M; WP supply table year 1 = 315.4M (`:99`) |
| D7 | Halving yearly (50/25/12.5/6.25/3.125), summing to ~611M (§9.1) | spec-consistency | **verified** | WP §7 note (`:110–111`); S₁ = 611,010,000 (`:49`) |
| D8 | "5-second reference pace" is the pace governing emission math (§9.1, §8) | spec-consistency | **verified** | WP §7: "The 5-second level is the *monetary reference*: all emission…" (`:165`); 3 s = consensus floor, 40 s = idle ceiling (`:497–499`). §10's "Min block time 5s" (`10-economics.tex:530`) is the WP-internal editorial item (v1 F10, carried as obs O2) — primer sides with the governing §7 |
| D9 | Fee: 1 pico/byte × ~5,000 B × factor 1 × output-penalty 4 = 20,000 pico = 20 nano-BTH (capstone step 2) | factual + spec | **verified** (arithmetic + units exact; see N1) | WP §7: f_base = 1 pico-BTH/byte (`07-monetary.tex:232`); f_min = b_dyn × size × cluster_factor + d (`:244–248`); output penalty min(2,10)² = 4 (`:387–391`); 1 BTH = 10¹² pico (`:235`) ⇒ 20,000 pico = 2×10⁻⁸ BTH = 20 nano |
| D10 | ~5,000-byte / "~5 KB" transaction size for 2-in-2-out (capstone steps 2, 7) | spec-consistency | **verified-with-simplification** (N1) | Parameter appendix: "<5 KB for 2-in-2-out" (`appendix-parameters.tex:45`). Spec-internal tension: §5 byte table ~4,552 B for 1-in-2-out (`05-transactions.tex:425`) implies ~5.9 KB at 2-in; §7's worked instance uses 4,000 B → 16 nano (`07-monetary.tex:380–391`). All in the same magnitude; operator editorial item |
| D11 | 100,000-BTH lineage → ~3.5× fee + holding-cost term (capstone step 2) | spec-consistency | **verified** | WP §5 table: ~100,000 BTH (midpoint) → 3.5× (`05-transactions.tex:236`); d > 0 for factor > 1 |
| D12 | Million-BTH lineages "climb past 5× toward the 6× ceiling" (§9.3) | spec-consistency | **verified** | WP §5 table: ≳1,000,000 BTH → ≈5.2×–6.0× (`:237`) |
| D13 | Phase-1 coins ≈ 69% of supply at year 20 (§9.1) | factual + spec | **verified** | WP §7 supply projection: year 20 = 884.9M (`07-monetary.tex:105`); 611.0/884.9 = 69.05% |
| D14 | Fee reveals size + lineage class (+ demurrage clock for wealthier lineages), "never who and never how much" (capstone summary) | factual | **verified-with-simplification** (N2) | f_min = b·size·φ + d (`07-monetary.tex:244–248`); d ∝ value × elapsed (`demurrage.rs`) so the fee constrains the *product* for factor > 1; WP §9 metadata table: "Fee amount, 2–4 bits, residual 1" (`09-security.tex:276`). Carried-away belief (amounts not visible on-chain) is correct per WP §4/§9 headline claims |
| D15 | "Two inputs, each hidden among twenty candidate coins" (capstone summary) | factual | **verified** | Per-ring guarantee, ring size 20 (WP §5; §9 log₂(20) anonymity bound). Correctly replaces v1's 2-of-40 union upper bound (F4) |
| D16 | RandomX: "odds proportional to the computation it contributes, with no structural edge for specialized hardware" (§8) | spec-consistency | **verified** | WP §7: "No economy of scale: Linear scaling of rewards" (`07-monetary.tex:434`); "CPU-friendly PoW: Algorithm resists ASIC development" (`:430`); §10 mitigation row (`10-economics.tex:502–503`). v1 per-watt overreach removed (F5) |
| D17 | Attack "costs the attacker roughly a fifth of its position over five years" (§9.6) | spec-consistency | **verified** | WP §10: "costing the attacker ∼19% of its position over five years" (`10-economics.tex:346–348`) |
| D18 | "Redistribution holds essentially unchanged — it does not degrade" (§9.6) | spec-consistency | **verified** | WP §10 table: +0.078 ± 0.0002 honest / +0.076 gamed (`:322`); "The strategic-whale … attack does not degrade this" (`:366–368`). v1 F6 ("marginally improves") correctly repaired |

## Per-claim table — carried-forward load-bearing sample

| # | Claim | Kind | Verified? | Evidence |
|---|---|---|---|---|
| S1 | CLSAG ≈ 700 B per input | factual | verified | 704 B (`05-transactions.tex:425` table); §9 s_i ≈ 700 B |
| S2 | ML-KEM-768 ciphertext 1,088 B; NIST middle category | spec | verified | `03-preliminaries.tex:145`; `04-cryptography.tex:91`; appendix Level-3 rationale |
| S3 | ML-DSA-65 signature ≈ 3.3 KB, minting-only, seed-derived key | spec | verified | Signature size 3,309 B (`03-preliminaries.tex`); minting role (`04-cryptography.tex:316–323`). WP↔code divergence = carried operator item O1 |
| S4 | PQ ring sigs ~50× CLSAG (~35 KB/input); several-input payment > 100 KB | spec | verified | "~35 KB per input, making transactions 10× larger" (`05-transactions.tex:430–431`); 35,000/704 ≈ 49.7 |
| S5 | Fees: 20% burned / 80% to pool; 4 UTXOs/block; capped at one block reward; surplus carries over | spec | verified | `07-monetary.tex:270–291`, constants `:501` |
| S6 | Emission-to-lottery slice ramps to 50% of reward | spec | verified | "ramps from 0 at genesis by 10 percentage points per halving" (`:281`); cap 50% (`:502`) |
| S7 | Lottery seed = previous, already-finalized block hash; grinding costs a full PoW ≈ R(h) > redirectable payout | spec | verified | `:328–346`; `09-security.tex:729–739` |
| S8 | Inverse-factor tilt: factor-1 coin has 6× per-BTH weight of factor-6 | spec | verified | E[income] ∝ v(φ_max − φ + 1) (`10-economics.tex`) |
| S9 | Eligibility: dust floor one millionth of a BTH; maturity ~1 hour | spec | verified | ≥ 720 blocks old and ≥ 10⁶ pico-BTH = 1 micro-BTH (`07-monetary.tex:326–327`) |
| S10 | Tag decay 5% per qualifying transfer; 720-block (~1 h) age gate; clock-bound, not count-bound | spec | verified | ×0.95/event, one event per 720 blocks ≈ 1 h at 5 s (`05-transactions.tex:159–180`) |
| S11 | Decoys: spend-age realistic + cluster-tag similarity | spec | verified | Decoy Selection: empirical age distribution; ≥70% cosine tag similarity (`:435–443`) |
| S12 | Fee charged on max ring factor; whales can't hide behind low-factor decoys | spec | verified | `:245–257` |
| S13 | Factor curve: log-domain sigmoid, midpoint 100k BTH at exactly 3.5×, clamped [1,6] | spec | verified | `:216–223` |
| S14 | Adaptive block time 3–40 s, 5 s reference; idle network emits less | spec | verified | `07-monetary.tex:497–499`, tail-emission table `:206–219` |
| S15 | Base unit 1 BTH = 10¹² picocredits; integer arithmetic | spec | verified | `:235`; `appendix-parameters` decimals row |

## Non-flag findings

| ID | Severity | Finding |
|---|---|---|
| N1 | minor | Capstone size assumption ~5,000 B for 2-in-2-out follows the Parameter appendix ("<5 KB"), but WP §5's byte table implies ~5.9 KB at 2 inputs and WP §7's worked instance uses 4,000 B (→16 nano). The primer's 20-nano figure is exact under its stated, hedged size and magnitude-consistent with every spec statement. The three-way size tension is a **whitepaper editorial item** (adjacent to v1 F10), not a primer defect. |
| N2 | minor | "Never how much" (capstone summary) is absolute; for factor >1 spends the public fee's demurrage component scales with value moved × idle time, i.e. it constrains a product involving the amount (WP §9 itself prices fee-amount leakage at 2–4 bits, residual ~1). Lossy-but-true — the belief "amounts are not visible on-chain" remains correct — but a half-clause hedge would be strictly tighter. Optional polish. |
| O1 | obs (operator, carried from v1 F2) | WP §4 "Minting Signatures" vs live code divergence on ML-DSA-65's role (minting-only per spec; per-input in `transaction_pq.rs`, none on `MintingTx` in `block.rs`). Primer follows the declared oracle; reconciliation stays operator-side. |
| O2 | obs (operator, carried from v1 F10) | WP-internal inconsistencies unchanged: §10 constants "Min block time 5s" vs §7's 3 s floor; §2 CLSAG byte figure vs §5/appendix. Primer sides with the governing sections in each case. |

**Majors: none.** (`spec_ref` is declared and resolves — the missing-spec major does not apply.)
**Critical flags: none.** `audit_clean: true`.
