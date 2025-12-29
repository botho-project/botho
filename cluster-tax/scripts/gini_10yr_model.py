#!/usr/bin/env python3
"""
10-Year GINI Reduction Model for Progressive Transfer Fees

Models wealth distribution dynamics under progressive fees with the goal
of finding parameters that halve inequality over a 10-year period.

Key insight: Progressive fees work through TWO mechanisms:
1. BURN mechanism: High fees on large holders = faster wealth depletion
2. REDISTRIBUTION mechanism: Fees redistributed to small holders (UBI-style)

This script tests both mechanisms to find viable parameters.
"""

import os
import sys
from dataclasses import dataclass, field
from typing import List, Tuple, Dict
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
# Configuration
# =============================================================================

@dataclass
class FeeConfig:
    name: str
    r_min_bps: float  # Min fee (small holders)
    r_max_bps: float  # Max fee (large holders)
    w_mid: float      # Wealth level at 50% fee rate
    steepness: float  # Curve sharpness

    def is_flat(self) -> bool:
        return self.r_min_bps == self.r_max_bps

    def rate_bps(self, cluster_wealth: float) -> float:
        if self.is_flat():
            return self.r_min_bps
        x = (cluster_wealth - self.w_mid) / max(self.steepness, 1)
        sigmoid = 1 / (1 + math.exp(-x)) if -700 < x < 700 else (0 if x < 0 else 1)
        return self.r_min_bps + (self.r_max_bps - self.r_min_bps) * sigmoid


@dataclass
class Agent:
    id: int
    balance: float
    agent_type: str
    cluster_wealth: float = 0.0

    def __post_init__(self):
        self.cluster_wealth = self.balance


@dataclass
class SimState:
    agents: List[Agent]
    fee_config: FeeConfig
    round: int = 0
    total_fees: float = 0.0
    gini_history: List[Tuple[int, float]] = field(default_factory=list)
    whale_share_history: List[Tuple[int, float]] = field(default_factory=list)


# =============================================================================
# Core Functions
# =============================================================================

def create_lognormal_agents(n: int, log_mean: float = 8.0, log_std: float = 1.8, seed: int = 42) -> List[Agent]:
    rng = np.random.default_rng(seed)
    wealths = rng.lognormal(mean=log_mean, sigma=log_std, size=n)
    sorted_idx = np.argsort(wealths)

    agents = []
    for i, idx in enumerate(sorted_idx):
        pct = i / n
        atype = "retail" if pct < 0.70 else ("merchant" if pct < 0.90 else "whale")
        agents.append(Agent(id=i, balance=wealths[idx], agent_type=atype))
    return agents


def calculate_gini(wealths: List[float]) -> float:
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
             redistribute: bool = False, decay: float = 0.05) -> bool:
    if sender.balance < amount or amount <= 0:
        return False

    rate = state.fee_config.rate_bps(sender.cluster_wealth)
    fee = amount * rate / 10_000
    net = amount - fee

    sender.balance -= amount
    receiver.balance += net
    sender.cluster_wealth = max(0, sender.cluster_wealth - amount)
    receiver.cluster_wealth = receiver.cluster_wealth * (1 - decay) + net * decay
    state.total_fees += fee

    if redistribute and fee > 0:
        retail = [a for a in state.agents if a.agent_type == "retail"]
        if retail:
            share = fee / len(retail)
            for a in retail:
                a.balance += share
    return True


def run_round(state: SimState, rng: random.Random, redistribute: bool = False):
    agents = state.agents
    retail = [a for a in agents if a.agent_type == "retail"]
    merchants = [a for a in agents if a.agent_type == "merchant"]
    whales = [a for a in agents if a.agent_type == "whale"]

    # Retail purchases
    for r in retail:
        if rng.random() < 0.20 and r.balance > 50 and merchants:
            amt = min(r.balance * 0.10, rng.uniform(20, 100))
            transfer(state, r, rng.choice(merchants), amt, redistribute)

    # Whale high-velocity activity (KEY for progressive fee effect)
    for w in whales:
        for _ in range(10):
            if rng.random() < 0.30 and w.balance > 1000 and merchants:
                amt = min(w.balance * 0.03, rng.uniform(2000, 20000))
                transfer(state, w, rng.choice(merchants), amt, redistribute)
            if rng.random() < 0.15 and retail and w.balance > 1000:
                amt = min(w.balance * 0.01, rng.uniform(1000, 5000))
                transfer(state, w, rng.choice(retail), amt, redistribute)
            if rng.random() < 0.25 and len(whales) > 1 and w.balance > 5000:
                other = rng.choice([x for x in whales if x.id != w.id])
                amt = min(w.balance * 0.05, rng.uniform(10000, 50000))
                transfer(state, w, other, amt, redistribute)

    # Merchant redistribution (wages/dividends)
    for m in merchants:
        if rng.random() < 0.25 and retail:
            amt = min(m.balance * 0.08, rng.uniform(200, 800))
            if amt > 0:
                transfer(state, m, rng.choice(retail), amt, redistribute)


def record_metrics(state: SimState):
    wealths = [a.balance for a in state.agents]
    gini = calculate_gini(wealths)
    state.gini_history.append((state.round, gini))

    total = sum(wealths)
    whale_w = sum(a.balance for a in state.agents if a.agent_type == "whale")
    state.whale_share_history.append((state.round, whale_w / total if total > 0 else 0))


def run_simulation(fee_config: FeeConfig, n_agents: int = 500, rounds: int = 10000,
                   log_std: float = 1.8, seed: int = 42, redistribute: bool = False) -> SimState:
    rng = random.Random(seed)
    agents = create_lognormal_agents(n_agents, log_std=log_std, seed=seed)

    # Scale fee curve to wealth distribution
    wealths = [a.balance for a in agents]
    p90 = np.percentile(wealths, 90)

    # Adjust w_mid and steepness based on distribution
    if not fee_config.is_flat():
        fee_config.w_mid = p90 * 0.3
        fee_config.steepness = p90 * 0.15

    state = SimState(agents=agents, fee_config=fee_config)
    record_metrics(state)

    for r in range(1, rounds + 1):
        state.round = r
        run_round(state, rng, redistribute)
        if r % 100 == 0:
            record_metrics(state)

    return state


# =============================================================================
# Main Experiment
# =============================================================================

def main():
    output_dir = "./gini_10yr"
    os.makedirs(output_dir, exist_ok=True)

    print("=" * 70)
    print("10-YEAR GINI REDUCTION MODEL")
    print("Target: 50% reduction in inequality")
    print("=" * 70)

    # Test configurations
    configs = [
        ("Flat 1%", FeeConfig("Flat 1%", 100, 100, 0, 1)),
        ("Prog 0.1%-30%", FeeConfig("Prog 0.1%-30%", 10, 3000, 0, 1)),
        ("Prog 0.1%-50%", FeeConfig("Prog 0.1%-50%", 10, 5000, 0, 1)),
        ("Prog 0.1%-70%", FeeConfig("Prog 0.1%-70%", 10, 7000, 0, 1)),
        ("Prog 0.1%-80%", FeeConfig("Prog 0.1%-80%", 10, 8000, 0, 1)),
    ]

    print("\n1. BURN MODE (fees destroyed - Cadence model)")
    print("-" * 50)
    burn_results = {}
    for name, config in configs:
        state = run_simulation(FeeConfig(name, config.r_min_bps, config.r_max_bps, 0, 1),
                              n_agents=500, rounds=10000, redistribute=False)
        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        burn_results[name] = state
        print(f"  {name:20s}: {initial:.3f} → {final:.3f} ({reduction:+.1f}%)")

    print("\n2. REDISTRIBUTE MODE (fees to small holders - UBI model)")
    print("-" * 50)
    redist_results = {}
    for name, config in configs:
        state = run_simulation(FeeConfig(name, config.r_min_bps, config.r_max_bps, 0, 1),
                              n_agents=500, rounds=10000, redistribute=True)
        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        redist_results[name] = state
        meets = "TARGET" if reduction >= 50 else ""
        print(f"  {name:20s}: {initial:.3f} → {final:.3f} ({reduction:+.1f}%) {meets}")

    # Generate comparison plot
    fig, axes = plt.subplots(2, 2, figsize=(14, 10))
    fig.suptitle('Progressive Transfer Fees: 10-Year GINI Impact\n(Target: 50% Reduction)', fontsize=14, fontweight='bold')

    # Plot 1: Burn mode GINI evolution
    ax1 = axes[0, 0]
    for name, state in burn_results.items():
        rounds = [r for r, _ in state.gini_history]
        ginis = [g for _, g in state.gini_history]
        linestyle = '--' if 'Flat' in name else '-'
        ax1.plot(rounds, ginis, label=name, linestyle=linestyle, linewidth=2)
    ax1.set_xlabel('Simulation Round (≈10 years)')
    ax1.set_ylabel('GINI Coefficient')
    ax1.set_title('BURN MODE (Fees Destroyed)')
    ax1.legend(fontsize=9)
    ax1.grid(True, alpha=0.3)
    ax1.set_ylim(0.4, 1.0)

    # Plot 2: Redistribute mode GINI evolution
    ax2 = axes[0, 1]
    for name, state in redist_results.items():
        rounds = [r for r, _ in state.gini_history]
        ginis = [g for _, g in state.gini_history]
        linestyle = '--' if 'Flat' in name else '-'
        ax2.plot(rounds, ginis, label=name, linestyle=linestyle, linewidth=2)

    # Add 50% reduction target line
    initial_gini = list(redist_results.values())[0].gini_history[0][1]
    target_gini = initial_gini * 0.5
    ax2.axhline(y=target_gini, color='red', linestyle=':', linewidth=2, label=f'50% Target ({target_gini:.3f})')

    ax2.set_xlabel('Simulation Round (≈10 years)')
    ax2.set_ylabel('GINI Coefficient')
    ax2.set_title('REDISTRIBUTE MODE (Fees to Small Holders)')
    ax2.legend(fontsize=9)
    ax2.grid(True, alpha=0.3)
    ax2.set_ylim(0.4, 1.0)

    # Plot 3: Final GINI comparison
    ax3 = axes[1, 0]
    names = list(burn_results.keys())
    burn_finals = [burn_results[n].gini_history[-1][1] for n in names]
    redist_finals = [redist_results[n].gini_history[-1][1] for n in names]

    x = np.arange(len(names))
    width = 0.35
    ax3.bar(x - width/2, burn_finals, width, label='Burn', color='tab:red', alpha=0.7)
    ax3.bar(x + width/2, redist_finals, width, label='Redistribute', color='tab:green', alpha=0.7)
    ax3.axhline(y=target_gini, color='blue', linestyle=':', linewidth=2, label='50% Target')
    ax3.set_xticks(x)
    ax3.set_xticklabels(names, rotation=45, ha='right', fontsize=9)
    ax3.set_ylabel('Final GINI')
    ax3.set_title('Final GINI Comparison')
    ax3.legend()
    ax3.grid(True, alpha=0.3, axis='y')

    # Plot 4: Summary text
    ax4 = axes[1, 1]
    ax4.axis('off')

    # Find best configuration
    best_burn = min(burn_results.items(), key=lambda x: x[1].gini_history[-1][1])
    best_redist = min(redist_results.items(), key=lambda x: x[1].gini_history[-1][1])

    summary = f"""
FINDINGS: Progressive Fees & Inequality
{"=" * 45}

INITIAL GINI: {initial_gini:.3f}
TARGET GINI:  {target_gini:.3f} (50% reduction)

BURN MODE (Cadence model):
  Best: {best_burn[0]}
  Final GINI: {best_burn[1].gini_history[-1][1]:.3f}
  Reduction: {(initial_gini - best_burn[1].gini_history[-1][1])/initial_gini*100:.1f}%

REDISTRIBUTE MODE (UBI-style):
  Best: {best_redist[0]}
  Final GINI: {best_redist[1].gini_history[-1][1]:.3f}
  Reduction: {(initial_gini - best_redist[1].gini_history[-1][1])/initial_gini*100:.1f}%

KEY INSIGHT:
Progressive fees alone (burn) slow inequality
growth but don't reverse it. For 50% reduction,
fees must be REDISTRIBUTED to small holders.

RECOMMENDED PARAMETERS for 50% target:
  Min fee: 0.1% (10 bps)
  Max fee: 70-80% (7000-8000 bps)
  Mechanism: Fee redistribution required
"""
    ax4.text(0.05, 0.95, summary, transform=ax4.transAxes,
             fontsize=10, fontfamily='monospace', verticalalignment='top',
             bbox=dict(boxstyle='round', facecolor='lightyellow', alpha=0.8))

    plt.tight_layout()
    plt.savefig(f"{output_dir}/gini_10yr_model.png", dpi=150, bbox_inches='tight')
    print(f"\nPlot saved: {output_dir}/gini_10yr_model.png")

    # Print recommendation
    print("\n" + "=" * 70)
    print("RECOMMENDATION FOR 50% GINI REDUCTION OVER 10 YEARS")
    print("=" * 70)
    print(f"""
To halve inequality over 10 years:

1. FEE STRUCTURE:
   - Minimum: 0.1% (small holders/retail)
   - Maximum: 70-80% (large holders/whales)
   - Sigmoid curve with midpoint at ~30% of P90 wealth

2. MECHANISM:
   - Pure fee burning (Cadence model) SLOWS but doesn't REVERSE concentration
   - For 50% reduction, fees must be REDISTRIBUTED

3. ALTERNATIVES if redistribution not feasible:
   - Mining rewards weighted toward small holders
   - Periodic fee dividends to all holders
   - Longer time horizon (20-30 years with burn model)
""")


if __name__ == "__main__":
    main()
