#!/usr/bin/env python3
"""
Correct Cluster-Based Progressive Fee Simulation

Key insight: Identity flows with money along the transfer graph.

Each cluster's effective wealth = sum of all balances weighted by tag attribution.
Fee is based on cluster wealth, not individual balance.
"""

import os
import sys
from dataclasses import dataclass, field
from typing import Dict, List, Tuple
import math
import random

try:
    import numpy as np
    import matplotlib.pyplot as plt
except ImportError:
    print("pip install numpy matplotlib")
    sys.exit(1)


# =============================================================================
# Configuration
# =============================================================================

@dataclass
class FeeConfig:
    """Progressive fee curve based on cluster wealth."""
    name: str
    r_min_bps: float  # Min fee rate (basis points)
    r_max_bps: float  # Max fee rate (basis points)
    w_mid: float      # Wealth level at 50% fee rate
    steepness: float  # Curve sharpness

    def rate_bps(self, cluster_wealth: float) -> float:
        """Compute fee rate based on cluster's total wealth."""
        if self.r_min_bps == self.r_max_bps:
            return self.r_min_bps
        x = (cluster_wealth - self.w_mid) / max(self.steepness, 1)
        sigmoid = 1 / (1 + math.exp(-x)) if -700 < x < 700 else (0 if x < 0 else 1)
        return self.r_min_bps + (self.r_max_bps - self.r_min_bps) * sigmoid


@dataclass
class Agent:
    """An agent with balance and cluster tags."""
    id: int
    balance: float
    agent_type: str  # "retail", "merchant", "whale"
    tags: Dict[int, float] = field(default_factory=dict)  # cluster_id -> weight (0-1)

    def dominant_cluster(self) -> Tuple[int, float]:
        """Return (cluster_id, weight) of dominant tag, or (-1, 0) if none."""
        if not self.tags:
            return (-1, 0.0)
        max_cluster = max(self.tags.keys(), key=lambda k: self.tags[k])
        return (max_cluster, self.tags[max_cluster])

    def background_weight(self) -> float:
        """Weight not attributed to any cluster (fully diffused)."""
        return max(0, 1.0 - sum(self.tags.values()))


class ClusterRegistry:
    """Tracks all clusters and their total wealth."""

    def __init__(self):
        self.next_id = 0

    def create_cluster(self) -> int:
        """Create a new cluster, return its ID."""
        cluster_id = self.next_id
        self.next_id += 1
        return cluster_id

    def compute_cluster_wealth(self, agents: List[Agent]) -> Dict[int, float]:
        """
        Compute each cluster's effective wealth.

        Wealth(cluster_k) = sum_i(agent_i.balance * agent_i.tags[k])
        """
        wealth = {}
        for agent in agents:
            for cluster_id, weight in agent.tags.items():
                if cluster_id not in wealth:
                    wealth[cluster_id] = 0.0
                wealth[cluster_id] += agent.balance * weight
        return wealth


@dataclass
class SimState:
    """Simulation state."""
    agents: List[Agent]
    registry: ClusterRegistry
    fee_config: FeeConfig
    round: int = 0
    total_fees: float = 0.0
    gini_history: List[Tuple[int, float]] = field(default_factory=list)
    whale_share_history: List[Tuple[int, float]] = field(default_factory=list)


# =============================================================================
# Core Functions
# =============================================================================

def create_agents_with_clusters(n: int, registry: ClusterRegistry,
                                 log_mean: float = 8.0, log_std: float = 1.8,
                                 seed: int = 42) -> List[Agent]:
    """
    Create agents with lognormal wealth distribution.
    Each agent starts with their own cluster (100% self-tag).
    """
    rng = np.random.default_rng(seed)
    wealths = rng.lognormal(mean=log_mean, sigma=log_std, size=n)
    sorted_idx = np.argsort(wealths)

    agents = []
    for i, idx in enumerate(sorted_idx):
        pct = i / n
        atype = "retail" if pct < 0.70 else ("merchant" if pct < 0.90 else "whale")

        # Each agent starts with their own cluster
        cluster_id = registry.create_cluster()
        agent = Agent(
            id=i,
            balance=wealths[idx],
            agent_type=atype,
            tags={cluster_id: 1.0}  # 100% self-attribution
        )
        agents.append(agent)

    return agents


def calculate_gini(wealths: List[float]) -> float:
    """Calculate Gini coefficient (0=equal, 1=unequal)."""
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
             decay: float = 0.05, cluster_wealth_cache: Dict[int, float] = None) -> bool:
    """
    Transfer coins from sender to receiver.

    - Sender's tags flow with the coins (proportionally)
    - Tags decay by `decay` fraction per hop
    - Fee is based on sender's dominant cluster's TOTAL wealth
    """
    if sender.balance < amount or amount <= 0:
        return False

    # Use cached cluster wealth if provided, else compute
    if cluster_wealth_cache is None:
        cluster_wealth_cache = state.registry.compute_cluster_wealth(state.agents)

    # Compute effective wealth as MAX(tag_weight × cluster_wealth) across all clusters
    # This ensures you pay fees based on the wealthiest cluster your coins trace to
    effective_wealth = 0
    for cluster_id, tag_weight in sender.tags.items():
        if cluster_id in cluster_wealth_cache:
            weighted_wealth = tag_weight * cluster_wealth_cache[cluster_id]
            effective_wealth = max(effective_wealth, weighted_wealth)

    # Compute fee based on cluster wealth (fee on top)
    rate = state.fee_config.rate_bps(effective_wealth)
    fee = amount * rate / 10_000
    total_cost = amount + fee

    if sender.balance < total_cost:
        return False

    # --- Execute transfer ---

    # 1. Compute sender's tag rates (for proportional flow)
    sender_tag_rates = {k: v for k, v in sender.tags.items()}

    # 2. Update sender's balance
    sender.balance -= total_cost

    # 3. Compute incoming tags (sender's tags, decayed)
    incoming_tags = {}
    for cluster_id, weight in sender_tag_rates.items():
        decayed_weight = weight * (1 - decay)
        if decayed_weight > 0.001:  # Prune tiny weights
            incoming_tags[cluster_id] = decayed_weight

    # 4. Mix incoming tags with receiver's existing tags (value-weighted)
    old_balance = receiver.balance
    new_balance = old_balance + amount

    if new_balance > 0:
        mixed_tags = {}

        # Receiver's existing tags, weighted by old balance
        for cluster_id, weight in receiver.tags.items():
            mixed_tags[cluster_id] = weight * (old_balance / new_balance)

        # Incoming tags, weighted by incoming amount
        for cluster_id, weight in incoming_tags.items():
            if cluster_id in mixed_tags:
                mixed_tags[cluster_id] += weight * (amount / new_balance)
            else:
                mixed_tags[cluster_id] = weight * (amount / new_balance)

        # Prune small tags
        receiver.tags = {k: v for k, v in mixed_tags.items() if v > 0.001}

    # 5. Update receiver's balance
    receiver.balance = new_balance

    # 6. Record fee
    state.total_fees += fee

    return True


def run_round(state: SimState, rng: random.Random):
    """Run one round of economic activity."""
    agents = state.agents
    retail = [a for a in agents if a.agent_type == "retail"]
    merchants = [a for a in agents if a.agent_type == "merchant"]
    whales = [a for a in agents if a.agent_type == "whale"]

    # Compute cluster wealth ONCE per round (expensive operation)
    cluster_wealth = state.registry.compute_cluster_wealth(agents)

    # Retail purchases
    for r in retail:
        if rng.random() < 0.20 and r.balance > 50 and merchants:
            amt = min(r.balance * 0.10, rng.uniform(20, 100))
            transfer(state, r, rng.choice(merchants), amt, cluster_wealth_cache=cluster_wealth)

    # Whale activity
    for w in whales:
        for _ in range(10):
            if rng.random() < 0.30 and w.balance > 1000 and merchants:
                amt = min(w.balance * 0.03, rng.uniform(2000, 20000))
                transfer(state, w, rng.choice(merchants), amt, cluster_wealth_cache=cluster_wealth)
            if rng.random() < 0.15 and retail and w.balance > 1000:
                amt = min(w.balance * 0.01, rng.uniform(1000, 5000))
                transfer(state, w, rng.choice(retail), amt, cluster_wealth_cache=cluster_wealth)
            if rng.random() < 0.25 and len(whales) > 1 and w.balance > 5000:
                other = rng.choice([x for x in whales if x.id != w.id])
                amt = min(w.balance * 0.05, rng.uniform(10000, 50000))
                transfer(state, w, other, amt, cluster_wealth_cache=cluster_wealth)

    # Merchant redistribution (wages)
    for m in merchants:
        if rng.random() < 0.25 and retail:
            amt = min(m.balance * 0.08, rng.uniform(200, 800))
            if amt > 0:
                transfer(state, m, rng.choice(retail), amt, cluster_wealth_cache=cluster_wealth)


def record_metrics(state: SimState):
    """Record current metrics."""
    wealths = [a.balance for a in state.agents]
    gini = calculate_gini(wealths)
    state.gini_history.append((state.round, gini))

    total = sum(wealths)
    whale_w = sum(a.balance for a in state.agents if a.agent_type == "whale")
    state.whale_share_history.append((state.round, whale_w / total if total > 0 else 0))


def run_simulation(fee_config: FeeConfig, n_agents: int = 500, rounds: int = 10000,
                   log_std: float = 1.8, seed: int = 42) -> Tuple[SimState, dict]:
    """Run full simulation."""
    rng = random.Random(seed)
    registry = ClusterRegistry()
    agents = create_agents_with_clusters(n_agents, registry, log_std=log_std, seed=seed)

    # Scale fee curve to wealth distribution
    wealths = [a.balance for a in agents]
    p90 = np.percentile(wealths, 90)

    if fee_config.r_min_bps != fee_config.r_max_bps:
        fee_config.w_mid = p90 * 0.3
        fee_config.steepness = p90 * 0.15

    state = SimState(agents=agents, registry=registry, fee_config=fee_config)
    initial_supply = sum(a.balance for a in agents)
    record_metrics(state)

    for r in range(1, rounds + 1):
        state.round = r
        run_round(state, rng)
        if r % 100 == 0:
            record_metrics(state)

    final_supply = sum(a.balance for a in agents)
    stats = {
        'initial_supply': initial_supply,
        'final_supply': final_supply,
        'total_burned': state.total_fees,
        'burn_pct': (state.total_fees / initial_supply) * 100
    }

    return state, stats


# =============================================================================
# Main
# =============================================================================

def main():
    output_dir = "./gini_10yr"
    os.makedirs(output_dir, exist_ok=True)

    print("=" * 70)
    print("CLUSTER-BASED PROGRESSIVE FEE SIMULATION")
    print("Identity flows with money - fee based on CLUSTER wealth, not balance")
    print("=" * 70)

    configs = [
        ("Flat 1%", FeeConfig("Flat 1%", 100, 100, 0, 1)),
        ("Prog 0.1%-30%", FeeConfig("Prog 0.1%-30%", 10, 3000, 0, 1)),
        ("Prog 0.1%-50%", FeeConfig("Prog 0.1%-50%", 10, 5000, 0, 1)),
        ("Prog 0.1%-70%", FeeConfig("Prog 0.1%-70%", 10, 7000, 0, 1)),
    ]

    print("\nBURN MODE (fees destroyed)")
    print("-" * 60)

    results = {}
    for name, config in configs:
        state, stats = run_simulation(
            FeeConfig(name, config.r_min_bps, config.r_max_bps, 0, 1),
            n_agents=500, rounds=5000  # Fewer rounds since cluster calc is expensive
        )
        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        results[name] = (state, stats)

        print(f"  {name:20s}: GINI {initial:.3f} → {final:.3f} ({reduction:+.1f}%) | burned {stats['burn_pct']:.1f}%")

    # Plot results
    fig, axes = plt.subplots(1, 2, figsize=(12, 5))
    fig.suptitle('Cluster-Based Progressive Fees\n(Fee based on CLUSTER wealth, not individual balance)',
                 fontsize=12, fontweight='bold')

    ax1 = axes[0]
    for name, (state, _) in results.items():
        rounds = [r for r, _ in state.gini_history]
        ginis = [g for _, g in state.gini_history]
        linestyle = '--' if 'Flat' in name else '-'
        ax1.plot(rounds, ginis, label=name, linestyle=linestyle, linewidth=2)

    ax1.set_xlabel('Round')
    ax1.set_ylabel('GINI Coefficient')
    ax1.set_title('GINI Over Time')
    ax1.legend()
    ax1.grid(True, alpha=0.3)

    ax2 = axes[1]
    names = list(results.keys())
    ginis = [results[n][0].gini_history[-1][1] for n in names]
    burns = [results[n][1]['burn_pct'] for n in names]

    x = np.arange(len(names))
    ax2.bar(x, ginis, color='steelblue', alpha=0.7)
    ax2.set_xticks(x)
    ax2.set_xticklabels(names, rotation=45, ha='right')
    ax2.set_ylabel('Final GINI')
    ax2.set_title('Final GINI Comparison')
    ax2.grid(True, alpha=0.3, axis='y')

    # Add burn % as text
    for i, (g, b) in enumerate(zip(ginis, burns)):
        ax2.text(i, g + 0.02, f'{b:.0f}%\nburned', ha='center', fontsize=8)

    plt.tight_layout()
    plt.savefig(f"{output_dir}/gini_cluster_model.png", dpi=150, bbox_inches='tight')
    print(f"\nPlot saved: {output_dir}/gini_cluster_model.png")


if __name__ == "__main__":
    main()
