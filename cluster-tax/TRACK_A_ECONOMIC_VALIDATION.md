# Track A: Economic Validation Agent Prompt

## Context

You are working on **cluster taxation**, a novel mechanism for progressive transaction fees in a privacy-preserving cryptocurrency. The goal is reducing wealth inequality by making concentrated holdings pay higher fees, without requiring identity or being vulnerable to Sybil attacks.

## Core Concept

**Cluster taxation** taxes coins based on their *ancestry*, not account identity:

1. **Clusters**: Each coin-creation event (mining reward) spawns a new "cluster" identity
2. **Tag Vectors**: Every account carries a sparse vector of weights indicating what fraction of its coins trace back to each cluster origin
3. **Cluster Wealth**: `W_k = Σ (balance_i × tag_i(k))` — total value tagged to cluster k across all accounts
4. **Progressive Fees**: Fee rate = `f(cluster_wealth)` via sigmoid curve — larger clusters pay higher rates
5. **Tag Decay**: Tags decay by factor `(1-λ)` per transaction hop, gradually diffusing into "background"
6. **Tag Mixing**: When receiving coins, your tags become a weighted average of existing + incoming tags

**Key insight**: Splitting transactions or creating Sybil accounts doesn't help because fee rate depends on *cluster wealth*, not transaction size or account count. All accounts holding coins from the same origin pay the same high rate.

## What's Already Built

Location: `/cluster-tax/` in the cadence repo

```
cluster-tax/
├── src/
│   ├── lib.rs           # crate root
│   ├── cluster.rs       # ClusterId, ClusterWealth
│   ├── fee_curve.rs     # FeeCurve (sigmoid, LUT-based)
│   ├── tag.rs           # TagVector with decay/mixing
│   ├── transfer.rs      # Account, execute_transfer(), mint()
│   ├── analysis.rs      # Attack economics, parameter analysis
│   └── bin/sim.rs       # CLI simulation tool
```

**Current capabilities:**
- Basic transfer simulation with tag inheritance
- Fee curve with smooth sigmoid approximation
- Analysis functions for wash trading, structuring attacks
- CLI with scenarios: decay, fee-curve, structuring, wash-trading, whale-diffusion, mixer

**Run the CLI:**
```bash
cargo run -p mc-cluster-tax --features cli --bin cluster-tax-sim -- --help
```

## Your Mission: Economic Validation

### 1. Agent-Based Modeling

Extend the simulation with diverse actor types:

```rust
pub trait Agent {
    fn decide_action(&mut self, state: &SimulationState) -> Option<Action>;
    fn receive_payment(&mut self, amount: u64, tags: &TagVector);
}

pub enum Action {
    Transfer { to: AgentId, amount: u64 },
    Hold,
    UseMixer { amount: u64 },
}
```

**Agent types to implement:**

- **Whale**: Large holder, tries to minimize fees (may attempt wash trading, structuring, mixer use)
- **Merchant**: Receives many small payments, makes occasional large payments to suppliers
- **Retail User**: Small holder, occasional transactions
- **Market Maker**: High velocity, many small trades, tries to accumulate low-tag coins
- **Mixer Service**: Accepts deposits, returns coins from pooled reserves, charges fee
- **Miner**: Receives block rewards (fresh clusters), sells coins for goods/services

### 2. Metrics to Track

For each simulation run, compute:

- **Gini coefficient** of wealth distribution over time
- **Effective fee rate by wealth quintile** — are the rich actually paying more?
- **Total fees collected** (burned tokens)
- **Tag entropy** — how diffuse are tags across the economy?
- **Wash trading profitability** — do rational agents attempt it?
- **Mixer utilization** — how much volume flows through mixers?

### 3. Scenarios to Simulate

**Scenario A: Baseline Economy**
- 100 retail users, 10 merchants, 1 whale (10% of supply)
- 10,000 transaction rounds
- Measure fee distribution, Gini evolution

**Scenario B: Whale Fee Minimization**
- Whale actively tries to minimize fees via:
  - Splitting transactions
  - Wash trading
  - Using mixers
- Compare total fees paid vs. passive whale

**Scenario C: Mixer Equilibrium**
- Multiple competing mixers with different fee structures
- Measure: which mixers survive? What's equilibrium mixer fee?

**Scenario D: Velocity Variation**
- Compare high-velocity economy vs. low-velocity
- Does high velocity naturally reduce concentration?

**Scenario E: Parameter Sensitivity**
- Vary decay rate λ: [0.01, 0.05, 0.10, 0.20]
- Vary fee curve steepness
- Measure impact on inequality reduction and economic activity

### 4. Formal Analysis

If possible, derive:

- **Nash equilibrium** for whale behavior — is there a dominant strategy?
- **Steady-state distribution** of cluster wealths
- **Conditions for inequality reduction** — what parameter ranges guarantee Gini decrease?

### 5. Deliverables

1. Extended simulation code in `cluster-tax/src/simulation/` with agent-based modeling
2. New CLI commands for running the scenarios above
3. Report (can be markdown in the repo) with:
   - Simulation results
   - Parameter recommendations
   - Identified edge cases or failure modes
4. Any suggested changes to the core mechanism based on findings

## Key Design Decisions Already Made

- **Decay rate**: 5% per hop (λ = 0.05), ~14 hops to halve
- **Fee curve**: Sigmoid, r_min=0.05%, r_max=30%, midpoint at 10M tokens
- **Tag pruning**: Weights below 0.01% pruned to background, max 32 tags per vector
- **Fresh coins**: Not a significant problem — small fraction of supply, mixed immediately on first spend

## Open Questions for You to Explore

1. **What decay rate optimizes inequality reduction without killing economic activity?**
2. **Is there a fee curve shape that's more resistant to gaming than sigmoid?**
3. **Do mixers become too powerful? What's the equilibrium mixer market share?**
4. **How does the system behave with realistic wealth distributions (power law)?**
5. **Are there emergent attack strategies we haven't considered?**

## Code Style Notes

- Use `cargo fmt` and `cargo clippy`
- Tests for any new functionality
- The existing code uses fixed-point arithmetic for determinism (TAG_WEIGHT_SCALE = 1,000,000)
- Fee rates in basis points (1 bps = 0.01%)

Good luck. The mechanism is sound in principle — your job is to validate it empirically and find the parameter sweet spots.
