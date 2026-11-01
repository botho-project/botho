# Ring Signature Privacy Analysis

This document gives a quantitative privacy analysis of Botho's ring
signatures, measured against the decoy pickers that production actually runs.
It was regenerated for #1692 / #1755 (random-offset age window) from
`cluster-tax-sim` output; the exact commands are listed below. The full tables, including the per-rank histograms, are in
[`cluster-tax/src/simulation/PRIVACY_ANALYSIS.md`](../../cluster-tax/src/simulation/PRIVACY_ANALYSIS.md).

> **Retraction.** An earlier version of this document recommended ring size
> 11 and analysed an age-ratio constraint of 2.0× and a factor-ratio
> constraint of 1.5×. None of these match the code:
>
> - Ring size is **20**: `MIN_RING_SIZE = DEFAULT_RING_SIZE = 20`
>   (`transaction/clsag/src/lib.rs:976-980`).
> - The age policy is a window **20%** of the real input's age wide
>   (`AGE_SIMILARITY_SPREAD_BPS = 1_000`, ±10%, `botho/src/decoy_selection.rs`).
>   Since #1692 its position is drawn at random per ring
>   (`draw_age_similarity_window`); before that it was centred on the real
>   input.
> - A ±10% factor band exists (`FACTOR_SIMILARITY_SPREAD`,
>   `botho/src/decoy_selection.rs`), but only
>   `select_decoys_cluster_aware` uses it, and nothing in production calls
>   that function.
>
> No CLI command ever produced that version's numbers (2.76 bits, 21.3%
> identification, and so on), and they should not be cited. The text is
> still in git history.

## Executive summary

Since #1692 all three pickers draw one random-offset age window per ring: the
window is 20% of the real age wide, the real input's age is uniform inside
it, and decoys are a uniform sample of it. The node, CLI and web pickers use
the same rule.

| producer | picker | how the real input is exposed |
|---|---|---|
| node ledger/wallet, detached signer | `select_decoys_for_input` | by age: not measurably (rank χ² 11–29, every age-based adversary 4.5–5.5%; Age-Heuristic 6.6% when real ages follow the gamma spend model). Before #1692: youngest in 100% of rings at 1k–5k, oldest in 100% at 100k–1M (edge clamp) |
| `botho-wallet` CLI | `select_rpc_decoy_pool` | by age: not measurably (rank χ² 8–22). Before #1692: the median member of a centred band, midrange guess about 50% (95% at age 100) |
| web wallet, Snap, claim links | `decoys.ts` `selectDecoysForInput` | by age: not measurably (rank χ² 14–23). Before #1692: same centred-band median leak as the CLI (#1755); before #1735, the youngest member in 100% of rings |
| node and CLI (outputs) | plaintext inherited cluster tags | cluster fingerprint 99.7% — upper estimate (every output tagged); about 55% when half are untagged. Web outputs carry empty tags (5.0%, uniform) |

A uniform guess identifies the real input 5% of the time (1 in 20). Against
the age-based adversaries, every production route is now at that level.
Against the node and CLI routes, the cluster-tag fingerprint alone is still
close to decisive (#1756).

## Methodology

- **Pickers.** `cluster-tax/src/simulation/privacy_routes.rs` ports
  `GammaDecoySelector::select_decoys_for_input`,
  `select_rpc_decoys_from_history` / `select_rpc_decoy_pool`, and the web
  `decoys.ts` `selectDecoysForInput` (#1735).
  `botho/tests/privacy_sim_route_parity.rs` pins the two native ports to the
  production functions (same candidates and same seeded RNG give the same
  ring, including the #1786 shuffle and the #1692 window draw) and replays
  the rings recorded from the real `send.ts` with a seeded mulberry32
  (`scripts/research/ct-economics/web-selection.json`, re-recorded for
  #1692) through the web port, which must reproduce them exactly. CI runs it in
  `workspace-build.yml`. The pre-#1735 chain-ordered window survives as the
  historical `--route web-pre-1735` (not in the production default).
- **Chain.** One output at every height 1..=1,200,000; real inputs at fixed
  ages, plus a row whose real ages follow the node picker's own gamma spend
  model (k = 19.28, θ = 1.61 days at 17,280 blocks/day, the mainnet 5 s
  block time; it assumed 720 blocks/day before #1692).
- **Tags.** The legacy output-type mix (standard, exchange, whale, coinbase,
  mixed). Node and CLI outputs publish the real input's tags decayed 5%
  (`botho/src/wallet.rs:695`, `botho-wallet/src/transaction.rs:465`). Web
  outputs publish empty tags.
- **Metric.** Argmax identification rate: how often the adversary's single
  top suspect is the real input, with ties split. Entropy ("bits") is not
  used for headline claims, because it can show 95% while the
  identification rate is 59% (see the legacy table below).

Commands:

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000 --route web-pre-1735
```

## Results (10,000 rings per row, seed 1700)

Identification rates. *Age-Rank* always guesses the rank the real input held
most often in independent training rings. *Band-Center* guesses the member
nearest the midrange of the ring's ages. Rank χ² is against uniform (df 19;
above 36.19 is distinguishable at p = 0.01).

| route | real age | youngest | oldest | rank χ² | ID Age-Rank | ID Band-Center | ID Cluster-Fingerprint |
|---|---|---|---|---|---|---|---|
| node | 100 | 5.0% | 5.2% | 15 | 4.8% (r16) | 5.0% | 99.7% |
| node | 1000 | 5.5% | 4.9% | 25 | 5.1% (r17) | 4.7% | 99.7% |
| node | 5000 | 4.5% | 4.9% | 29 | 4.7% (r15) | 4.7% | 99.7% |
| node | 30000 | 5.1% | 5.0% | 16 | 4.5% (r13) | 4.7% | 99.7% |
| node | 100000 | 4.9% | 4.5% | 20 | 4.9% (r4) | 5.1% | 99.7% |
| node | 1000000 | 5.3% | 5.1% | 11 | 5.0% (r2) | 4.7% | 99.7% |
| node | gamma-spend | 5.2% | 4.8% | 29 | 5.0% (r3) | 5.0% | 99.8% |
| cli | 100 | 4.8% | 5.2% | 15 | 5.2% (r20) | 4.7% | 99.7% |
| cli | 1000 | 4.7% | 5.1% | 19 | 5.3% (r12) | 5.0% | 99.7% |
| cli | 5000 | 5.2% | 4.7% | 15 | 5.0% (r5) | 5.0% | 99.7% |
| cli | 30000 | 5.1% | 5.1% | 11 | 5.1% (r20) | 4.9% | 99.7% |
| cli | 100000 | 5.2% | 5.0% | 15 | 5.1% (r17) | 5.0% | 99.7% |
| cli | 1000000 | 5.2% | 5.1% | 22 | 5.0% (r16) | 5.0% | 99.7% |
| cli | gamma-spend | 4.9% | 5.0% | 8 | 4.9% (r18) | 5.0% | 99.7% |
| web | 100 | 4.7% | 4.9% | 15 | 4.9% (r15) | 4.9% | 5.0% |
| web | 1000 | 4.7% | 5.2% | 18 | 4.7% (r1) | 4.9% | 5.0% |
| web | 5000 | 5.5% | 5.1% | 23 | 5.5% (r1) | 5.1% | 5.0% |
| web | 30000 | 5.2% | 4.9% | 15 | 5.0% (r13) | 4.9% | 5.0% |
| web | 100000 | 5.0% | 4.9% | 18 | 4.8% (r17) | 5.0% | 5.0% |
| web | 1000000 | 4.5% | 5.1% | 14 | 5.0% (r11) | 4.9% | 5.0% |
| web | gamma-spend | 4.8% | 5.1% | 22 | 5.2% (r9) | 5.1% | 5.0% |
| web-pre-1735 (historical) | every age | 100.0% | 0.0% | 190000 | 100.0% (r1) | 0.0% | 5.0% |

With `--background-fraction 0.5` (half the outputs untagged), the node and CLI
cluster-fingerprint rates fall to 54–56%; the web rate stays at uniform
(4.7–5.0%). The age columns do not change.

### What changed in #1692, and what remains

1. **Age rank (fixed).** Before #1692 the node picker drew each slot's
   target age from a gamma with mean about 22k blocks, clamped it into a
   ±10% band centred on the real age and took the nearest candidate. Outside
   roughly 13k–33k blocks every target landed on the same band edge. The CLI
   and web pickers drew uniformly from the same centred band, so each decoy
   was younger than the real input with probability ½: the real input's rank
   was Binomial(19, ½) + 1, and the ring's age midrange estimated the real
   age (#1755). With the window position drawn at random, all 20 members are
   exchangeable given the window, so on a pool of locally uniform age
   density the real input's rank is uniform. All 21 production rows above
   pass the χ² test.
2. **Residual age signal.** When real ages follow the gamma spend model, an
   adversary who weights members by that prior (Age-Heuristic) reaches
   6.2–6.6% instead of 5.0%: decoys are uniform inside the window while the
   real age follows the prior. Pools whose age density changes within a
   window bias the rank slightly toward the denser side.
3. **Over-charge cost.** The window is still 20% wide, but the real input
   can sit at its young edge, so the `ring_elapsed_quantile@max` demurrage
   clock can read up to 1.20× the real age (1.10× before). Aggregate honest
   over-charge at ring 20: 1.0825 (centred band: 1.0855), inside the #314
   tolerance of 1.15; about 19% of individual honest spends exceed 1.15
   (`cluster-tax-sim decoy-quantile-sweep --ring-size 20
   --honest-jitter-bps 1000 --honest-band-offset`).
4. **Cluster tags (open, #1756).** `ClusterTagVector` outputs carry cluster
   ids and weights in plaintext. They are inherited from the real inputs
   only, and no picker matches decoys on tags.

## Legacy model, for comparison

The previous analysis rested on a cluster-similarity-aware picker (minimum
similarity 70%) over a gamma-aged pool. No producer implements that picker.

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy --route legacy -n 10000 --pool-size 100000 [--cluster-aware false]
```

| Adversary | Bits / ID (cluster-aware) | Bits / ID (`--cluster-aware false`) |
|---|---|---|
| Naive | 4.32 / 5.2% | 4.32 / 5.2% |
| Age-Heuristic | 4.26 / 3.6% | 4.27 / 3.5% |
| Cluster-Fingerprint | 4.11 / **58.7%** | 0.07 / **99.7%** (upper estimate: every output tagged) |
| Combined | 4.11 / 32.1% | 0.09 / 99.2% |

## Signature cost

`ring-size --sizes 11,16,20` gives CLSAG signatures of 448 B, 608 B and
**736 B** at ring sizes 11, 16 and 20. The layout is `c_zero` +
32 B × ring + key image + commitment key image
(`transaction/clsag/src/lib.rs:1052`). Ring members are carried separately.

## Recommendations (tracked as issues, not changed here)

- Done in #1692 (also closes #1755): random-offset age window in the node,
  CLI and web pickers; gamma blocks/day derived from the monetary policy
  block time.
- Stop publishing real-input-derived tags in plaintext, or make the pickers
  tag-aware: #1756. Related to #902 (committed amounts
  and tags) and #1687 (tag drop).

## References

- `cluster-tax/src/simulation/privacy_routes.rs`: production-route simulation
- `cluster-tax/src/simulation/privacy.rs`: legacy model and adversaries
- `botho/tests/privacy_sim_route_parity.rs`: port ↔ production parity
- `cluster-tax/src/simulation/constrained_analysis.rs`: models the retired
  2×/1.5× constraint; not production
- [Ring Signature Tag Propagation Design](./ring-signature-tag-propagation.md)
- Issues #245, #246 (historical), #1683 (epic), #1692, #1693, #1700, #1755, #1756
