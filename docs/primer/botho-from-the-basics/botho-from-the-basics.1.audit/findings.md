# Findings — botho-from-the-basics.1 audit

Spec oracle: `spec_ref: ../sections/*.tex` (ACTIVE), resolved to the 18 whitepaper
LaTeX sections under `whitepaper/sections/`. "WP §N" cites the whitepaper section;
code paths cite the Botho Rust implementation (consulted where the whitepaper is
ambiguous, per the audit charter; the whitepaper remains the oracle for flags).

Verification vocabulary: **verified** (claim matches spec/code) ·
**verified-w/-simplification** (lossy-but-true; correct carried-away belief) ·
**contradicts** (would fire flag 2 — none) · **false** (would fire flag 3 — none).

## Named findings

| ID | Severity | Finding |
|---|---|---|
| F1 | — (record) | spec_ref ACTIVE and fully resolved (18/18 sections). Spec-consistency sweep ran; 0 contradictions. |
| F2 | **major** (operator-facing; not a primer defect) | Whitepaper↔code divergence on ML-DSA-65's role. WP §4 "Minting Signatures" (and §2, §5 table) assign ML-DSA-65 to minting transactions. The live `MintingTx` (`botho/src/block.rs`) carries **no** ML-DSA signature — minter identity binds via the PoW preimage over `minter_view_key ‖ minter_spend_key` — while `botho/src/transaction_pq.rs` applies ML-DSA-65 signatures to transaction **inputs** (a quantum-private tx variant), the opposite allocation. The primer follows the spec (correctly, per its own "the whitepaper wins" preamble and the BRIEF's oracle designation); the reconciliation belongs to the whitepaper/implementation, not the primer. |
| F3 | minor | Capstone step 2 states the fee as "base rate × size × factor 1" but the resulting "a few tens of nano-BTH" is only reproduced with the output-count penalty (min(2,10)² = 4) from WP §7's own derivation: 1 pico/byte × ~5,000 B × 1 × 4 = 20,000 pico = 20 nano-BTH. Magnitude correct; formula incomplete. |
| F4 | minor | "two-of-forty possible inputs" (capstone summary) — an upper bound: WP §5 does not forbid cross-ring decoy overlap (only within-ring canonical ordering; the two real inputs are distinct via distinct key images). Guarantee is "each of two inputs hidden among 20." Suggest rephrase. |
| F5 | minor | "a laptop or a $50/month cloud node buys tickets at roughly fair odds per watt" — extends past WP's stated claims (ASIC resistance, linear reward scaling, solo viability; §7 "Decentralization Incentives", §10). Hedged and directionally faithful; carried-away belief matches spec intent. |
| F6 | minor | §9.6(3) "redistribution marginally improves" under the whale attack — WP §10 table for the full mechanism: honest ΔG +0.078 vs gamed +0.076 (essentially unchanged, marginally less); the WP sentence "the gamed run shows marginally more redistribution" applies to the cluster-tilted+emission row (+0.054/+0.055). Teaching point ("attack does not degrade redistribution and costs the attacker ~19%") is spec-true; wording slightly overstates. |
| F7 | minor | "roughly 611 million BTH, the overwhelming majority of supply" — horizon-dependent under a perpetual ~2% tail (WP §7 projection: year-20 supply 884.9M → Phase-1 is ~69%). True for the first decades; suggest "the large majority of supply for decades." |
| F8 | observation | "million-BTH lineages saturate near 6×" — WP §5 table gives ≈5.2–6.0× for ≥1M BTH (the log-sigmoid is ≈5.2× at exactly 1M). "Near 6×" is fair for ≫1M; borderline at 1M. |
| F9 | observation | "a fee whose size reveals only the *lineage class* of the coins moved" — the fee also encodes tx size and, for high-factor coins, the demurrage clock (WP §7 fee floor). Accurate for the capstone's factor-1 case; "only" is slightly strong in general. |
| F10 | observation (spec-internal, not primer) | WP-internal inconsistencies observed during the sweep: (a) §10 "Economic Constants Summary" lists Min block time 5 s vs §7's 3 s consensus floor and §1's "3–40 seconds" (primer follows §1/§7); (b) §2's PQ-comparison table lists CLSAG ring-16 = 704 B while §5's size table gives 704 B at ring 20 = (n+2)×32 (primer's "~700 bytes" matches §5 and the Parameter appendix at ring 20). |

## Per-claim verification table

| # | Claim (primer location) | Kind | Verified? | Evidence / cited source |
|---|---|---|---|---|
| 1 | Three mandatory protections; privacy not opt-in (§1) | spec-consistency | verified | WP §1 "Privacy as baseline: All transactions are private by default" |
| 2 | Botho = Sesotho/Setswana "humanity"; opening proverb "a person is a person through other people" (§1) | spec-consistency | verified | WP §1 epigraph + Design Philosophy |
| 3 | Harvest-now-decrypt-later threat; chain data permanent (§1, §6) | spec-consistency | verified | WP §1, §2 "Quantum Threat Model" |
| 4 | Monero and MobileCoin are classical-only; retroactive recipient unmasking risk (§1) | spec-consistency | verified | WP §2 (Monero "No quantum resistance"; MobileCoin "primitives are entirely classical") |
| 5 | Fee-only security is an open question for fixed-supply chains (§1, §9.1) | spec-consistency | verified | WP §1 "security budget problem", §10 (budish2022/huberman2021) |
| 6 | Curve group is Ristretto255; scalar·point one-way (DLP) (§2) | spec-consistency | verified | WP §3 (Ristretto255, DLP definition) |
| 7 | Shor breaks curve one-wayness on a large QC (§2) | factual | verified | WP §2 "Quantum Threat Model" |
| 8 | KEM = encapsulation to (secret, ciphertext); recipient-only decapsulation (§2) | spec-consistency | verified | WP §3 "ML-KEM-768" API listing |
| 9 | Paint-mixing analogy for one-wayness (§2) | factual | verified-w/-simplification | Conveys one-wayness only (drafter self-check concurs); no group-structure claim made |
| 10 | View/spend key split; published address = the two public keys (§3) | spec-consistency | verified | WP §4 "Key Hierarchy": A=aG, B=bG, address (A,B) |
| 11 | 24-word mnemonic; everything derived from one seed (§3, §6) | spec-consistency | verified | WP §4 (BIP39 24-word, SLIP-10); "minter's ML-DSA public key is derived from the same seed" |
| 12 | Subaddresses: unlimited unlinkable addresses (§3) | spec-consistency | verified | WP §4 Subaddress Derivation + Unlinkability theorem |
| 13 | View-key-only scanning (accountant/auditor use) (§3) | factual | verified-w/-simplification | WP §4: scanning needs sk_kem = DeriveKEM(a) (view side) only; spend needs b. Standard CryptoNote capability |
| 14 | Classical CryptoNote stealth flow (DH vs view key → one-time key; recipient re-derives) (§3) | spec-consistency | verified | WP §4 Post-Quantum Stealth Addresses (DKSAP lineage, CryptoNote cite) |
| 15 | Ring of 20; 19 decoys; decoys uninvolved (§4) | spec-consistency | verified | WP §5 `RING_SIZE = 20`; §4 "n−1 decoy outputs… (we use n = 20)"; code `transaction/types/src/constants.rs:155` |
| 16 | CLSAG named + expanded; ~700 B/input; adopted by Monero (§4) | spec-consistency | verified | WP §4 CLSAG; §5 table 704 B; §2 (Monero CLSAG); Parameter appendix "~700 B" at ring 20 |
| 17 | Key image: same coin ⇒ same image; unlinkable; DB of seen images; in-signature correctness proof (§4) | spec-consistency | verified | WP §4 (I = x·Hp(P), linkability proof); §5 validation rule 2 |
| 18 | Decoy sampling: spend-age distribution + cluster-tag similarity (§4) | spec-consistency | verified | WP §5 "Decoy Selection" (empirical age distribution; ≥70% cosine similarity) |
| 19 | Probabilistic anonymity vs Zcash ZK; trade made deliberately, no trusted setup (§4) | spec-consistency | verified | WP §2 Zcash ("accepting the tradeoff of probabilistic rather than cryptographic anonymity") |
| 20 | Pedersen: information-theoretic hiding, computational binding, homomorphic; blinding factor (§5) | spec-consistency | verified | WP §3 Pedersen theorem |
| 21 | Balance equation: Σin = Σout + fee (public), checked on commitments (§5) | spec-consistency | verified | WP §4 Value Conservation |
| 22 | Wrap-around/negative-amount exploit; range proof to [0, 2^64); Bulletproofs logarithmic aggregation (§5) | spec-consistency | verified | WP §3 Bulletproofs; §4 Range Proofs |
| 23 | Blinding factor + encrypted amount derived from the shared secret (§5, capstone 3) | spec-consistency | verified | WP §4 ("derived deterministically from the shared secret"); §5 `encrypted_amount` field |
| 24 | Permanent vs ephemeral data split; design rule (§6) | spec-consistency | verified | WP §4 Hybrid Architecture Rationale table |
| 25 | ML-KEM (formerly Kyber), lattice KEM, NIST standard; ML-KEM-768 = NIST middle security category (§6) | spec-consistency | verified | WP §3 (FIPS 203, "formerly known as Kyber"); category 3 = middle of 5 (WP labels sibling ML-DSA-65 "security level 3"). No contradiction with WP's quoted bit figures — primer asserts no number |
| 26 | PQ stealth swap: encapsulate against KEM key derived from view key; ciphertext 1,088 B per output; recipient decapsulates and matches (§6, capstone 3/11) | spec-consistency | verified | WP §4 protocol listing (pk_kem = DeriveKEM(A); c = 1,088 bytes; scan-by-decapsulation); §3 ciphertext size |
| 27 | Quantum adversary with full chain cannot link outputs to recipients; proven by reduction to ML-KEM security (§6) | spec-consistency | verified | WP §4 Recipient Unlinkability theorem + QROM corollary |
| 28 | Ciphertext = biggest single line item in a tx (§6) | spec-consistency | verified | WP §5 size table (1,088 of each ~1,152-B output; largest single component vs 704/736/680) |
| 29 | ML-DSA-65 (formerly Dilithium) signs minting txs; identity public; ~3.3 KB signature; once per block (§6, capstone 8) | spec-consistency | verified (spec) — see F2 for code | WP §4 Minting Signatures; §3 (3,309 B); §5 Minting type table; §2 "ML-DSA-65 for minting". Code diverges (F2) |
| 30 | PQ ring signatures ~50× CLSAG; ~35 KB/input; multi-input > 100 KB; migration path documented (§6) | spec-consistency | verified | WP §4 "Why not full post-quantum?"; §2 PQ ring-signature survey + migration path |
| 31 | Sender anonymity value decays with time (§6) | spec-consistency | verified | WP §4 "Why is ephemeral sender privacy acceptable?" |
| 32 | Nakamoto = probabilistic finality, tens of minutes; BFT = deterministic but fixed membership (§7) | spec-consistency | verified | WP §6 Design Rationale; §2 consensus review |
| 33 | Quorum slice = per-node local trust declaration; quorum = self-sufficient set (§7) | spec-consistency | verified | WP §6 Definitions (Quorum Slice, Quorum) |
| 34 | Safety = any two quorums share ≥1 honest node → no two honest nodes finalize different blocks at same height (§7) | spec-consistency | verified | WP §6 SCP Safety theorem + Fork Freedom theorem |
| 35 | Tiered default: few high-uptime infrastructure nodes + community validators (§7) | spec-consistency | verified | WP §6 Tiered Quorum Structure (3-of-4 infra AND 2-of-3 community) |
| 36 | Rounds: nominate → ballot (prepare/commit, escalating) → externalize; no reorgs of externalized blocks (§7) | spec-consistency | verified-w/-simplification | WP §6 four phases (PoW proposal folded into primer §8; ballot abort/higher-number ✓; externalize = deterministic finality ✓) |
| 37 | Voting takes a few seconds after proposal; finality = block time + few seconds (§7, §8, capstone 9) | spec-consistency | verified | WP §6 Timing table (total = block time + ~3–4 s; "within 5 seconds of block proposal") |
| 38 | Halt-don't-fork under partition; resume without unwinding (§7) | spec-consistency | verified | WP §6 "nodes simply halt rather than fork… safety over liveness" |
| 39 | MobileCoin: closest prior design; CryptoNote privacy + SCP; no mining; fixed supply (§8) | spec-consistency | verified | WP §2 MobileCoin ("closest existing system"; "fixed supply"; SCP not PoW). "Fully pre-created" not asserted by WP but consistent and externally true |
| 40 | PoW decides proposal + reward; SCP decides finality; hashpower buys zero consensus votes (§8) | spec-consistency | verified | WP §6 Phase 1 + Fork Freedom corollary ("not merely accumulating hashpower"); §1 contribution 2 |
| 41 | 51% hashpower cannot rewrite externalized blocks; requires corrupting quorum structure (§8) | spec-consistency | verified | WP §6 Finality Irreversibility corollary |
| 42 | Not proof-of-stake; ownership buys no consensus power; validators earn no fees (§8) | spec-consistency | verified | WP §6 (trust-based slices); §7 (minter income = R(h); fee split 80/20 exhausts fees); §10 Validator Incentives (no fee income listed) |
| 43 | RandomX: CPU-oriented, Monero lineage, ASIC-hostile; linear reward scaling; no protocol economies of scale (§8) | spec-consistency | verified | WP §7 Decentralization Incentives; §10 Mining Centralization mitigation |
| 44 | "Laptop buys tickets at roughly fair odds per watt" (§8) | factual | verified-w/-simplification (F5) | Beyond WP's literal claims but faithful to their intent; hedged |
| 45 | Adaptive block timing 3 s (heavy) – 40 s (idle), 5 s reference under sustained load; idle network mints less (§8, §9.1, capstone 8) | spec-consistency | verified | WP §7 Dynamic Block Timing (5 discrete levels, 3 s floor, 40 s ceiling, 5 s monetary reference); §1 contribution 4. WP §10 constants-table "Min 5s" is spec-internal drift (F10) |
| 46 | Emission: 50 BTH start, yearly halvings ×5 (50/25/12.5/6.25/3.125), ≈611M Phase-1 (§9.1) | spec-consistency | verified | WP §7: S1 = 611,010,000 BTH; halving period 6,307,200 blocks ≈ 1 yr at 5 s |
| 47 | "The overwhelming majority of supply" (§9.1) | factual | verified-w/-simplification (F7) | WP §7 projection: ~69% at year 20; horizon-dependent |
| 48 | Tail: perpetual, ~2% net, scales with supply, gross > net anticipating burn (§9.1) | spec-consistency | verified | WP §7 Tail Emission (β_net = 0.02, β_burn = 0.005; R_tail ∝ S) |
| 49 | 2% dilution as gentle wealth tax on idle balances (§9.1) | spec-consistency | verified | WP §7 Wealth Tax Equivalence (≈2%) |
| 50 | 1 BTH = 10^12 picocredits; integer picocredit arithmetic (§9.1) | spec-consistency | verified | WP §7 ("smallest unit is the picocredit"; Decimals 12); code integer-only fee/demurrage paths |
| 51 | Wealth invisible ⇒ tax lineage, not holder; identity/account mechanisms dead on arrival (§9.2) | spec-consistency | verified | WP §5 Cluster Tags intro ("provenance rather than current ownership"); §1 contribution 3 |
| 52 | Cluster = per-minting-event lineage, id = hash(minter key ‖ height); fresh coins 100% tagged (§9.3) | spec-consistency | verified | WP §5 (c_new = Hash(minter_pubkey ‖ block_height); t_mint = {(c_new, 1.0)}) |
| 53 | Tag vector = value-weighted blend; 70/30 example (§9.3) | spec-consistency | verified | WP §5 blending equation + identical 70 A / 30 B example |
| 54 | Splitting/self-shuffling are no-ops; only real commerce dilutes tags (§9.3) | spec-consistency | verified | WP §5 Sybil Resistance Analysis + Theorem |
| 55 | Cluster wealth = total tagged value across UTXO set; can far exceed one reward (§9.3) | spec-consistency | verified | WP §5 Cluster Wealth Accumulation (W_c; u128 cumulative) |
| 56 | Factor 1–6×; ~100k BTH midpoint → 3.5×; log-domain sigmoid; exact curve deferred to spec (§9.3) | spec-consistency | verified | WP §5 Cluster Factor (W_mid = 100k → exactly 3.5×; clamp [1,6]; log2 domain). Deferral honored |
| 57 | "Million-BTH lineages saturate near 6×" (§9.3) | spec-consistency | verified-w/-simplification (F8) | WP §5 table: ≥1M BTH ≈5.2–6.0× |
| 58 | Decay 5% per qualifying transfer; ≥720 blocks (~1 h at 5 s) to qualify; clock not count binds; weeks to wash clean (§9.3) | spec-consistency | verified | WP §5 Age-Based Tag Decay (×0.95; T_min = 720; "binding constraint is age, not transaction count"; ≈100% at 1 patient week) |
| 59 | Min fee scaled by cluster factor; factor-1 pays base; floor in nano-BTH; whale ≤6× (§9.4) | spec-consistency | verified | WP §7 Base Fee (1 pico/byte floor; worked 16 nano-BTH); §5 multiplier table |
| 60 | Sybil-proof by construction; blending averages, never launders (§9.4) | spec-consistency | verified | WP §5 Theorem + mixing analysis ("weighted average, not a minimum") |
| 61 | Fee charged at max factor among ring members; decoy similarity keeps the max honest (§9.4, capstone 5) | spec-consistency | verified | WP §5 Ring Signature Tag Propagation + Decoy Selection |
| 62 | Demurrage: stock-level term when high-factor coins move; ∝ value, time held, cluster factor; factor-1 exempt (§9.4, capstone 2) | spec-consistency + factual | verified | WP §7 fee floor (d(t,v) stock-level term; "exempt for factor-1 coins"); code `cluster-tax/src/demurrage.rs`: charge = value × rate × (factor−1)/(max−1) × elapsed/blocks_per_year — time-proportionality confirmed in code; affine-in-factor nuance covered by the stated factor-1 exemption |
| 63 | Gesell/stamp-scrip contrast: classical demurrage taxed everyone; Botho only concentrated lineages (§9.4) | spec-consistency | verified | WP §2 Demurrage subsection (Freigeld/Chiemgauer; "applies demurrage selectively") |
| 64 | Congestion pricing: per-byte base floats up, ≤100× floor (§9.4) | spec-consistency | verified | WP §7 ("bounded by a 100× safety clamp") |
| 65 | Miners never receive fees; no MEV/reorder/censor motive (§9.5) | spec-consistency | verified | WP §7 (minter income = R(h), Anti-MEV); §10 Miner Incentives |
| 66 | Fee split 20% burn / 80% pool; burn can outpace tail under heavy use (§9.5) | spec-consistency | verified | WP §7 Fee Redistribution; Effective Inflation ("burning may exceed tail emission") |
| 67 | Emission share ramps over halvings to 50% of reward; reaches idle wealth fees cannot (§9.5) | spec-consistency | verified | WP §7 ("ramps from 0… 10 percentage points per halving epoch to a 50% cap"; consumption-tax argument) |
| 68 | 4 winners/block; payout capped at one reward; surplus carries; no registration (§9.5, capstone 10) | spec-consistency | verified | WP §7 Lottery Mechanism |
| 69 | Weight = value × inverse-factor tilt; factor-1 gets 6× per-BTH weight; split-invariant; dust floor 1 micro-BTH; ~1 h maturity (§9.5) | spec-consistency | verified | WP §7 (w = v(φmax − φ + 1), φmax = 6; ≥720 blocks, ≥10^6 pico); §10 split-invariance |
| 70 | Seed = hash of previous externalized block (+height); proposer cannot bias via current block (§9.5, capstone 10) | spec-consistency | verified | WP §7 seed equation; §9 Lottery Grinding defense |
| 71 | Regrinding prev block costs a full PoW solution to redirect ≤ a fraction of the capped payout; unprofitable by construction (§9.5) | spec-consistency | verified | WP §9 cost-benefit bound: grinding_cost ≈ R(h) > Δ_payout |
| 72 | Sim result 1: per-UTXO lottery best honest / worst gamed; whale captures stream, Gini rises (§9.6) | spec-consistency | verified | WP §10 (uniform per-UTXO: honest +0.176, gamed −0.026; whale 5%→24%) |
| 73 | Sim result 2: wealth-proportional dilution exactly neutral; tilt required (§9.6) | spec-consistency | verified | WP §10 ("exactly Gini-neutral") |
| 74 | Sim result 3: full system — attack costs ~a fifth of position over 5 yrs; redistribution "marginally improves" (§9.6) | spec-consistency | verified-w/-simplification (F6) | WP §10: ~19% cost ✓; "does not degrade" ✓; full-system gamed +0.076 vs honest +0.078 — "improves" fits the tilted+emission row, not the full row |
| 75 | Ancestry = the only wealth signal restructuring cannot forge; intake and outflow both anchor to it (§9.6) | spec-consistency | verified | WP §7 ("the only split-invariant progressive signal available in an anonymous-value system") |
| 76 | Capstone fee: 2-in-2-out ~5 KB, factor 1, no demurrage ⇒ "a few tens of nano-BTH"; 100k lineage ⇒ ~3.5× + holding cost (capstone 2) | spec-consistency + factual | verified-w/-simplification (F3) | WP §7 derivation: 1 pico/byte × 5,000 × 1 × min(2,10)^2 = 20,000 pico = 20 nano-BTH ✓; Parameter appendix pins "<5 KB for 2-in-2-out" ✓; 3.5× midpoint ✓; picocredit units exact, no 10^3 error. Printed formula omits the ×4 output penalty |
| 77 | Payment and change outputs indistinguishable on-chain (capstone 3) | factual | verified | Both are standard `TxOutput`s (WP §5); no distinguishing field |
| 78 | Validators check: key images fresh, rings valid, sigs verify, commitments balance, proofs check, fee sufficient (capstone 7) | spec-consistency | verified | WP §5 Validation Rules 1–8 (subset listed; size cap omitted — lossy) |
| 79 | Dandelion++ stem then fluff; origin-IP protection (capstone 7) | spec-consistency | verified | WP §8 Dandelion++ ("Cannot trace back to stem origin") |
| 80 | "Two-of-forty possible inputs" (capstone summary) | factual | verified-w/-simplification (F4) | Upper bound; per-input guarantee is 1-of-20 (WP §5); cross-ring decoy overlap not forbidden |
| 81 | "Fee… reveals only the lineage class" (capstone summary) | factual | verified-w/-simplification (F9) | Fee also encodes size and (factor >1) the demurrage clock; fine at the capstone's factor 1 |
| 82 | §11 whitepaper map (13 rows) + external literature (Zero to Monero, CLSAG, Bulletproofs, CryptoNote, Mazières SCP, FIPS 203/204, RandomX spec) | spec-consistency | verified | Row-by-row against the section files present in `whitepaper/sections/`; FIPS 203/204 per WP §3 cites |

## Counts

- Claims examined: 82 rows (the six drafter-flagged spots are rows 25/26/29/44/62/76/80).
- Contradictions ("Contradicts cited spec"): **0**
- False simplifications ("Subtly-wrong intuition"): **0**
- Verified-with-simplification (lossy-but-true): 10 (rows 9, 13, 36, 44, 47, 57, 74, 76, 80, 81)
- Named findings: 1 major (F2 — whitepaper↔code, operator-facing), 5 minor (F3–F7), 3 observations (F8–F10)
