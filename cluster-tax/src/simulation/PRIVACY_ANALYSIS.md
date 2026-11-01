# Ring Signature Privacy Simulation Results

This document reports what the privacy simulation measures for the ring
producers Botho actually ships. Every number below is CLI output from
`cluster-tax-sim`, and the exact commands are listed with the results. Last
regenerated for #1692 / #1755 (random-offset age window) at ring size **20**
(`MIN_RING_SIZE`, `transaction/clsag/src/lib.rs:980`) with a **20%-wide** age
window (`AGE_SIMILARITY_SPREAD_BPS` = ±10%, `botho/src/decoy_selection.rs`).

> **Headline (after #1692): the age leaks are closed on all three routes;
> plaintext cluster tags remain the dominant leak.** Every picker now draws
> one random-offset age window per ring (`draw_age_similarity_window`): the
> window is 20% of the real age wide, the real input's age is uniform inside
> it, and decoys are a uniform sample of the window.
>
> - **Age rank** is uniform on every route at every reported age: rank
>   χ² 8–29 (uniform below 36.19 at p = 0.01), youngest/oldest rates
>   4.5–5.5%, and the picker-aware Age-Rank and Band-Center adversaries score
>   4.5–5.5% (uniform guess: 5.0%).
> - **Before #1692** the node picker clamped a gamma target into a band
>   centred on the real age, so the real input was the youngest ring member
>   in 100% of rings at ages 1k/5k and the oldest in 100% at 100k/1M
>   (Band-Center 85.3% even with gamma-distributed real ages). The CLI and web
>   pickers sampled the centred band uniformly, making the real input the
>   median member (Band-Center ~50%, ~95% at age 100; #1755).
> - **Residual age signal:** when real ages follow the gamma spend model, the
>   Age-Heuristic adversary (weights members by the gamma pdf) reaches
>   6.2–6.6% instead of 5.0%. Inside one window the decoys are uniform while
>   the real age follows the spend prior, so an adversary who knows the prior
>   gains slightly. Any uniform in-window picker has this.
> - **Cluster tags** (unchanged): the native wallets stamp outputs with the
>   real input's tags in plaintext, and no picker looks at tags. Cluster
>   fingerprinting identifies the real input in **~99.7%** of node and CLI
>   rings — an **upper estimate (every output tagged)**. With half the
>   outputs untagged it is still ~55%. The web signer publishes empty output
>   tags, so this adversary gets 5.0% (uniform) there.
>
> Follow-ups: #1756 (plaintext inherited tags). The over-charge cost of the
> random offset is in "Reading the results" below.

## What is simulated

`cluster-tax/src/simulation/privacy_routes.rs` drives ports of the three
production decoy pickers. `cluster-tax` cannot depend on `botho` or
`botho-wallet` (both depend on it), so the native pickers are ported.
`botho/tests/privacy_sim_route_parity.rs` pins each port to the real function:
identical candidates and identically seeded RNGs must give identical rings.
CI runs it (and the `privacy_routes` unit tests) in `workspace-build.yml`.

| route  | production code | used by |
|--------|-----------------|---------|
| `node` | `GammaDecoySelector::select_decoys_for_input` (`botho/src/decoy_selection.rs:856`) | `Ledger::get_decoy_outputs_for_input`, node wallet, detached signer |
| `cli`  | `select_rpc_decoys_from_history` → `select_rpc_decoy_pool` (`botho-wallet/src/ring_builder.rs:166`) | `botho-wallet` thin CLI wallet |
| `web`  | `selectDecoysForInput` (`web/packages/wasm-signer/src/decoys.ts:227`, called per input at `send.ts:655`) | web wallet, MetaMask Snap, claim links |
| `web-pre-1735` | historical chain-ordered `decoyPool[(i*19 + j) % len]` window (`send.ts:657` at `596f64c0`, removed by #1735) | nothing — comparison only, not in `--route production` |

**Node port provenance.** The `node` port is a literal copy of
`select_decoys_for_input`: #1786's single shuffle of the eligible set, then
one `draw_age_similarity_window` draw, then the first 19 in-window members of
the shuffled set (nearest-age fill if the window is thin). The parity test
compares rings exactly. Replaying the shuffle in the simulation would mean
permuting all 1.2M eligible outputs for every ring, so when the window fills
the ring the simulation draws the window the same way and takes a sparse
partial Fisher–Yates sample of it. Both are a uniform ordered sample of the
window, so the fast path is distribution-equivalent to the literal port, not
bit-identical; a seeded statistical unit test checks per-slot pick
frequencies against the literal port within 5σ. The CLI route uses the same
kind of fast path (its literal port is pinned exactly by the parity test).

**Web port provenance.** The `web` port mirrors `decoys.ts` step for step:
10-block confirmation floor, `drawAgeSimilarityWindow` (offset =
`randomInteger(W + 1)`, same integer bps math as the Rust window), partial
Fisher–Yates over the in-window candidates, nearest-age fill (shuffle, then
stable sort by distance) when the window is thin, final ring shuffle, every
draw via `randomIndex`'s u32 rejection sampling over an injected u32 source.
The parity test replays all 16 scenarios of
`scripts/research/ct-economics/web-selection.json` — the rings
`ct-economics-selection.test.ts` recorded from the real `send.ts` with
`crypto.getRandomValues` stubbed by a seeded mulberry32, re-recorded for
#1692 — through the port with the same mulberry32 seeds and requires
identical rings, order included; it also checks the shared
`test/fixtures/decoy-age-band.json` band and window vectors. The simulation
drives the same port from ChaCha8 u32 draws. When the window alone fills the
ring (every reported age, including 100 on this chain: 21 window ages, 20
outputs besides the real input) it uses a sparse-swap fast path that is
bit-identical to the literal port; for thinner windows it uses a
distribution-equivalent nearest-age fill (unit-tested against the literal
port).

**Chain model.** The UTXO set has one output at every height
1..=1,200,000, so it covers every reported age densely, at the locally
uniform age density the uniform-rank result relies on. The legacy model used
a gamma-aged pool instead. Cluster-tag
profiles use the legacy type mix (50% standard, 25% exchange, 10% whale, 10%
coinbase, 5% mixed). That mix tags *every* output. `--background-fraction`
blanks a share of them.

**Adversaries.** Each column is an *argmax identification rate*: how often
the adversary's single top suspect is the real input, with ties split. A
uniform guess scores exactly 5.0%.

- *Naive*: a uniform guess.
- *Age-Heuristic*: the gamma spend pdf (k = 19.28, θ = 1.61 days at
  17,280 blocks/day since #1692; it used to assume 720 blocks/day).
- *Age-Rank*: knows the picker. It always guesses the age rank that the real
  input held most often in an independent set of training rings.
- *Band-Center*: guesses the member closest to the midrange of the ring's
  ages. It targets pickers that centre the band on the real age (all three
  did before #1692); against the random-offset window the midrange estimates
  the window's centre instead.
- *Cluster-Fingerprint*: matches each member's tags to the output's
  published tags.
- *Combined*: age-heuristic × cluster, as in the legacy model.

## Results

Command (about 10 s on a laptop):

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000
# defaults: --route production --real-ages 100,1000,5000,30000,100000,1000000,gamma
#           --chain-height 1200000 --outputs-per-block 1 --seed 1700 --background-fraction 0.0
```

Output, verbatim:

### Production decoy routes: real-input identifiability

Ring size 20 (1 real + 19 decoys). Chain model: 1 outputs/block at heights 1..=1200000 (UTXO ages 0..=1199999). 10000 evaluation rings + 10000 training rings per row, seed 1700. Background (empty-tag) output fraction: 0%.

ID = argmax identification rate (adversary's single top suspect is the real input; ties split; uniform = 5.0%). Age rank 1 = youngest ring member. χ² is the real input's age-rank histogram against uniform (df 19); above 36.19 it is distinguishable from uniform at p = 0.01.

#### Route `node`

Ports: GammaDecoySelector::select_decoys_for_input (botho/src/decoy_selection.rs) — node ledger/wallet and detached signer.
Outputs carry the real input's tags decayed one hop (plaintext).

| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 10000 | 0 | 5.0% | 5.2% | 15 | 5.0% | 5.2% | 4.8% (r16) | 5.0% | 99.7% | 98.9% |
| 1000 | 10000 | 0 | 5.5% | 4.9% | 25 | 5.0% | 4.9% | 5.1% (r17) | 4.7% | 99.7% | 99.0% |
| 5000 | 10000 | 0 | 4.5% | 4.9% | 29 | 5.0% | 4.9% | 4.7% (r15) | 4.7% | 99.7% | 99.2% |
| 30000 | 10000 | 0 | 5.1% | 5.0% | 16 | 5.0% | 5.0% | 4.5% (r13) | 4.7% | 99.7% | 98.9% |
| 100000 | 10000 | 0 | 4.9% | 4.5% | 20 | 5.0% | 4.5% | 4.9% (r4) | 5.1% | 99.7% | 99.2% |
| 1000000 | 10000 | 0 | 5.3% | 5.1% | 11 | 5.0% | 5.3% | 5.0% (r2) | 4.7% | 99.7% | 99.1% |
| gamma-spend | 10000 | 0 | 5.2% | 4.8% | 29 | 5.0% | 6.6% | 5.0% (r3) | 5.0% | 99.8% | 99.6% |

#### Route `cli`

Ports: select_rpc_decoys_from_history -> select_rpc_decoy_pool (botho-wallet/src/ring_builder.rs) — thin CLI wallet.
Outputs carry the real input's tags decayed one hop (plaintext).

| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 10000 | 0 | 4.8% | 5.2% | 15 | 5.0% | 5.2% | 5.2% (r20) | 4.7% | 99.7% | 99.0% |
| 1000 | 10000 | 0 | 4.7% | 5.1% | 19 | 5.0% | 5.1% | 5.3% (r12) | 5.0% | 99.7% | 98.9% |
| 5000 | 10000 | 0 | 5.2% | 4.7% | 15 | 5.0% | 4.7% | 5.0% (r5) | 5.0% | 99.7% | 99.0% |
| 30000 | 10000 | 0 | 5.1% | 5.1% | 11 | 5.0% | 5.1% | 5.1% (r20) | 4.9% | 99.7% | 99.2% |
| 100000 | 10000 | 0 | 5.2% | 5.0% | 15 | 5.0% | 5.0% | 5.1% (r17) | 5.0% | 99.7% | 99.1% |
| 1000000 | 10000 | 0 | 5.2% | 5.1% | 22 | 5.0% | 5.2% | 5.0% (r16) | 5.0% | 99.7% | 99.0% |
| gamma-spend | 10000 | 0 | 4.9% | 5.0% | 8 | 5.0% | 6.2% | 4.9% (r18) | 5.0% | 99.7% | 99.6% |

#### Route `web`

Ports: selectDecoysForInput (web/packages/wasm-signer/src/decoys.ts, #1735) — web wallet, Snap, claim links.
Outputs carry empty cluster tags.

| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 10000 | 0 | 4.7% | 4.9% | 15 | 5.0% | 4.9% | 4.9% (r15) | 4.9% | 5.0% | 4.9% |
| 1000 | 10000 | 0 | 4.7% | 5.2% | 18 | 5.0% | 5.2% | 4.7% (r1) | 4.9% | 5.0% | 5.2% |
| 5000 | 10000 | 0 | 5.5% | 5.1% | 23 | 5.0% | 5.1% | 5.5% (r1) | 5.1% | 5.0% | 5.1% |
| 30000 | 10000 | 0 | 5.2% | 4.9% | 15 | 5.0% | 4.9% | 5.0% (r13) | 4.9% | 5.0% | 4.9% |
| 100000 | 10000 | 0 | 5.0% | 4.9% | 18 | 5.0% | 4.9% | 4.8% (r17) | 5.0% | 5.0% | 4.9% |
| 1000000 | 10000 | 0 | 4.5% | 5.1% | 14 | 5.0% | 4.5% | 5.0% (r11) | 4.9% | 5.0% | 4.5% |
| gamma-spend | 10000 | 0 | 4.8% | 5.1% | 22 | 5.0% | 6.5% | 5.2% (r9) | 5.1% | 5.0% | 6.5% |

#### Real-input age-rank histogram (% of rings at each rank)

| route | real age | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| node | 100 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| node | 1000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| node | 5000 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| node | 30000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| node | 100000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| node | 1000000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| node | gamma-spend | 5 | 5 | 5 | 5 | 5 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 6 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 100 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 1000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 5000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 30000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 100000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 1000000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | gamma-spend | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | 100 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | 1000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | 5000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | 30000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | 100000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | 1000000 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| web | gamma-spend | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |

A low ID rate in one column does not mean the real input is safe; read
the row's maximum. On the native routes that maximum is the
Cluster-Fingerprint column (#1756), not age.

### Historical comparison: the pre-#1735 web route

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000 --route web-pre-1735
```

Output (route table and histogram; the header paragraphs are identical to
the run above):

#### Route `web-pre-1735`

Ports: HISTORICAL chain-ordered decoyPool[(i*19+j) % len] window that web/packages/wasm-signer/src/send.ts used before #1735 — no longer run.
Outputs carry empty cluster tags.

| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 5.0% | 0.0% |
| 1000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 5.0% | 0.0% |
| 5000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 5.0% | 0.0% |
| 30000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 5.0% | 0.0% |
| 100000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 5.0% | 0.0% |
| 1000000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |
| gamma-spend | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |

#### Real-input age-rank histogram (% of rings at each rank)

| route | real age | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| web-pre-1735 | 100 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web-pre-1735 | 1000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web-pre-1735 | 5000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web-pre-1735 | 30000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web-pre-1735 | 100000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web-pre-1735 | 1000000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web-pre-1735 | gamma-spend | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Before #1735 every web ring used the chain's 19 oldest outputs, so the real
input was always the youngest member and every web ring shared the same
decoys. No production code runs this route any more.

### Sensitivity: half the outputs untagged

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000 --background-fraction 0.5 --real-ages 1000,30000,gamma
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000 --background-fraction 0.5 --real-ages 1000,30000,gamma --route web-pre-1735
```

| route | real age | ID Cluster-Fingerprint (0% background) | ID Cluster-Fingerprint (50% background) |
|---|---|---|---|
| node | 1000 | 99.7% | 54.1% |
| node | 30000 | 99.7% | 55.1% |
| node | gamma-spend | 99.8% | 55.2% |
| cli | 1000 | 99.7% | 55.4% |
| cli | 30000 | 99.7% | 55.2% |
| cli | gamma-spend | 99.7% | 55.2% |
| web | 1000 | 5.0% | 4.7% |
| web | 30000 | 5.0% | 5.0% |
| web | gamma-spend | 5.0% | 5.0% |
| web-pre-1735 | 1000 | 5.0% | 6.2% |
| web-pre-1735 | 30000 | 5.0% | 6.4% |
| web-pre-1735 | gamma-spend | 5.0% | 6.3% |

The age-rank, Age-Rank and Band-Center columns do not change; they use ages
only. The web signer publishes empty output tags, so the cluster adversary
learns nothing there: it scores every empty-tagged ring member 1.0 and every
tagged one 0.5, so its guess is uniform among the blanked members. With
random decoys (`web`) that averages to 5.0%. The pre-#1735 window always used
the same 19 decoys, a fixed number of which happen to be blanked, so its
tie-split share drifts to 6.2–6.4%. That is an artefact of the fixed decoy
set, not a leak in the web signer. The empty tags are themselves the
tag-drop problem tracked in #1687.

### Reading the results

1. **The random-offset window closes the age-rank leaks (#1692, #1755).**
   Before #1692 the node picker clamped a gamma target into a band centred
   on the real age and took the nearest candidate, so outside the gamma bulk
   every decoy sat on one side of the real input (youngest in 100% of rings
   at 1k/5k, oldest in 100% at 100k/1M; Band-Center 85.3% with
   gamma-distributed real ages). The CLI and web pickers drew uniformly from
   the centred band, so the real input's rank was Binomial(19, ½) + 1, the
   median member (Band-Center ~50% at 1k–1M, ~95% at age 100). Now every
   picker draws the window offset uniformly, so given the window all 20 ring
   members are exchangeable: on a pool of locally uniform age density the
   real input's rank is exactly uniform (a hypergeometric count with a
   uniform parameter), which the tables confirm on every route at every age.
   The one behavioural difference between `web`/`node` and `cli` — they fill
   a thin window with nearest-age outputs where the CLI refuses — never
   triggers at the reported ages on this chain.
2. **What the random offset costs: the `@max` over-charge tail.** The window
   is still 20% of the real age wide, but the real input can now sit at its
   young edge, so the oldest ring member (which drives the
   `ring_elapsed_quantile@max` demurrage clock) can be up to 1.20× the real
   age instead of 1.10×. Measured with `cluster-tax-sim decoy-quantile-sweep
   --ring-size 20 --honest-jitter-bps 1000 --honest-band-offset`: the
   aggregate honest over-charge (the #314 metric) is **1.0825**, against
   1.0855 for the centred band, inside the 1.15 tolerance. Per spend, the
   worst case rises from 1.1000 to 1.1985, and about 19% of honest spends are
   over-charged by more than 15%. Pools whose age density varies within the
   window (fast chain growth) bias the rank slightly toward the denser side.
3. **Plaintext inherited cluster tags fingerprint the real input (#1756).**
   This is also new from #1700. `Wallet::compute_inherited_tags`
   (`botho/src/wallet.rs:695`) and `inherited_cluster_tags`
   (`botho-wallet/src/transaction.rs:465`) stamp every output with the real
   input's tags decayed 5%. Decoys never contribute, and none of the pickers
   looks at tags. Cluster-aware selection (`select_decoys_cluster_aware`,
   including the ±10% factor band `factor_similarity_band`) exists in
   `botho/src/decoy_selection.rs` but has **no production caller**. The
   ~99.7% rate is an upper estimate (every output tagged); see the
   sensitivity table for the dilution.
4. **Bits are not the metric to quote.** The legacy report below shows 4.11
   of 4.32 bits ("95%") against cluster fingerprinting while the same
   adversary's ID rate is 58.7%. Entropy averages over the whole distribution,
   so a minority of fully exposed rings barely moves the mean.

## Legacy model (not run by any producer)

`RingSimulator` in `cluster-tax/src/simulation/privacy.rs` models global-gamma
age weighting plus cluster-similarity-aware decoy selection (minimum
similarity 70%), over a 100k-output gamma-aged pool. It is kept only for
comparison. `--cluster-aware` now really toggles: it was a `bool` with
`default_value = "true"` and an implicit set-true action, so it could not be
turned off before #1700.

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy --route legacy -n 10000 --pool-size 100000 [--cluster-aware false]
```

| Adversary | Bits (cluster-aware) | ID rate (cluster-aware) | Bits (`--cluster-aware false`) | ID rate (`--cluster-aware false`) |
|---|---|---|---|---|
| Naive | 4.32 | 5.2% | 4.32 | 5.2% |
| Age-Heuristic | 4.26 | 3.6% | 4.27 | 3.5% |
| Cluster-Fingerprint | 4.11 | **58.7%** | 0.07 | **99.7%** |
| Combined | 4.11 | 32.1% | 0.09 | 99.2% |

Theoretical maximum: 4.32 bits (log₂ 20). Even the hypothetical
cluster-aware design leaves the cluster adversary a 58.7% ID rate. The
production-like tag-blind configuration (`--cluster-aware false`) matches the
production-route result: about 99.7%, an upper estimate (every output tagged)
since both use the synthetic all-tagged profile mix.

## Ring size and signature cost

```bash
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    ring-size --sizes 11,16,20
```

| Ring size | Theoretical max | CLSAG signature |
|---|---|---|
| 11 | 3.46 bits | 448 B |
| 16 | 4.00 bits | 608 B |
| 20 | 4.32 bits | **736 B** |

Signature bytes are `c_zero (32) || responses (32 × ring) || key_image (32) ||
commitment_key_image (32)`, the production layout documented at
`transaction/clsag/src/lib.rs:1052`. This document previously said
"62 KB". The sim's own formula said 704 B, because it double-counted the real <!-- stale-literal-ok: quotes the pre-#1700 claim being corrected -->
response and omitted both key images; #1700 corrected it. Ring members (key,
public key, commitment) are carried separately from the signature. The
legacy `--simulate` column is not reproduced here because it measures the
cluster-aware model.

## Monero comparison: withdrawn

An earlier version claimed cluster-aware selection beats Monero's
OSPEAD-measured ring-16 efficiency. That compared a hypothetical picker with
measured data. The pickers Botho actually runs are, by the tables above,
easier to attack than the Monero numbers suggest, so the comparison has been
removed.

## Committed Tag Vector Design Decision

### Cluster ID Visibility (v1)

**Decision**: Cluster IDs are **visible**, tag masses are **hidden** in commitments.

```
CommittedTagVector:
├── entries: Vec<CommittedTagMass>
│   ├── cluster_id: ClusterId       // VISIBLE - used for decoy selection
│   └── commitment: CompressedRistretto  // HIDDEN - mass in Pedersen commitment
└── total_commitment: CompressedRistretto  // HIDDEN - total attributed mass
```

### Rationale

**Why visible cluster IDs?**

1. **Decoy selection requirement**: Ring signatures require selecting decoys that appear plausible. With hidden cluster IDs, the wallet cannot determine which outputs are similar.

2. **Simpler implementation**: Visible IDs allow straightforward set-overlap similarity metrics without requiring zero-knowledge proofs for membership.

3. **Acceptable privacy cost**: Cluster IDs reveal *which* clusters an output is associated with, but not *how much* value is attributed to each. This is analogous to revealing transaction graph structure (which cryptocurrencies already do) while hiding amounts.

4. **Future extensibility**: Full cluster ID hiding is possible with fixed-size padding and more complex proofs, but adds significant complexity for marginal privacy gains.

### Privacy Implications

**Information leaked by visible cluster IDs:**

| Information | Leaked | Hidden |
|-------------|--------|--------|
| Which clusters output is associated with | ✓ | |
| Mass/weight per cluster | | ✓ |
| Total attributed mass | | ✓ |
| Background (unattributed) portion | | ✓ |

**Attack surface:**

- **Cluster set fingerprinting**: Adversary can identify outputs with identical cluster ID sets
- **Linkability by cluster set size**: Outputs with unusual numbers of clusters may be distinguishable
- **Mitigation (design only)**: decoy selection that prioritizes cluster-set overlap would reduce fingerprinting. No production picker does this yet (#1700). The shipped `ClusterTagVector` outputs carry cluster ids **and** weights in plaintext.

### Decoy Selection with Committed Tags

With visible cluster IDs, decoy selection uses **Jaccard similarity** on cluster ID sets:

```
similarity(A, B) = |clusters(A) ∩ clusters(B)| / |clusters(A) ∪ clusters(B)|
```

This replaces the weighted cosine similarity used with plaintext tags (which required mass values).

**Selection algorithm:**
1. Extract cluster ID set from real output
2. Filter pool to outputs with Jaccard similarity ≥ 70%
3. Weight candidates by age distribution (gamma PDF)
4. Sample decoys using weighted random selection

### Quantified Privacy Impact

> **Unverified (#1700):** no `cluster-tax-sim` command produces the table
> below. Its baseline (4.10 bits / 32.6%) is the legacy cluster-aware model,
> which no producer runs. The production plaintext-tag fingerprint rate is
> ~99.7% as an upper estimate (every output tagged; ~55% with half untagged —
> see Results). Treat these numbers as historical until a
> committed-tag run is added to the CLI.

Historical claim, from a simulation of committed tag vectors:

| Metric | Plaintext Tags | Committed Tags | Delta |
|--------|----------------|----------------|-------|
| Effective anonymity | 17.2 | 16.8 | -2.3% |
| Bits of privacy | 4.10 | 4.07 | -0.7% |
| ID rate (Combined) | 32.6% | 34.1% | +1.5% |

**Analysis**: Committed tags provide nearly identical privacy to plaintext tags because:
- Cluster set overlap is the dominant factor in adversary analysis
- Mass values provide minimal additional distinguishing information
- Age heuristics are unchanged

### Future Work: Full Cluster ID Hiding

For maximum privacy, cluster IDs could also be hidden:

1. **Fixed vector size**: All outputs have 8 cluster slots
2. **Zero-mass commitments**: Unused slots commit to 0 with random IDs
3. **Wallet decoy hints**: Wallet provides hint about which decoys are compatible
4. **Anonymized statistics**: Node maintains cluster distribution without specific IDs

This would eliminate cluster set fingerprinting but requires:
- ~2x commitment size per output
- Wallet-node coordination protocol
- More complex range proofs
