# Economic Simulation Scripts

This directory contains Python scripts for modeling the economic effects of Botho's progressive fee structure on wealth inequality.

## Quick Start

```bash
cd cluster-tax
python3 -m venv .venv
source .venv/bin/activate
pip install numpy matplotlib
python scripts/botho_fee_model.py
```

Output is saved to `./gini_10yr/botho_fee_model.png`.

## Scripts

### `botho_fee_model.py`

The primary simulation script. Models a 500-agent economy over 10,000 rounds (~10 years) with:

- **Lognormal wealth distribution**: Empirically matches real-world wealth distributions
- **Three agent types**: Retail (70%), Merchants (20%), Whales (10%)
- **Two transaction types**: Plain (transparent) and Hidden (private)
- **Privacy preferences**: Whales prefer hidden (70-90%), merchants prefer plain (20-40%)

### `gini_10yr_model.py`

Earlier exploration script comparing burn vs redistribution mechanisms. Useful for understanding the theoretical limits of different approaches.

## Methodology

### Agent-Based Modeling

The simulation uses agent-based modeling where individual actors make decisions based on:

1. **Balance constraints**: Can't spend more than you have
2. **Transaction patterns**: Different agent types have different behaviors
3. **Privacy preferences**: Probabilistic choice between plain and hidden transactions
4. **Cluster wealth tracking**: Wealth accumulation affects fee rates

### Wealth Distribution

Initial wealth follows a lognormal distribution with parameters chosen to match observed cryptocurrency wealth distributions:

```python
wealths = rng.lognormal(mean=8.0, sigma=1.8, size=n_agents)
```

This produces a distribution with:
- Initial GINI coefficient: ~0.79 (high inequality)
- Long tail of wealthy agents
- Many small holders

### Transaction Patterns

Each simulation round models realistic economic activity:

| Agent Type | Behavior |
|------------|----------|
| **Retail** | 20% chance of small purchase (20-100 units) from merchants |
| **Merchants** | 25% chance of wage payment (200-800 units) to retail |
| **Whales** | High-velocity trading: 10 transactions/round to merchants, retail, and other whales |

Whale high-velocity activity is critical - it exposes large holders to progressive fees frequently.

### Fee Calculation

Fees mirror the Rust implementation:

```python
def rate_bps(self, tx_type: TxType, cluster_wealth: float) -> float:
    factor = self.cluster_factor(cluster_wealth)  # 1x to 6x
    base = 5 if tx_type == TxType.PLAIN else 20   # bps
    return base * factor
```

### Metrics

**GINI Coefficient**: Standard measure of inequality (0 = perfect equality, 1 = one person has everything).

```python
def calculate_gini(wealths):
    sorted_w = sorted(wealths)
    n = len(sorted_w)
    sum_idx = sum((i + 1) * w for i, w in enumerate(sorted_w))
    return (2 * sum_idx - (n + 1) * sum(wealths)) / (n * sum(wealths))
```

**Whale Share**: Percentage of total wealth held by top 10% of agents.

## Results

### Fee Structure Comparison

| Configuration | Initial GINI | Final GINI | Reduction | Fees Burned |
|---------------|--------------|------------|-----------|-------------|
| Flat 1% | 0.788 | 0.413 | 47.5% | 985,840 |
| **Botho Default** | **0.788** | **0.409** | **48.1%** | **215,964** |
| Botho 1x-10x | 0.788 | 0.403 | 48.8% | 292,194 |
| Botho 10/40 bps | 0.788 | 0.406 | 48.5% | 431,196 |
| Botho 10/40 1x-10x | 0.788 | 0.408 | 48.3% | 583,021 |

### Visualization

![Botho Fee Model Results](../gini_10yr/botho_fee_model.png)

### Key Findings

1. **~48% inequality reduction is achievable** with burn-only mechanism over 10 years

2. **Progressive fees are 4.5x more efficient** than flat fees:
   - Flat 1%: Burns 985K to achieve 47.5% reduction
   - Botho Default: Burns 216K to achieve 48.1% reduction
   - Same result, 78% less total fee burden

3. **Diminishing returns beyond 6x factor**:
   - 1x-6x: 48.1% reduction
   - 1x-10x: 48.8% reduction
   - Only 0.7% improvement for 67% higher max factor

4. **Transaction type distribution** stabilizes at ~47% plain / 53% hidden, reflecting agent privacy preferences

The plot shows:
- **Top left**: GINI coefficient over time for all configurations
- **Top right**: Whale (top 10%) share decline over time
- **Middle left**: Bar chart comparing inequality reduction
- **Middle right**: Plain vs hidden transaction distribution
- **Bottom**: Fee rate curves showing progressive structure

## Sensitivity Analysis

### Varying Initial Inequality

| Initial GINI | Final GINI | Reduction |
|--------------|------------|-----------|
| 0.6 | 0.32 | 47% |
| 0.7 | 0.36 | 49% |
| 0.8 | 0.41 | 49% |
| 0.9 | 0.47 | 48% |

The fee structure achieves consistent ~48% reduction regardless of starting inequality.

### Varying Transaction Velocity

Higher whale transaction velocity leads to faster inequality reduction because whales are exposed to progressive fees more frequently.

### Burn vs Redistribution

From `gini_10yr_model.py`:

| Mechanism | Best Config | Final GINI | Reduction |
|-----------|-------------|------------|-----------|
| Burn | Prog 0.1%-80% | 0.42 | 47% |
| Redistribute | Prog 0.1%-70% | 0.38 | 52% |

Redistribution achieves ~5% better reduction but adds implementation complexity.

## Limitations

1. **Simplified economy**: Real economies have more complex transaction patterns
2. **No external factors**: Doesn't model new entrants, exits, or external wealth
3. **Fixed parameters**: Sigmoid midpoint is scaled to initial distribution only
4. **No behavioral adaptation**: Agents don't change behavior in response to fees

## Extending the Simulation

To test different parameters:

```python
config = BothoFeeConfig(
    name="Custom",
    plain_base_bps=10,      # Higher base rate
    hidden_base_bps=40,     # Maintain 4x ratio
    factor_min=1,
    factor_max=8,           # More aggressive progression
)
state = run_simulation(config, n_agents=1000, rounds=20000)
```

To add new agent types or behaviors, modify `run_round()` in the script.

## References

- GINI coefficient: [Wikipedia](https://en.wikipedia.org/wiki/Gini_coefficient)
- Lognormal wealth distribution: Pareto, V. (1896). "Cours d'économie politique"
- Agent-based economic modeling: Tesfatsion, L. (2006). "Agent-Based Computational Economics"
