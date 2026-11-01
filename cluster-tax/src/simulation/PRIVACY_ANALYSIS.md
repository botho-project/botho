# Ring Signature Privacy Simulation Results

This document reports what the privacy simulation measures for the ring
producers Botho actually ships. Every number below is CLI output from
`cluster-tax-sim`, and the exact commands are listed with the results. Last
regenerated for #1700 (after #1735 and #1786 merged) at ring size **20**
(`MIN_RING_SIZE`, `transaction/clsag/src/lib.rs:980`) with the **±10%** age
band (`AGE_SIMILARITY_SPREAD_BPS`, `botho/src/decoy_selection.rs:269`).

> **Headline: deployed ring privacy is far below the "≈95% efficiency" this
> document used to claim.** The old number came from a model of
> cluster-aware decoy selection, which no wallet implements, and it reported
> entropy (bits) instead of how often the real input is singled out. Measured
> against the real pickers:
>
> - **node** (`select_decoys_for_input`): the real input's age rank is fixed
>   (youngest in 100% of rings at ages 1k/5k, oldest in 100% at 100k/1M).
>   Even when real ages follow the picker's own gamma model, the member
>   nearest the ring's age midrange is the real input **85.3%** of the time.
> - **CLI wallet** (`select_rpc_decoy_pool`) and **web/Snap/claim links**
>   (`decoys.ts` `selectDecoysForInput`, #1735): decoys are uniform within the
>   band, but the band is *centred* on the real age. The real input is
>   therefore the median-age member, and the midrange guess identifies it in
>   **~50%** of rings (cli 49.1–53.4%, web 49.1–52.3% at ages 1k–1M and
>   gamma; ~95% at age 100). #1735 removed the pre-#1735 web leak (the real
>   input was the youngest member in 100% of rings) but the web route now
>   shares the centred-band leak (#1755).
> - **Cluster tags**: the native wallets stamp outputs with the real input's
>   tags in plaintext, and no picker looks at tags. Cluster fingerprinting
>   identifies the real input in **~99.7%** of node and CLI rings — an
>   **upper estimate (every output tagged)**: the legacy synthetic profile mix
>   tags every output. With half the outputs untagged it is still ~55%. The
>   web signer publishes empty output tags, so this adversary gets 5.0%
>   (uniform) there.
>
> Follow-ups: #1692 (node edge clamp), #1755 (centred band — node, CLI and
> web), #1756 (plaintext inherited tags).

## What is simulated

`cluster-tax/src/simulation/privacy_routes.rs` drives ports of the three
production decoy pickers. `cluster-tax` cannot depend on `botho` or
`botho-wallet` (both depend on it), so the native pickers are ported.
`botho/tests/privacy_sim_route_parity.rs` pins each port to the real function:
identical candidates and identically seeded RNGs must give identical rings.
CI runs it (and the `privacy_routes` unit tests) in `workspace-build.yml`.

| route  | production code | used by |
|--------|-----------------|---------|
| `node` | `GammaDecoySelector::select_decoys_for_input` (`botho/src/decoy_selection.rs:755`) | `Ledger::get_decoy_outputs_for_input`, node wallet, detached signer |
| `cli`  | `select_rpc_decoys_from_history` → `select_rpc_decoy_pool` (`botho-wallet/src/ring_builder.rs:159`) | `botho-wallet` thin CLI wallet |
| `web`  | `selectDecoysForInput` (`web/packages/wasm-signer/src/decoys.ts:141`, called per input at `send.ts:655`) | web wallet, MetaMask Snap, claim links |
| `web-pre-1735` | historical chain-ordered `decoyPool[(i*19 + j) % len]` window (`send.ts:657` at `596f64c0`, removed by #1735) | nothing — comparison only, not in `--route production` |

**Node port provenance.** The `node` port is a literal copy of
`select_decoys_for_input`, including #1786's single shuffle of the eligible
set (which makes equal-distance ties a uniform draw), so the parity test
compares rings exactly. Replaying that shuffle in the simulation would
mean permuting all 1.2M eligible outputs for every ring, so when the band
fills the ring the simulation uses a banded fast path instead. It draws the
same gamma targets and picks the same distances, and breaks ties by a
per-ring random priority over the band members, which induces the same
uniform relative order as the shuffle. It is distribution-equivalent to the
literal port, not bit-identical, and a seeded statistical unit test checks
per-slot pick frequencies against the literal port within 5σ.

**Web port provenance.** The `web` port mirrors `decoys.ts` step for step:
10-block confirmation floor, `ageSimilarityBand` (±10%, same integer bps
math), partial Fisher–Yates over the in-band candidates, nearest-age fill
(shuffle, then stable sort by distance) when the band is thin, final ring
shuffle, every draw via `randomIndex`'s u32 rejection sampling over an
injected u32 source. The parity test replays all 16 scenarios of
`scripts/research/ct-economics/web-selection.json` — the rings #1735's
`ct-economics-selection.test.ts` recorded from the real `send.ts` with
`crypto.getRandomValues` stubbed by a seeded mulberry32 — through the port
with the same mulberry32 seeds and requires identical rings, order included;
it also checks the shared `test/fixtures/decoy-age-band.json` vectors. The
simulation drives the same port from ChaCha8 u32 draws. When the band alone
fills the ring (every reported age, including 100 on this chain: 20 in-band
outputs) it uses a sparse-swap fast path that is bit-identical to the
literal port; for thinner bands it uses a distribution-equivalent
nearest-age fill (unit-tested against the literal port).

**Chain model.** The UTXO set has one output at every height
1..=1,200,000, so it covers every reported age densely. The legacy model used
a gamma-aged pool instead, which puts every output between about 13k and 33k
blocks, exactly where the node picker's edge clamp hurts least. Cluster-tag
profiles use the legacy type mix (50% standard, 25% exchange, 10% whale, 10%
coinbase, 5% mixed). That mix tags *every* output. `--background-fraction`
blanks a share of them.

**Adversaries.** Each column is an *argmax identification rate*: how often
the adversary's single top suspect is the real input, with ties split. A
uniform guess scores exactly 5.0%.

- *Naive*: a uniform guess.
- *Age-Heuristic*: the gamma spend pdf.
- *Age-Rank*: knows the picker. It always guesses the age rank that the real
  input held most often in an independent set of training rings.
- *Band-Center*: guesses the member closest to the midrange of the ring's
  ages. Every native picker centres its band on the real age.
- *Cluster-Fingerprint*: matches each member's tags to the output's
  published tags.
- *Combined*: age-heuristic × cluster, as in the legacy model.

## Results

Command (about 30 s on a laptop):

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
| 100 | 10000 | 0 | 0.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r10) | 50.0% | 99.8% | 99.4% |
| 1000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 99.8% | 98.5% |
| 5000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 0.0% | 100.0% (r1) | 0.0% | 99.6% | 97.5% |
| 30000 | 10000 | 0 | 0.0% | 22.1% | 40752 | 5.0% | 0.0% | 34.7% (r19) | 59.6% | 99.7% | 98.4% |
| 100000 | 10000 | 0 | 0.0% | 100.0% | 190000 | 5.0% | 0.0% | 100.0% (r20) | 0.0% | 99.7% | 89.8% |
| 1000000 | 10000 | 0 | 0.0% | 100.0% | 190000 | 5.0% | 5.0% | 100.0% (r20) | 0.0% | 99.7% | 99.7% |
| gamma-spend | 10000 | 0 | 4.8% | 4.8% | 13 | 5.0% | 4.8% | 5.1% (r5) | 85.3% | 99.7% | 99.2% |

#### Route `cli`

Ports: select_rpc_decoys_from_history -> select_rpc_decoy_pool (botho-wallet/src/ring_builder.rs) — thin CLI wallet.
Outputs carry the real input's tags decayed one hop (plaintext).

| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 10000 | 0 | 0.0% | 0.0% | 90000 | 5.0% | 0.0% | 49.9% (r10) | 95.1% | 99.7% | 99.3% |
| 1000 | 10000 | 0 | 0.0% | 0.0% | 16760 | 5.0% | 0.0% | 18.1% (r11) | 53.4% | 99.7% | 99.2% |
| 5000 | 10000 | 0 | 0.0% | 0.0% | 15869 | 5.0% | 0.0% | 17.8% (r11) | 49.7% | 99.8% | 99.2% |
| 30000 | 10000 | 0 | 0.0% | 0.0% | 15504 | 5.0% | 0.0% | 18.0% (r10) | 50.9% | 99.7% | 99.3% |
| 100000 | 10000 | 0 | 0.0% | 0.0% | 16142 | 5.0% | 0.0% | 18.3% (r10) | 50.3% | 99.7% | 97.4% |
| 1000000 | 10000 | 0 | 0.0% | 0.0% | 15715 | 5.0% | 5.0% | 18.0% (r11) | 49.9% | 99.8% | 99.8% |
| gamma-spend | 10000 | 0 | 0.0% | 0.0% | 15830 | 5.0% | 1.8% | 18.6% (r10) | 49.1% | 99.8% | 99.6% |

#### Route `web`

Ports: selectDecoysForInput (web/packages/wasm-signer/src/decoys.ts, #1735) — web wallet, Snap, claim links.
Outputs carry empty cluster tags.

| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 10000 | 0 | 0.0% | 0.0% | 90000 | 5.0% | 0.0% | 50.0% (r11) | 94.7% | 5.0% | 0.0% |
| 1000 | 10000 | 0 | 0.0% | 0.0% | 17078 | 5.0% | 0.0% | 18.8% (r11) | 52.3% | 5.0% | 0.0% |
| 5000 | 10000 | 0 | 0.0% | 0.0% | 16128 | 5.0% | 0.0% | 17.4% (r11) | 50.2% | 5.0% | 0.0% |
| 30000 | 10000 | 0 | 0.0% | 0.0% | 16126 | 5.0% | 0.0% | 17.0% (r10) | 49.6% | 5.0% | 0.0% |
| 100000 | 10000 | 0 | 0.0% | 0.0% | 15689 | 5.0% | 0.0% | 16.9% (r10) | 49.1% | 5.0% | 0.0% |
| 1000000 | 10000 | 0 | 0.0% | 0.0% | 15756 | 5.0% | 5.0% | 17.8% (r10) | 49.5% | 5.0% | 5.0% |
| gamma-spend | 10000 | 0 | 0.0% | 0.0% | 15595 | 5.0% | 1.8% | 17.5% (r10) | 49.9% | 5.0% | 1.8% |

#### Real-input age-rank histogram (% of rings at each rank)

| route | real age | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| node | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| node | 1000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| node | 5000 | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| node | 30000 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 4 | 12 | 26 | 35 | 22 |
| node | 100000 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 100 |
| node | 1000000 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 100 |
| node | gamma-spend | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 |
| cli | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 50 | 50 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| cli | 1000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 15 | 19 | 18 | 14 | 9 | 5 | 2 | 0 | 0 | 0 | 0 | 0 |
| cli | 5000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 14 | 18 | 18 | 15 | 9 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| cli | 30000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 14 | 18 | 17 | 15 | 9 | 6 | 2 | 1 | 0 | 0 | 0 | 0 |
| cli | 100000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 14 | 18 | 18 | 14 | 10 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| cli | 1000000 | 0 | 0 | 0 | 0 | 1 | 3 | 5 | 9 | 14 | 17 | 18 | 15 | 9 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| cli | gamma-spend | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 14 | 19 | 18 | 14 | 10 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| web | 100 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 50 | 50 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| web | 1000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 14 | 19 | 19 | 15 | 9 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| web | 5000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 10 | 15 | 18 | 17 | 15 | 9 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| web | 30000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 9 | 15 | 17 | 18 | 15 | 9 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| web | 100000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 10 | 15 | 17 | 18 | 14 | 10 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| web | 1000000 | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 10 | 14 | 18 | 18 | 14 | 9 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |
| web | gamma-spend | 0 | 0 | 0 | 0 | 1 | 2 | 5 | 10 | 14 | 18 | 18 | 14 | 10 | 5 | 2 | 1 | 0 | 0 | 0 | 0 |

A low ID rate in one column does not mean the real input is safe; read
the row's maximum. In particular, `ID Combined` on the `web` rows (0.0% at
most ages) is a weighted geometric mean of the Age-Heuristic score and a
cluster score that is flat, because web outputs carry empty tags. It therefore reproduces the
Age-Heuristic guess (the ring member the gamma spend pdf favours, which is
rarely the median-aged real input) and ignores the midrange signal that
Band-Center uses to reach 94.7% at age 100.

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
| 100 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |
| 1000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |
| 5000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |
| 30000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |
| 100000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 100.0% | 100.0% (r1) | 0.0% | 5.0% | 100.0% |
| 1000000 | 10000 | 0 | 100.0% | 0.0% | 190000 | 5.0% | 5.0% | 100.0% (r1) | 0.0% | 5.0% | 5.0% |
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
| node | 1000 | 99.8% | 55.5% |
| node | 30000 | 99.7% | 54.3% |
| node | gamma-spend | 99.7% | 54.9% |
| cli | 1000 | 99.7% | 55.1% |
| cli | 30000 | 99.7% | 54.5% |
| cli | gamma-spend | 99.8% | 54.0% |
| web | 1000 | 5.0% | 4.9% |
| web | 30000 | 5.0% | 5.1% |
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

1. **The node picker's edge clamp (#1692)** puts every decoy on one side of
   the real input wherever the real age sits outside the gamma bulk (about
   13k–33k blocks). Inside the bulk (30k) the Age-Rank guess still gets 34.7%
   and Band-Center 59.6%. When real ages follow the picker's own gamma model,
   the pooled rank histogram looks uniform (χ² = 13). Conditioned on the
   ring's public ages, though, the real input is the midrange member in 85.3%
   of rings.
2. **The centred ±10% band leaks the real age (#1755).** This is a new
   finding of #1700. The CLI wallet and, since #1735, the web signer draw
   decoys uniformly within the band. Because the band is centred on the real
   age, each decoy is younger than the real input with probability ½. The
   real input's rank is therefore Binomial(19, ½) + 1: it is the median
   member, and the ring's midrange estimates its age. The `web` rows confirm
   it: Band-Center 49.1–52.3% at ages 1k–1M and gamma (cli 49.1–53.4%),
   Age-Rank 16.9–18.8% (cli 17.8–18.6%), 94.7% Band-Center at age 100 where
   the band holds only 20 outputs (cli 95.1%). Any picker that centres the
   band on the real input inherits this, including the node picker. A
   uniform age rank (the #1692 acceptance) is out of reach while the band
   stays centred. The one behavioural difference between `web` and `cli` —
   web fills a thin band with nearest-age outputs where the CLI refuses —
   never triggers at the reported ages on this chain (0 failures and no
   fallback for either route at age 100).
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
