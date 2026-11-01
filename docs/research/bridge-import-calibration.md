# Calibrating the bridge-import epoch length `K` and floor `F`

**Issues:** [#937](https://github.com/botho-project/botho/issues/937) (original calibration, 2026-07-14), [#1697](https://github.com/botho-project/botho/issues/1697) (re-run on production semantics, 2026-10-31). Part of the design-confidence epic [#1683](https://github.com/botho-project/botho/issues/1683).
**Status:** `K` = 17,280 blocks and `F` = 1.5× remain the ratified values ([ADR 0007](../decisions/0007-bridge-import-cluster-tagging.md)); this re-run does **not** change them. It shows the original calibration measured the ADR's intent rather than the node, and that against the node the import factor can be shed for well under 1% of value. The decision that follows is about *where the floor is enforced* (#1687, #1686), not about the value of `K` or `F`. One `K` question (block time) is raised for a human decision in §7.
**Reproduce:**
```
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim -- bridge-import-sweep
```
Sim source: `cluster-tax/src/simulation/bridge_import_sweep.rs`. It is deterministic (no RNG): every number below is printed by that command. §8 reproduces the full output.

---

## 1. What changed in the sim (#1697)

The 2026-07 sweep modelled what ADR 0007 says. The node does something different in five ways. The sweep now models the node:

| | 2026-07 sweep | Node (and the sweep now) |
|---|---|---|
| Import-cluster wealth `W` | Σ unwrap amounts in the epoch | Cumulative: the block writer adds `amount·weight` for **every** output carrying the tag, at every hop, and never subtracts (`update_cluster_wealth_for_output`, `botho/src/ledger/store/writer.rs:469-500`) |
| Factor | `w·max(F, curve(W)) + (1−w)·1×` | Per spend, `max(curve(output-tag wealth), ring-centroid factor, F-blend on output tags)` (`consensus_fee_floor`, `botho/src/ledger/store/validation.rs:224-246`; `import_floor_factor_from_outputs`, `:305-338`) |
| Tags | Local 32-tag `TagVector`, no decay | Production `ClusterTagVector`: 16 tags, 0.1% pruning, wallet decay 5% per transfer (`merge_weighted`) |
| Block time | 5 s hard-coded | `K` is in blocks; wall-clock cost reported at 5, 20 and 40 s |
| Constants | `K`, `F`, `import_cluster_id` copied | Imported from `cluster_tax::bridge_import` |
| Fees | demurrage kernel only | `consensus_fee_model` (#1695): base fee + `max(accrued, capitalized downgrade)` |

Both file:line claims were re-checked against `main` at d5962dab. The F-blend is computed from the spender's **output** tags. Consensus only rejects tag *inflation* (`check_cluster_tag_inheritance`, `CONSENSUS_TAG_DECAY_RATE = 0`, `botho/src/ledger/store.rs:102-235`). So any decay a spender declares is valid, up to and including empty tags.

The DB-dependent half of the fee floor (claimed wealth, ring-centroid floor, import floor, writer accumulation) is restated over plain data in `cluster-tax/src/fee_floor_inputs.rs`. The golden test `fee_floor_inputs_match_ledger_for_sampled_inputs` (`botho/src/ledger/store.rs`) runs the real ledger and those mirrors over 24 sampled inputs and checks they agree, including the full consensus fee floor. `sim_factor_matches_production_import_factor_for_sampled_wealth` checks that the sim's factor for a fresh unwrap equals `import_cluster_factor(W)` over 200 sampled `W`.

Model assumptions: each input sits in a ring of 20 (`RING_SIZE`) with 19 background decoys of equal value. Fees are priced at the first demurrage-active height. In epoch 0 every demurrage term below is zero (#1685). Spends are two outputs (payment + change).

---

## 2. Split-game cost (§8 table 1)

A whale that unwraps its whole import in one epoch is charged the curve factor on its first spend (5.437× for 2M BTH). To be charged at the floor instead, it must spread the import across **584** epochs (was 541; the wallet's 5% decay and the per-spend `max` move the threshold slightly). `epochs to floor` is still independent of `K`.

What changes is the wall-clock cost. `K` is fixed in blocks, so at the 20–40 s blocks seen on testnet an epoch lasts 4–8 days, not one:

| whale (BTH) | epochs | K = 1 d @ 5 s | @ 20 s | @ 40 s |
|---:|---:|---:|---:|---:|
| 500,000 | 146 | 146 days | 584 days | 1,168 days |
| 2,000,000 | 584 | 584 days | 2,336 days | 4,672 days |
| 10,000,000 | 2,919 | 2,919 days | 11,676 days | 23,352 days |

**But the split game is no longer the cheapest escape.** §5 shows the same import shed in three self-spends. The split-game wall only matters if the tag cannot be shed some other way.

## 3. Co-location collateral (§8 table 2)

A 1,000-BTH entrant alone is charged 1.475× on its first spend. That is the F-blend after one 5% wallet decay: `1 + 0.5 × 0.95`. Caught in an epoch with a flood, it pays the shared pool's factor. Because `W` is cumulative, the collateral **keeps growing after the epoch closes**, as the flood's holders spend:

| flood | caught at close | after 5 flood spends | after 20 |
|---|---:|---:|---:|
| 5,000 BTH | 1.579× | 2.576× | 3.203× |
| 100,000 BTH | 3.473× | 4.623× | 5.261× |
| 2,000,000 BTH | 5.437× | 5.746× | 5.906× |

The 2026-07 doc bounded collateral by the epoch's unwrap volume. On the node it has no such bound: every downstream spend by anyone holding the epoch's tag raises `W`. A small 5,000-BTH co-entrant who churns 20 times takes a 1,000-BTH neighbour from 1.58× to 3.20×. Even the entrant's own self-churn raises its `W` (§5: a lone 1,000-BTH coin's factor rises to 1.56× after 10 self-spends).

## 4. The floor `F` (§8 table 3)

| F | entrant factor | wallet spend @ 1 day | @ 1 year | tag drop @ 1 day |
|---|---:|---:|---:|---:|
| 1.0× | 1.254× | 0.0003% | 0.1016% | 0.0600% |
| **1.5×** | **1.475×** | **0.4420%** | **0.4420%** | **0.0600%** |
| 2.0× | 1.950× | 1.3920% | 1.3920% | 0.0600% |

Two findings:

1. **The onboarding toll is not "~0.20 %/yr transient". It is ~0.44% of value on the first spend.** The claimed (output) class is `curve(0.95 × W)` ≈ 1.254× for a lone 1,000-BTH import. The F-blend floor is 1.475×. So every honest wallet spend looks like a class downgrade, and #925's capitalized charge fires on the 0.22× gap over the 5-year settlement horizon. At F = 1.0× the same spend costs 0.0003%.
2. **The tag-drop spend costs 0.06% at every F.** Background outputs make the F-blend read 1×. Only the ring-centroid floor is left, and 19 background decoys dilute it to 1.030×. An honest small entrant pays ~7× more than one who drops the tag. `F` taxes compliant wallets only.

## 5. Decay, self-churn and tag drop (§8 tables 4–6)

**Domestic circulation.** A 10,000-BTH coin from a 2,000,000-BTH epoch, merged on each spend with an equal background input, falls from 5.437× to 1.425× in 9 spends. On the 10th spend its import weight drops below the 0.1% pruning threshold and the tag disappears (cumulative fees 0.19%). That matches the old "≈9 spends" headline, for a different reason: pruning, not a harmonic tail.

**Self-churn, default wallet.** The ADR's invariant, "a pure-external holder who never receives domestic money stays at its import factor", **does not hold on the node**. With no domestic money at all, the stock wallet's 5% decay prunes the tag after **135** self-spends:

| coin | spends to ≤ F | spends to tag pruned | cumulative fee |
|---|---:|---:|---:|
| 1,000 BTH alone | (already 1.475×) | 135 | 0.616% |
| 10,000 of a 2M epoch | 124 | 135 | 0.108% |

At one spend per 12 blocks, 135 spends is about 2.25 hours at 5 s blocks.

**Self-churn, max-shed decay.** A spender can declare any decay. The most it can shed per hop without triggering the downgrade charge is to weight ÷ 20. At that rate its claimed wealth equals the 20-member ring centroid. **The flood coin goes from 5.437× to 1.000× in 3 spends for 0.154% of value.** The 2,000,000-BTH whale in §2 can do the same instead of waiting 584 epochs.

**One-shot tag drop.** Dropping every tag in one spend costs 0.06% for a small entrant. For a flood coin it costs 5.0%, because the ring-centroid floor is still 3.5× (W/20 = 100k BTH). The whale's rational path is max-shed churn: 3 hops, 0.154%.

These are consensus-valid behaviours, already tracked as #1687 (tags can be dropped; F is read from outputs) and #1686 (the ring-centroid floor is diluted by spender-chosen decoys). The sim gives them numbers.

## 6. Epoch steering by the relayer (finding; fix is separate)

`bth_scan::build_release_tx` tags the release to `import_cluster_id_for_height(created_at_height)`, where `created_at_height` is the relayer's choice (`bridge/service/src/bth_scan.rs:379, 420`). The ledger records a tag as an import cluster only if its id matches the epoch of the block that includes it (`record_bridge_import_clusters_for_output`, `writer.rs:511-534`). A release tagged to any *other* epoch's id is still accepted:

- If the id belongs to an earlier epoch that is already recorded, the release joins that epoch's pool. The relayer can pick a quiet epoch (low `W`) or a flood epoch (high `W`).
- If the id belongs to an epoch never recorded (a future epoch, or a past one with no unwrap), the cluster is not in the import set. The F-blend does not apply, and the coin is priced on the plain curve of its own amount.

Not fixed here. Filed separately.

## 7. Recommendation

- **Keep `K` = 17,280 and `F` = 1.5× unchanged.** No value of `K` or `F` closes the escapes in §4–§5. They exist because the floor is read from spender-declared output tags and the only input-side floor is diluted by decoys. Raising `F` makes it worse: it raises the toll on honest wallets (0.44% → 1.39% at 2×) while the tag-drop cost stays at 0.06%.
- **The import factor is only as strong as #1687 / #1686 make it.** Until consensus derives the import floor from inputs or the ring, ADR 0007's guarantees are advisory against a modified wallet, and even the stock wallet sheds them by self-churn. Evidence posted on #1687.
- **Human decision: `K` in blocks vs wall-clock.** `K` = 17,280 is one day only at 5 s. At the 20–40 s blocks seen on testnet it is 4–8 days. The original calibration recommended "no longer than 1 day" to keep the co-location net narrow. If block times stay above 5 s, either `K` should be lowered in blocks or the 1-day rationale restated. Filed for decision; the constant is not changed here.
- **Honest-wallet toll.** The 0.44% first-spend charge comes from comparing `curve(claimed)` against the F-blend. Whether an honest import spend should count as a "downgrade" belongs with the #1687 decision.

---

## 8. Generated tables (verbatim sweep output)

Ratified constants (imported from `cluster_tax::bridge_import`): K = 17280 blocks, F = 1.500x. Reference block time 5 s. Wallet tag decay 5% per transfer; ring size 20. Fees are `consensus_fee_model` (consensus floor only) at the first demurrage-active height.

### Split-game cost by epoch length K and block time (at F)

`undiluted factor` = the demurrage factor at the first wallet spend of the whole import unwrapped in one epoch. `epochs to floor` = the fewest equal epochs after which a tranche's first spend is charged at the import floor (the curve no longer binds). K is fixed in blocks, so the wall-clock cost scales with the block time.

| K | whale import (BTH) | undiluted factor | epochs to floor | days @ 5 s | days @ 20 s | days @ 40 s |
|---|---:|---:|---:|---:|---:|---:|
| 14h (10080 blocks) | 500000 | 4.576x | 146 | 85.2 | 340.7 | 681.3 |
| 1d (17280 blocks) | 500000 | 4.576x | 146 | 146.0 | 584.0 | 1168.0 |
| 2d (34560 blocks) | 500000 | 4.576x | 146 | 292.0 | 1168.0 | 2336.0 |
| 3.5d (60480 blocks) | 500000 | 4.576x | 146 | 511.0 | 2044.0 | 4088.0 |
| 1wk (120960 blocks) | 500000 | 4.576x | 146 | 1022.0 | 4088.0 | 8176.0 |
| 14h (10080 blocks) | 2000000 | 5.437x | 584 | 340.7 | 1362.7 | 2725.3 |
| 1d (17280 blocks) | 2000000 | 5.437x | 584 | 584.0 | 2336.0 | 4672.0 |
| 2d (34560 blocks) | 2000000 | 5.437x | 584 | 1168.0 | 4672.0 | 9344.0 |
| 3.5d (60480 blocks) | 2000000 | 5.437x | 584 | 2044.0 | 8176.0 | 16352.0 |
| 1wk (120960 blocks) | 2000000 | 5.437x | 584 | 4088.0 | 16352.0 | 32704.0 |
| 14h (10080 blocks) | 10000000 | 5.738x | 2919 | 1702.8 | 6811.0 | 13622.0 |
| 1d (17280 blocks) | 10000000 | 5.738x | 2919 | 2919.0 | 11676.0 | 23352.0 |
| 2d (34560 blocks) | 10000000 | 5.738x | 2919 | 5838.0 | 23352.0 | 46704.0 |
| 3.5d (60480 blocks) | 10000000 | 5.738x | 2919 | 10216.5 | 40866.0 | 81732.0 |
| 1wk (120960 blocks) | 10000000 | 5.738x | 2919 | 20433.0 | 81732.0 | 163464.0 |

### Small-entrant collateral in a flood epoch, as the flood circulates

A 1000-BTH entrant shares an epoch with a flood. Its factor is measured at its first wallet spend, after the flood has been spent 0, 5 and 20 times. Each flood spend adds the flood's import-weighted value to the shared cumulative wealth, so the entrant's factor keeps rising after the epoch closes.

| flood regime | flood (BTH) | entrant alone | caught, 0 hops | caught, 5 hops | caught, 20 hops |
|---|---:|---:|---:|---:|---:|
| low | 5000 | 1.475x | 1.579x | 2.576x | 3.203x |
| medium | 100000 | 1.475x | 3.473x | 4.623x | 5.261x |
| high | 2000000 | 1.475x | 5.437x | 5.746x | 5.906x |

### Floor F sweep: onboarding toll vs the tag-drop escape

A 1000-BTH entrant alone in its epoch. Fees are the consensus floor as a % of value: its first wallet spend after one day and after one year, and the same coin spent once to background outputs (tag drop) after one day.

| F | entrant factor | wallet spend @ 1 day | wallet spend @ 1 year | tag-drop spend @ 1 day | residual above background |
|---|---:|---:|---:|---:|---:|
| 1.000x | 1.254x | 0.0003% | 0.1016% | 0.0600% | 0.000 |
| 1.250x | 1.254x | 0.0003% | 0.1016% | 0.0600% | 0.050 |
| 1.500x | 1.475x | 0.4420% | 0.4420% | 0.0600% | 0.100 |
| 1.750x | 1.712x | 0.9160% | 0.9160% | 0.0600% | 0.150 |
| 2.000x | 1.950x | 1.3920% | 1.3920% | 0.0600% | 0.200 |

### Decay by domestic circulation (production tags and wealth)

A 10000-BTH coin from a 2000000-BTH epoch. Each spend merges it with an equal-value background input under the wallet's 5% decay, pays half away and keeps half as change. `import W` is the import cluster's cumulative wealth; `next-spend factor` is what the coin's next wallet spend is charged at.

| spends | import weight | import W (BTH) | next-spend factor | step fee | cumulative fee |
|---:|---:|---:|---:|---:|---:|
| 0 | 1000000 | 2000000 | 5.437x | 0.0000% | 0.0000% |
| 1 | 475000 | 2009500 | 5.016x | 0.0044% | 0.0089% |
| 2 | 225625 | 2014012 | 4.503x | 0.0039% | 0.0166% |
| 3 | 107171 | 2016156 | 3.993x | 0.0033% | 0.0233% |
| 4 | 50905 | 2017174 | 3.483x | 0.0028% | 0.0288% |
| 5 | 24179 | 2017658 | 2.976x | 0.0022% | 0.0332% |
| 6 | 11484 | 2017887 | 2.469x | 0.0016% | 0.0365% |
| 7 | 5454 | 2017996 | 1.963x | 0.0011% | 0.0386% |
| 8 | 2590 | 2018048 | 1.559x | 0.0006% | 0.0399% |
| 9 | 1230 | 2018073 | 1.425x | 0.0005% | 0.0408% |
| 10 | 0 | 2018073 | 1.000x | 0.0740% | 0.1888% |
| 11 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 12 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 13 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 14 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 15 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 16 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 17 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 18 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 19 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |
| 20 | 0 | 2018073 | 1.000x | 0.0000% | 0.1888% |

### Self-churn adversary (no domestic money)

The coin is spent to its owner every 12 blocks. Nothing domestic enters. Under the default wallet the 5% decay alone strips the import tag, and `ClusterTagVector` prunes it to background below 0.1%. The max-shed spender declares decay 95.0% per hop (weight ÷ 20), the most it can shed while its claimed wealth stays at the ring-centroid floor, so the downgrade charge does not fire. Both are valid at consensus, which rejects only tag inflation.

**small entrant (1,000 BTH alone), default wallet**

| spends | import weight | import W (BTH) | next-spend factor | cumulative fee |
|---:|---:|---:|---:|---:|
| 0 | 1000000 | 1000 | 1.475x | 0.0000% |
| 1 | 950000 | 1950 | 1.451x | 0.4420% |
| 2 | 902500 | 2852 | 1.430x | 0.6160% |
| 5 | 773780 | 5298 | 1.511x | 0.6160% |
| 10 | 598735 | 8624 | 1.557x | 0.6160% |
| 20 | 358482 | 13189 | 1.545x | 0.6160% |
| 50 | 76937 | 18538 | 1.319x | 0.6160% |
| 100 | 5911 | 19887 | 1.053x | 0.6160% |
| 150 | 0 | 19979 | 1.000x | 0.6160% |

Spends to factor ≤ 1.500x: 0; to ≤ 1.100x: 75; to tag pruned: 135 (cumulative fee 0.6160%).

**flood coin (10,000 of a 2,000,000-BTH epoch), default wallet**

| spends | import weight | import W (BTH) | next-spend factor | cumulative fee |
|---:|---:|---:|---:|---:|
| 0 | 1000000 | 2000000 | 5.437x | 0.0000% |
| 1 | 950000 | 2009500 | 5.428x | 0.0000% |
| 2 | 902500 | 2018525 | 5.419x | 0.0000% |
| 5 | 773780 | 2042982 | 5.364x | 0.0000% |
| 10 | 598735 | 2076240 | 5.231x | 0.0000% |
| 20 | 358482 | 2121887 | 4.863x | 0.0000% |
| 50 | 76937 | 2175378 | 3.840x | 0.0001% |
| 100 | 5911 | 2188868 | 2.060x | 0.0001% |
| 150 | 0 | 2189793 | 1.000x | 0.1081% |

Spends to factor ≤ 1.500x: 124; to ≤ 1.100x: 134; to tag pruned: 135 (cumulative fee 0.1081%).

**flood coin (10,000 of a 2,000,000-BTH epoch), max-shed decay**

| spends | import weight | import W (BTH) | next-spend factor | cumulative fee |
|---:|---:|---:|---:|---:|
| 0 | 1000000 | 2000000 | 5.437x | 0.0000% |
| 1 | 50000 | 2000500 | 3.467x | 0.0000% |
| 2 | 2500 | 2000525 | 1.552x | 0.0000% |
| 5 | 0 | 2000525 | 1.000x | 0.1540% |
| 10 | 0 | 2000525 | 1.000x | 0.1540% |
| 20 | 0 | 2000525 | 1.000x | 0.1540% |
| 50 | 0 | 2000525 | 1.000x | 0.1540% |
| 100 | 0 | 2000525 | 1.000x | 0.1540% |
| 150 | 0 | 2000525 | 1.000x | 0.1540% |

Spends to factor ≤ 1.500x: 3; to ≤ 1.100x: 3; to tag pruned: 3 (cumulative fee 0.1540%).

### Tag-drop adversary (background outputs)

First spend after one day, honest wallet vs background outputs. With background outputs the import floor reads 1× (it is computed from output tags); only the ring-centroid floor remains, and 19 background decoys dilute it.

| coin | honest factor | honest fee | drop factor | drop ring floor | drop fee |
|---|---:|---:|---:|---:|---:|
| small entrant (1,000 BTH alone) | 1.475x | 0.4420% | 1.030x | 1.030x | 0.0600% |
| flood coin (10,000 of 2,000,000) | 5.437x | 0.0049% | 3.500x | 3.500x | 5.0000% |
| undiluted whale (2,000,000 alone) | 5.437x | 0.0049% | 3.500x | 3.500x | 5.0000% |

### Concentration signal: flood's cheap-money footprint

100 domestic 1,000-BTH holders plus one 2000000-BTH flood; Gini of value ÷ factor. Production `import_cluster_factor` prices the flood at 5.448x.

| flood enters at | Gini (effective-spendable) |
|---|---:|
| factor-1 (ADR 0003 status quo) | 0.9425 |
| import factor (ADR 0007) | 0.7760 |


