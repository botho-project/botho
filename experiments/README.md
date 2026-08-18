# experiments

Output and findings from Botho's economic simulations — the empirical record
behind the monetary-policy decisions (cluster tax, emission schedule, lottery
tilt, demurrage). This is **research output, not shipped product code**: nothing
here is compiled or executed by the node, wallet or CI.

The simulators that produce these files live in
[`../cluster-tax/`](../cluster-tax/) — the Rust harness
(`cluster-tax/src/simulation/`, driven by the `cluster-tax-sim` binary) and the
Python models in [`../cluster-tax/scripts/`](../cluster-tax/scripts/).

## Contents

| Path | What it is |
|------|------------|
| [`ANALYSIS.md`](ANALYSIS.md) | **Findings.** "Economic Simulation Analysis" — privacy baseline, decay-rate impact on ring-signature privacy, whale wash-trading evasion of the cluster tax, the structural Gini-reduction experiment, and the ratified cumulative-vs-decay outcome (2026-07-05). |
| [`M2_RUNBOOK.md`](M2_RUNBOOK.md) | **Reproduction guide.** The M2 run matrix (#605 / #626 §7): copy-pasteable `cluster-tax-sim` invocations for every run, the population ladder, the decision rule, the determinism guarantee (fixed `--seed`) and the smoke tests that keep the harness honest. |
| [`results/`](results/) | Raw output of those runs (see below). |

## `results/`

| File(s) | Produced by | Contents |
|---------|-------------|----------|
| `gini_experiment_1yr.txt`, `gini_experiment_5yr.txt` | `cluster-tax-sim lottery-experiment --blocks …` | Structural Gini-reduction runs over ~365- and ~1825-day horizons at ~2.5%/yr emission, honest and gamed (whale splits into 1,000 UTXOs and churns weekly) |
| `gini_experiment_5yr_f{25,50,100}.txt` | same, varying `--emission-per-block` | Emission sensitivity at the 5-year horizon: 320 (~0.5%/yr), 640 (~1.0%/yr) and 1280 (~2.0%/yr) per block |
| `gini_experiment_5yr_atspend_f{25,50}.txt` | same | The spend-time-demurrage variants (as implemented in the node) at ~0.5%/yr and ~1.0%/yr emission, including the permanent-parker escape scenario |
| `gini_progressive.csv`, `gini_flat.csv`, `gini_comparison.csv` | `cluster-tax-sim compare --output experiments/results` | Per-round metrics for the progressive and flat fee curves, plus the combined Gini comparison |
| `lottery_sweep_30d.txt` | `cluster-tax-sim lottery-sweep` | Combined progressive-mechanism parameter sweep over a 30-day horizon |
| `emission_sweep.csv`, `emission_sweep.md` | `cluster-tax-sim emission-sweep` | Emission-schedule sweep (#350) — neutral numbers and observations feeding the emission decision (#321/#351) |

## Reading these results

Start with [`ANALYSIS.md`](ANALYSIS.md) for what the numbers mean, then
[`M2_RUNBOOK.md`](M2_RUNBOOK.md) to re-run anything. The design documents that
cite this data are
[`../docs/design/cluster-tilted-redistribution.md`](../docs/design/cluster-tilted-redistribution.md)
and
[`../docs/design/asymmetric-fees-simulation.md`](../docs/design/asymmetric-fees-simulation.md).

Files here are snapshots of a run, not regenerated automatically — when you
re-run a sweep with different parameters, note the invocation alongside the
output so the numbers stay reproducible.
