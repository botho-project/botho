# Ring Signature Privacy Analysis

This document gives a quantitative privacy analysis of Botho's ring
signatures, measured against the decoy pickers that production actually runs.
It was regenerated for #1700 from `cluster-tax-sim` output; the exact commands
are listed below. The full tables, including the per-rank histograms, are in
[`cluster-tax/src/simulation/PRIVACY_ANALYSIS.md`](../../cluster-tax/src/simulation/PRIVACY_ANALYSIS.md).

> **Retraction.** An earlier version of this document recommended ring size
> 11 and analysed an age-ratio constraint of 2.0× and a factor-ratio
> constraint of 1.5×. None of these match the code:
>
> - Ring size is **20**: `MIN_RING_SIZE = DEFAULT_RING_SIZE = 20`
>   (`transaction/clsag/src/lib.rs:976-980`).
> - The age policy is a **±10%** band around the real input's age
>   (`AGE_SIMILARITY_SPREAD_BPS = 1_000`, `botho/src/decoy_selection.rs:269`).
> - A ±10% factor band exists (`FACTOR_SIMILARITY_SPREAD`,
>   `botho/src/decoy_selection.rs:330`), but only
>   `select_decoys_cluster_aware` uses it, and nothing in production calls
>   that function.
>
> No CLI command ever produced that version's numbers (2.76 bits, 21.3%
> identification, and so on), and they should not be cited. The text is
> still in git history.

## Executive summary

| producer | picker | how the real input is exposed |
|---|---|---|
| node ledger/wallet, detached signer | `select_decoys_for_input` | edge clamp (#1692): youngest in 100% of rings at ages 1k–5k, oldest in 100% at 100k–1M; midrange guess 85.3% under the gamma spend model |
| `botho-wallet` CLI | `select_rpc_decoy_pool` | the band is centred on the real age, so the real input is the median member: midrange guess about 50% (95% at age 100) |
| web wallet, Snap, claim links | `decoys.ts` `selectDecoysForInput` (#1735) | same centred band as the CLI (#1755): midrange guess 49–52% (94.7% at age 100). Before #1735 the chain-ordered window made it the youngest member in 100% of rings |
| node and CLI (outputs) | plaintext inherited cluster tags | cluster fingerprint 99.7% — upper estimate (every output tagged); about 55% when half are untagged. Web outputs carry empty tags (5.0%, uniform) |

A uniform guess identifies the real input 5% of the time (1 in 20). Every
production route does far worse than that against an adversary who knows the
picker. Against the node and CLI routes, the cluster-tag fingerprint alone is
close to decisive.

## Methodology

- **Pickers.** `cluster-tax/src/simulation/privacy_routes.rs` ports
  `GammaDecoySelector::select_decoys_for_input`,
  `select_rpc_decoys_from_history` / `select_rpc_decoy_pool`, and the web
  `decoys.ts` `selectDecoysForInput` (#1735).
  `botho/tests/privacy_sim_route_parity.rs` pins the two native ports to the
  production functions (same candidates and same seeded RNG give the same
  ring, including the #1786 tie-break shuffle in the node picker) and replays the rings #1735 recorded from the real `send.ts` with a
  seeded mulberry32 (`scripts/research/ct-economics/web-selection.json`)
  through the web port, which must reproduce them exactly. CI runs it in
  `workspace-build.yml`. The pre-#1735 chain-ordered window survives as the
  historical `--route web-pre-1735` (not in the production default).
- **Chain.** One output at every height 1..=1,200,000; real inputs at fixed
  ages, plus a row whose real ages follow the node picker's own gamma spend
  model (k = 19.28, θ = 1.61 days at 720 blocks/day).
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
nearest the midrange of the ring's ages.

| route | real age | youngest | oldest | ID Age-Rank | ID Band-Center | ID Cluster-Fingerprint |
|---|---|---|---|---|---|---|
| node | 100 | 0.0% | 0.0% | 100.0% (r10) | 50.0% | 99.8% |
| node | 1000 | 100.0% | 0.0% | 100.0% (r1) | 0.0% | 99.8% |
| node | 5000 | 100.0% | 0.0% | 100.0% (r1) | 0.0% | 99.6% |
| node | 30000 | 0.0% | 22.1% | 34.7% (r19) | 59.6% | 99.7% |
| node | 100000 | 0.0% | 100.0% | 100.0% (r20) | 0.0% | 99.7% |
| node | 1000000 | 0.0% | 100.0% | 100.0% (r20) | 0.0% | 99.7% |
| node | gamma-spend | 4.8% | 4.8% | 5.1% (r5) | 85.3% | 99.7% |
| cli | 100 | 0.0% | 0.0% | 49.9% (r10) | 95.1% | 99.7% |
| cli | 1000 | 0.0% | 0.0% | 18.1% (r11) | 53.4% | 99.7% |
| cli | 5000 | 0.0% | 0.0% | 17.8% (r11) | 49.7% | 99.8% |
| cli | 30000 | 0.0% | 0.0% | 18.0% (r10) | 50.9% | 99.7% |
| cli | 100000 | 0.0% | 0.0% | 18.3% (r10) | 50.3% | 99.7% |
| cli | 1000000 | 0.0% | 0.0% | 18.0% (r11) | 49.9% | 99.8% |
| cli | gamma-spend | 0.0% | 0.0% | 18.6% (r10) | 49.1% | 99.8% |
| web | 100 | 0.0% | 0.0% | 50.0% (r11) | 94.7% | 5.0% |
| web | 1000 | 0.0% | 0.0% | 18.8% (r11) | 52.3% | 5.0% |
| web | 5000 | 0.0% | 0.0% | 17.4% (r11) | 50.2% | 5.0% |
| web | 30000 | 0.0% | 0.0% | 17.0% (r10) | 49.6% | 5.0% |
| web | 100000 | 0.0% | 0.0% | 16.9% (r10) | 49.1% | 5.0% |
| web | 1000000 | 0.0% | 0.0% | 17.8% (r10) | 49.5% | 5.0% |
| web | gamma-spend | 0.0% | 0.0% | 17.5% (r10) | 49.9% | 5.0% |
| web-pre-1735 (historical) | every age | 100.0% | 0.0% | 100.0% (r1) | 0.0% | 5.0% |

With `--background-fraction 0.5` (half the outputs untagged), the node and CLI
cluster-fingerprint rates fall to 54–56%; the web rate stays at uniform
(4.9–5.1%). The age columns do not change.

### Why each route leaks

1. **Web.** #1735 (#1693) replaced the chain-ordered window (no age band, no
   randomness, real input always the youngest) with a ±10% band picker
   using a CSPRNG. As expected it now lands on the `cli` row: the
   centred-band median leak below, not the old youngest-member leak.
2. **Node.** Each slot's target age is drawn from a gamma with mean about
   22k blocks and then clamped into the band. Outside roughly 13k–33k
   blocks, every target lands on the same band edge (#1692).
3. **Centred band (node, CLI and web).** The ±10% band is
   centred on the real age. Any symmetric in-band distribution makes each
   decoy younger than the real input with probability ½. The real input's
   rank is then Binomial(19, ½) + 1, and the ring's age midrange estimates
   the real age. A uniform age rank, which is what #1692's acceptance
   criterion asks for, cannot be reached while the band stays centred on the
   real input.
4. **Cluster tags.** `ClusterTagVector` outputs carry cluster ids and
   weights in plaintext. They are inherited from the real inputs only, and
   no picker matches decoys on tags.

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

- Fix the node edge clamp: #1692. Note that its uniform-rank acceptance
  criterion needs a band that is *not* centred on the real input.
- Stop centring the age band on the real input. One option is to draw the
  band position at random, with the real input placed uniformly inside it,
  while still bounding the `@max` over-charge: #1755.
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
