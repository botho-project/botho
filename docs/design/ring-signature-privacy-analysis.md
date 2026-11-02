# Ring Signature Privacy Analysis

This document gives a quantitative privacy analysis of Botho's ring
signatures, measured against the decoy pickers that production actually runs.
It was regenerated for #1692 / #1755 (random-offset age window, ±5% since the
2026-11-02 operator ruling) from
`cluster-tax-sim` output; the exact commands are listed below. The full tables, including the per-rank histograms, are in
[`cluster-tax/src/simulation/PRIVACY_ANALYSIS.md`](../../cluster-tax/src/simulation/PRIVACY_ANALYSIS.md).

> **Retraction.** An earlier version of this document recommended ring size
> 11 and analysed an age-ratio constraint of 2.0× and a factor-ratio
> constraint of 1.5×. None of these match the code:
>
> - Ring size is **20**: `MIN_RING_SIZE = DEFAULT_RING_SIZE = 20`
>   (`transaction/clsag/src/lib.rs:976-980`).
> - The age policy is a window **10%** of the real input's age wide
>   (`AGE_SIMILARITY_SPREAD_BPS = 500`, ±5%, `botho/src/decoy_selection.rs`).
>   Since #1692 its position is drawn at random per ring
>   (`draw_age_similarity_window`); before that it was a ±10% band centred
>   on the real input.
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
window is 10% of the real age wide, the real input's age is uniform inside
it, and decoys are a uniform sample of it. The node, CLI and web pickers use
the same rule. Where the window holds fewer than 19 decoys (young inputs on
a sparse chain: below about 200 blocks at 1 output per block) the node and
web pickers fill by nearest age, which centres the ring on the real input,
and the CLI refuses the ring.

| producer | picker | how the real input is exposed |
|---|---|---|
| node ledger/wallet, detached signer | `select_decoys_for_input` | by age: not measurably from age 1,000 (rank χ² 11–29, every age-based adversary 4.4–5.4%; Age-Heuristic 5.7% when real ages follow the gamma spend model). At age 100 on this chain the window is thin and the nearest-age fill makes the real input the median member (Band-Center 50%). Before #1692: youngest in 100% of rings at 1k–5k, oldest in 100% at 100k–1M (edge clamp) |
| `botho-wallet` CLI | `select_rpc_decoy_pool` | by age: not measurably from age 1,000 (rank χ² 7–22). At age 100 on this chain it refuses every ring (thin window, no fill). Before #1692: the median member of a centred band, midrange guess about 50% (95% at age 100) |
| web wallet, Snap, claim links | `decoys.ts` `selectDecoysForInput` | by age: not measurably from age 1,000 (rank χ² 15–27). At age 100, the same thin-window median as the node. Before #1692: same centred-band median leak as the CLI (#1755); before #1735, the youngest member in 100% of rings |
| node and CLI (outputs) | plaintext inherited cluster tags | cluster fingerprint 99.7% — upper estimate (every output tagged); about 55% when half are untagged. Web outputs carry empty tags (5.0%, uniform) |

A uniform guess identifies the real input 5% of the time (1 in 20). Against
the age-based adversaries, every production route is now at that level
wherever the window fills the ring.
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
| node | 100 | 0.0% | 0.0% | 90001 | 50.2% (r11) | 50.0% | 99.7% |
| node | 1000 | 5.4% | 4.8% | 24 | 5.4% (r1) | 4.6% | 99.7% |
| node | 5000 | 4.4% | 4.8% | 29 | 4.7% (r13) | 4.8% | 99.7% |
| node | 30000 | 5.0% | 5.0% | 14 | 4.6% (r13) | 4.7% | 99.8% |
| node | 100000 | 4.8% | 4.5% | 22 | 5.3% (r10) | 5.1% | 99.7% |
| node | 1000000 | 5.2% | 5.1% | 11 | 4.9% (r4) | 4.8% | 99.7% |
| node | gamma-spend | 5.1% | 4.8% | 29 | 5.0% (r3) | 4.9% | 99.8% |
| cli | 100 | refused (10000 of 10000 rings) | – | – | – | – | – |
| cli | 1000 | 5.0% | 5.0% | 20 | 5.1% (r15) | 4.8% | 99.7% |
| cli | 5000 | 5.2% | 4.7% | 22 | 5.3% (r5) | 5.2% | 99.7% |
| cli | 30000 | 5.1% | 5.1% | 11 | 5.1% (r9) | 5.0% | 99.8% |
| cli | 100000 | 5.1% | 5.0% | 15 | 5.1% (r17) | 5.0% | 99.8% |
| cli | 1000000 | 5.2% | 5.0% | 21 | 4.8% (r19) | 5.1% | 99.7% |
| cli | gamma-spend | 4.9% | 5.0% | 7 | 5.0% (r20) | 5.0% | 99.7% |
| web | 100 | 0.0% | 0.0% | 90002 | 49.8% (r11) | 50.0% | 5.0% |
| web | 1000 | 5.1% | 5.4% | 15 | 5.2% (r12) | 5.2% | 5.0% |
| web | 5000 | 4.8% | 5.0% | 27 | 4.9% (r5) | 4.8% | 5.0% |
| web | 30000 | 5.3% | 5.3% | 25 | 5.0% (r7) | 5.6% | 5.0% |
| web | 100000 | 5.2% | 5.2% | 21 | 4.9% (r15) | 5.2% | 5.0% |
| web | 1000000 | 4.9% | 5.0% | 20 | 4.8% (r2) | 5.1% | 5.0% |
| web | gamma-spend | 5.0% | 5.2% | 19 | 4.7% (r19) | 4.8% | 5.0% |
| web-pre-1735 (historical) | every age | 100.0% | 0.0% | 190000 | 100.0% (r1) | 0.0% | 5.0% |

With `--background-fraction 0.5` (half the outputs untagged), the node and CLI
cluster-fingerprint rates fall to 54–56%; the web rate stays at uniform
(5.0–5.1%). The age columns do not change.

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
   density the real input's rank is uniform. All 18 production rows from age
   1,000 pass the χ² test. At age 100 on this 1-output-per-block chain the
   ±5% window spans 11 ages and holds only 10 decoys: the node and web
   pickers fill by nearest age, so the real input is the median member in
   every ring, and the CLI refuses the ring. A window fills the ring once
   `(2 × floor(age / 20) + 1) × outputs per block ≥ 20` (about 200 blocks at
   1 output per block; the ±10% window filled from about 100).
2. **Residual age signal.** When real ages follow the gamma spend model, an
   adversary who weights members by that prior (Age-Heuristic) reaches
   5.4–5.7% instead of 5.0% (6.2–6.6% with the ±10% window): decoys are
   uniform inside the window while the real age follows the prior. Pools
   whose age density changes within a window bias the rank slightly toward
   the denser side.
3. **Over-charge and the ±5% width.** With a random offset the real input
   can sit at the window's young edge, so the `ring_elapsed_quantile@max`
   demurrage clock can read up to `1 + 2 × spread` times the real age. At the
   old ±10% width that is 1.20×: aggregate honest over-charge 1.0825 at ring
   20, but a worst single spend of 1.1985 and 19.2% of honest spends above
   the #314 tolerance of 1.15. The operator ruling of 2026-11-02 narrowed the
   window to ±5%: aggregate **1.0413**, worst single spend **1.0992**, 0% of
   spends above 1.15 (`cluster-tax-sim decoy-quantile-sweep --ring-size 20
   --honest-jitter-bps 500 --honest-band-offset`). Every spend is capped at
   1.10× by construction.
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
