#!/usr/bin/env python3
"""
Cadence Fee Model Simulation

Models the economic effects of Cadence's dual-tier progressive fee structure:
- Plain transactions: 5 bps base x cluster_factor (0.05% to 0.3%)
- Hidden transactions: 20 bps base x cluster_factor (0.2% to 1.2%)

Both transaction types apply progressive fees based on cluster wealth,
ensuring large holders can't avoid taxation by choosing plain transactions.

The 4x privacy premium reflects:
- ~10x more verification work (ring signatures + bulletproofs)
- ~10x more storage (~2.5KB vs ~250 bytes)
- Averaged to 4x to keep privacy accessible
"""

import os
import sys
from dataclasses import dataclass, field
from typing import List, Tuple, Dict
from enum import Enum
import math
import random

try:
    import numpy as np
    import matplotlib.pyplot as plt
    from matplotlib.gridspec import GridSpec
except ImportError:
    print("pip install numpy matplotlib")
    sys.exit(1)


# =============================================================================
# Transaction Types
# =============================================================================

class TxType(Enum):
    PLAIN = "plain"    # Transparent: 5 bps base
    HIDDEN = "hidden"  # Private: 20 bps base


# =============================================================================
# Fee Configuration (mirrors Rust implementation)
# =============================================================================

@dataclass
class CadenceFeeConfig:
    """Cadence's dual-tier progressive fee structure."""
    name: str
    plain_base_bps: float = 5      # 0.05% base for plain
    hidden_base_bps: float = 20    # 0.2% base for hidden (4x plain)
    factor_min: float = 1.0        # Minimum multiplier (small clusters)
    factor_max: float = 6.0        # Maximum multiplier (large clusters)
    w_mid: float = 10_000_000      # Sigmoid midpoint
    steepness: float = 5_000_000   # Sigmoid steepness

    def cluster_factor(self, cluster_wealth: float) -> float:
        """Compute cluster factor (1x to 6x) based on wealth."""
        if self.steepness == 0:
            return self.factor_max if cluster_wealth >= self.w_mid else self.factor_min

        x = (cluster_wealth - self.w_mid) / self.steepness
        x = max(-10, min(10, x))  # Clamp to avoid overflow
        sigmoid = 1 / (1 + math.exp(-x))
        return self.factor_min + (self.factor_max - self.factor_min) * sigmoid

    def rate_bps(self, tx_type: TxType, cluster_wealth: float) -> float:
        """Get fee rate in basis points for a transaction."""
        factor = self.cluster_factor(cluster_wealth)
        base = self.plain_base_bps if tx_type == TxType.PLAIN else self.hidden_base_bps
        return base * factor

    def compute_fee(self, tx_type: TxType, amount: float, cluster_wealth: float) -> Tuple[float, float]:
        """Compute (fee, net_amount) for a transaction."""
        rate = self.rate_bps(tx_type, cluster_wealth)
        fee = amount * rate / 10_000
        return fee, amount - fee


# =============================================================================
# Agent Model
# =============================================================================

@dataclass
class Agent:
    id: int
    balance: float
    agent_type: str  # "retail", "merchant", "whale"
    cluster_wealth: float = 0.0
    privacy_preference: float = 0.5  # 0=always plain, 1=always hidden

    def __post_init__(self):
        self.cluster_wealth = self.balance

    def choose_tx_type(self, rng: random.Random) -> TxType:
        """Choose transaction type based on privacy preference."""
        return TxType.HIDDEN if rng.random() < self.privacy_preference else TxType.PLAIN


# =============================================================================
# Simulation State
# =============================================================================

@dataclass
class SimState:
    agents: List[Agent]
    fee_config: CadenceFeeConfig
    round: int = 0
    total_fees_burned: float = 0.0
    plain_tx_count: int = 0
    hidden_tx_count: int = 0
    gini_history: List[Tuple[int, float]] = field(default_factory=list)
    whale_share_history: List[Tuple[int, float]] = field(default_factory=list)
    fee_history: List[Tuple[int, float, float]] = field(default_factory=list)  # (round, plain_fees, hidden_fees)


# =============================================================================
# Core Functions
# =============================================================================

def create_lognormal_agents(n: int, log_mean: float = 8.0, log_std: float = 1.8, seed: int = 42) -> List[Agent]:
    """Create agents with lognormal wealth distribution."""
    rng = np.random.default_rng(seed)
    wealths = rng.lognormal(mean=log_mean, sigma=log_std, size=n)
    sorted_idx = np.argsort(wealths)

    agents = []
    for i, idx in enumerate(sorted_idx):
        pct = i / n
        if pct < 0.70:
            atype = "retail"
            # Retail: moderate privacy preference (50-70%)
            privacy = rng.uniform(0.5, 0.7)
        elif pct < 0.90:
            atype = "merchant"
            # Merchants: low privacy preference (20-40%) - want transparent receipts
            privacy = rng.uniform(0.2, 0.4)
        else:
            atype = "whale"
            # Whales: high privacy preference (70-90%) - want to hide large holdings
            privacy = rng.uniform(0.7, 0.9)

        agents.append(Agent(id=i, balance=wealths[idx], agent_type=atype, privacy_preference=privacy))
    return agents


def calculate_gini(wealths: List[float]) -> float:
    """Calculate GINI coefficient for wealth distribution."""
    if len(wealths) < 2:
        return 0.0
    total = sum(wealths)
    if total == 0:
        return 0.0
    sorted_w = sorted(wealths)
    n = len(sorted_w)
    sum_idx = sum((i + 1) * w for i, w in enumerate(sorted_w))
    return max(0, min(1, (2 * sum_idx - (n + 1) * total) / (n * total)))


def transfer(state: SimState, sender: Agent, receiver: Agent, amount: float,
             tx_type: TxType, decay: float = 0.05) -> bool:
    """Execute a transfer with appropriate fee."""
    if sender.balance < amount or amount <= 0:
        return False

    fee, net = state.fee_config.compute_fee(tx_type, amount, sender.cluster_wealth)

    sender.balance -= amount
    receiver.balance += net
    sender.cluster_wealth = max(0, sender.cluster_wealth - amount)
    receiver.cluster_wealth = receiver.cluster_wealth * (1 - decay) + net * decay
    state.total_fees_burned += fee

    if tx_type == TxType.PLAIN:
        state.plain_tx_count += 1
    else:
        state.hidden_tx_count += 1

    return True


def run_round(state: SimState, rng: random.Random):
    """Run one simulation round with realistic transaction patterns."""
    agents = state.agents
    retail = [a for a in agents if a.agent_type == "retail"]
    merchants = [a for a in agents if a.agent_type == "merchant"]
    whales = [a for a in agents if a.agent_type == "whale"]

    # Retail purchases (mostly to merchants)
    for r in retail:
        if rng.random() < 0.20 and r.balance > 50 and merchants:
            amt = min(r.balance * 0.10, rng.uniform(20, 100))
            tx_type = r.choose_tx_type(rng)
            transfer(state, r, rng.choice(merchants), amt, tx_type)

    # Whale high-velocity activity
    for w in whales:
        for _ in range(10):
            tx_type = w.choose_tx_type(rng)

            # Large purchases from merchants
            if rng.random() < 0.30 and w.balance > 1000 and merchants:
                amt = min(w.balance * 0.03, rng.uniform(2000, 20000))
                transfer(state, w, rng.choice(merchants), amt, tx_type)

            # Payments to retail (wages, dividends)
            if rng.random() < 0.15 and retail and w.balance > 1000:
                amt = min(w.balance * 0.01, rng.uniform(1000, 5000))
                transfer(state, w, rng.choice(retail), amt, tx_type)

            # Whale-to-whale transfers
            if rng.random() < 0.25 and len(whales) > 1 and w.balance > 5000:
                other = rng.choice([x for x in whales if x.id != w.id])
                amt = min(w.balance * 0.05, rng.uniform(10000, 50000))
                transfer(state, w, other, amt, tx_type)

    # Merchant redistribution (wages/dividends to retail)
    for m in merchants:
        if rng.random() < 0.25 and retail:
            amt = min(m.balance * 0.08, rng.uniform(200, 800))
            if amt > 0:
                tx_type = m.choose_tx_type(rng)
                transfer(state, m, rng.choice(retail), amt, tx_type)


def record_metrics(state: SimState):
    """Record simulation metrics."""
    wealths = [a.balance for a in state.agents]
    gini = calculate_gini(wealths)
    state.gini_history.append((state.round, gini))

    total = sum(wealths)
    whale_w = sum(a.balance for a in state.agents if a.agent_type == "whale")
    state.whale_share_history.append((state.round, whale_w / total if total > 0 else 0))


def run_simulation(fee_config: CadenceFeeConfig, n_agents: int = 500, rounds: int = 10000,
                   log_std: float = 1.8, seed: int = 42) -> SimState:
    """Run full simulation."""
    rng = random.Random(seed)
    agents = create_lognormal_agents(n_agents, log_std=log_std, seed=seed)

    # Scale fee curve to wealth distribution
    wealths = [a.balance for a in agents]
    p90 = np.percentile(wealths, 90)

    # Adjust sigmoid parameters based on distribution
    fee_config.w_mid = p90 * 0.3
    fee_config.steepness = p90 * 0.15

    state = SimState(agents=agents, fee_config=fee_config)
    record_metrics(state)

    for r in range(1, rounds + 1):
        state.round = r
        run_round(state, rng)
        if r % 100 == 0:
            record_metrics(state)

    return state


# =============================================================================
# Comparison Scenarios
# =============================================================================

def create_comparison_configs() -> List[CadenceFeeConfig]:
    """Create fee configurations for comparison."""
    return [
        # Baseline: Flat 1% fee (no progressivity)
        CadenceFeeConfig("Flat 1%", plain_base_bps=100, hidden_base_bps=100,
                         factor_min=1, factor_max=1),

        # Current Cadence defaults (5 bps plain, 20 bps hidden, 1x-6x)
        CadenceFeeConfig("Cadence Default", plain_base_bps=5, hidden_base_bps=20,
                         factor_min=1, factor_max=6),

        # Aggressive: Higher max factor (1x-10x)
        CadenceFeeConfig("Cadence 1x-10x", plain_base_bps=5, hidden_base_bps=20,
                         factor_min=1, factor_max=10),

        # Very aggressive: Higher base + higher max
        CadenceFeeConfig("Cadence 10/40 bps", plain_base_bps=10, hidden_base_bps=40,
                         factor_min=1, factor_max=6),

        # Maximum progressivity
        CadenceFeeConfig("Cadence 10/40 1x-10x", plain_base_bps=10, hidden_base_bps=40,
                         factor_min=1, factor_max=10),
    ]


# =============================================================================
# Visualization
# =============================================================================

def plot_results(results: Dict[str, SimState], output_dir: str):
    """Generate comprehensive visualization."""
    fig = plt.figure(figsize=(16, 12))
    gs = GridSpec(3, 2, figure=fig, height_ratios=[1, 1, 0.8])

    fig.suptitle('Cadence Progressive Fee Model: GINI Impact Analysis\n'
                 'Plain (5 bps base) vs Hidden (20 bps base) x Cluster Factor (1x-6x)',
                 fontsize=14, fontweight='bold')

    # Plot 1: GINI evolution over time
    ax1 = fig.add_subplot(gs[0, 0])
    for name, state in results.items():
        rounds = [r for r, _ in state.gini_history]
        ginis = [g for _, g in state.gini_history]
        linestyle = '--' if 'Flat' in name else '-'
        linewidth = 3 if 'Default' in name else 2
        ax1.plot(rounds, ginis, label=name, linestyle=linestyle, linewidth=linewidth)

    initial_gini = list(results.values())[0].gini_history[0][1]
    target_gini = initial_gini * 0.5
    ax1.axhline(y=target_gini, color='red', linestyle=':', linewidth=2,
                label=f'50% Target ({target_gini:.3f})')

    ax1.set_xlabel('Simulation Round (~10 years)')
    ax1.set_ylabel('GINI Coefficient')
    ax1.set_title('Wealth Inequality Over Time')
    ax1.legend(fontsize=8, loc='upper right')
    ax1.grid(True, alpha=0.3)
    ax1.set_ylim(0.3, 1.0)

    # Plot 2: Whale share evolution
    ax2 = fig.add_subplot(gs[0, 1])
    for name, state in results.items():
        rounds = [r for r, _ in state.whale_share_history]
        shares = [s * 100 for _, s in state.whale_share_history]
        linestyle = '--' if 'Flat' in name else '-'
        linewidth = 3 if 'Default' in name else 2
        ax2.plot(rounds, shares, label=name, linestyle=linestyle, linewidth=linewidth)

    ax2.set_xlabel('Simulation Round (~10 years)')
    ax2.set_ylabel('Whale Share (%)')
    ax2.set_title('Top 10% Wealth Concentration')
    ax2.legend(fontsize=8)
    ax2.grid(True, alpha=0.3)

    # Plot 3: Final GINI comparison (bar chart)
    ax3 = fig.add_subplot(gs[1, 0])
    names = list(results.keys())
    initial_ginis = [results[n].gini_history[0][1] for n in names]
    final_ginis = [results[n].gini_history[-1][1] for n in names]
    reductions = [(i - f) / i * 100 for i, f in zip(initial_ginis, final_ginis)]

    x = np.arange(len(names))
    bars = ax3.bar(x, reductions, color=['tab:gray' if 'Flat' in n else 'tab:blue' for n in names])
    ax3.axhline(y=50, color='red', linestyle=':', linewidth=2, label='50% Target')
    ax3.set_xticks(x)
    ax3.set_xticklabels(names, rotation=45, ha='right', fontsize=9)
    ax3.set_ylabel('GINI Reduction (%)')
    ax3.set_title('Inequality Reduction by Fee Structure')
    ax3.legend()
    ax3.grid(True, alpha=0.3, axis='y')

    # Add value labels on bars
    for bar, red in zip(bars, reductions):
        ax3.text(bar.get_x() + bar.get_width()/2, bar.get_height() + 1,
                f'{red:.1f}%', ha='center', va='bottom', fontsize=9)

    # Plot 4: Transaction type distribution
    ax4 = fig.add_subplot(gs[1, 1])
    plain_counts = [results[n].plain_tx_count for n in names]
    hidden_counts = [results[n].hidden_tx_count for n in names]
    total_counts = [p + h for p, h in zip(plain_counts, hidden_counts)]
    hidden_pcts = [h / t * 100 if t > 0 else 0 for h, t in zip(hidden_counts, total_counts)]

    bars1 = ax4.bar(x - 0.2, [p/1000 for p in plain_counts], 0.4, label='Plain', color='tab:green', alpha=0.7)
    bars2 = ax4.bar(x + 0.2, [h/1000 for h in hidden_counts], 0.4, label='Hidden', color='tab:purple', alpha=0.7)
    ax4.set_xticks(x)
    ax4.set_xticklabels(names, rotation=45, ha='right', fontsize=9)
    ax4.set_ylabel('Transaction Count (thousands)')
    ax4.set_title('Transaction Type Distribution')
    ax4.legend()
    ax4.grid(True, alpha=0.3, axis='y')

    # Plot 5: Fee rate curves visualization
    ax5 = fig.add_subplot(gs[2, :])

    # Get cadence default config for visualization
    cadence_config = [c for c in create_comparison_configs() if 'Default' in c.name][0]

    # Sample wealth range
    wealth_range = np.logspace(3, 9, 100)
    plain_rates = [cadence_config.rate_bps(TxType.PLAIN, w) for w in wealth_range]
    hidden_rates = [cadence_config.rate_bps(TxType.HIDDEN, w) for w in wealth_range]

    ax5.semilogx(wealth_range, plain_rates, label='Plain (5 bps base)', color='tab:green', linewidth=2)
    ax5.semilogx(wealth_range, hidden_rates, label='Hidden (20 bps base)', color='tab:purple', linewidth=2)

    # Mark key points
    ax5.axhline(y=5, color='tab:green', linestyle=':', alpha=0.5, label='Plain min (5 bps)')
    ax5.axhline(y=30, color='tab:green', linestyle='--', alpha=0.5, label='Plain max (30 bps)')
    ax5.axhline(y=20, color='tab:purple', linestyle=':', alpha=0.5, label='Hidden min (20 bps)')
    ax5.axhline(y=120, color='tab:purple', linestyle='--', alpha=0.5, label='Hidden max (120 bps)')

    ax5.set_xlabel('Cluster Wealth')
    ax5.set_ylabel('Fee Rate (basis points)')
    ax5.set_title('Cadence Fee Curves: Progressive Rates by Transaction Type')
    ax5.legend(loc='center right', fontsize=9)
    ax5.grid(True, alpha=0.3)
    ax5.set_ylim(0, 150)

    plt.tight_layout()
    plt.savefig(f"{output_dir}/cadence_fee_model.png", dpi=150, bbox_inches='tight')
    print(f"Plot saved: {output_dir}/cadence_fee_model.png")


def print_summary(results: Dict[str, SimState]):
    """Print summary statistics."""
    print("\n" + "=" * 70)
    print("CADENCE PROGRESSIVE FEE MODEL - SIMULATION RESULTS")
    print("=" * 70)

    initial_gini = list(results.values())[0].gini_history[0][1]
    print(f"\nInitial GINI: {initial_gini:.3f}")
    print(f"Target GINI:  {initial_gini * 0.5:.3f} (50% reduction)")

    print("\n" + "-" * 70)
    print(f"{'Configuration':<25} {'Initial':>8} {'Final':>8} {'Reduction':>10} {'Target':>8}")
    print("-" * 70)

    for name, state in results.items():
        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        meets = "YES" if reduction >= 50 else "no"
        print(f"{name:<25} {initial:>8.3f} {final:>8.3f} {reduction:>9.1f}% {meets:>8}")

    print("\n" + "=" * 70)
    print("FEE STRUCTURE SUMMARY")
    print("=" * 70)
    print("""
Transaction Type    Base Rate    Min Fee (1x)    Max Fee (6x)
-----------------------------------------------------------------
Plain (transparent)    5 bps       0.05%           0.30%
Hidden (private)      20 bps       0.20%           1.20%

The 4x privacy premium reflects:
- ~10x more verification work (ring signatures + bulletproofs)
- ~10x more storage (~2.5KB vs ~250 bytes)
- Averaged to 4x to keep privacy accessible

Progressive taxation ensures:
- Large holders can't avoid fees by using plain transactions
- Small holders get cheap fees regardless of transaction type
- Privacy remains accessible (only 4x premium, not punitive)
""")


# =============================================================================
# Main
# =============================================================================

def main():
    output_dir = "./gini_10yr"
    os.makedirs(output_dir, exist_ok=True)

    print("=" * 70)
    print("CADENCE PROGRESSIVE FEE MODEL SIMULATION")
    print("Modeling: Plain (5 bps) vs Hidden (20 bps) x Cluster Factor (1x-6x)")
    print("=" * 70)

    configs = create_comparison_configs()
    results = {}

    for config in configs:
        print(f"\nRunning: {config.name}...")
        state = run_simulation(config, n_agents=500, rounds=10000, seed=42)
        results[config.name] = state

        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        print(f"  GINI: {initial:.3f} -> {final:.3f} ({reduction:+.1f}%)")
        print(f"  Transactions: {state.plain_tx_count:,} plain, {state.hidden_tx_count:,} hidden")
        print(f"  Fees burned: {state.total_fees_burned:,.0f}")

    plot_results(results, output_dir)
    print_summary(results)


if __name__ == "__main__":
    main()
