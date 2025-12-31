#!/usr/bin/env python3
"""
UTXO-Based Progressive Fee Simulation

Tracks coins (UTXOs) individually, exactly as the network does.
Each UTXO carries its own tag vector indicating provenance.
"""

import os
import sys
from dataclasses import dataclass, field
from typing import Dict, List, Tuple, Optional
import math
import random

try:
    import numpy as np
    import matplotlib.pyplot as plt
except ImportError:
    print("pip install numpy matplotlib")
    sys.exit(1)


# =============================================================================
# Core Types
# =============================================================================

# Tag weight scale: 1_000_000 = 100%
TAG_SCALE = 1_000_000
TAG_PRUNE_THRESHOLD = 1000  # 0.1% minimum to keep

@dataclass
class UTXO:
    """An unspent transaction output with value and provenance tags."""
    id: int
    value: float
    tags: Dict[int, int]  # cluster_id -> weight (0 to TAG_SCALE)
    owner: int  # agent id

    def tag_weight(self, cluster_id: int) -> float:
        """Get tag weight as fraction (0.0 to 1.0)."""
        return self.tags.get(cluster_id, 0) / TAG_SCALE

    def background_weight(self) -> float:
        """Fraction not attributed to any cluster."""
        total = sum(self.tags.values())
        return max(0, TAG_SCALE - total) / TAG_SCALE


@dataclass
class Agent:
    """An agent who owns UTXOs."""
    id: int
    agent_type: str  # "retail", "merchant", "whale"


@dataclass
class FeeConfig:
    """Progressive fee curve."""
    name: str
    r_min_bps: float
    r_max_bps: float
    w_mid: float
    steepness: float

    def rate_bps(self, effective_wealth: float) -> float:
        if self.r_min_bps == self.r_max_bps:
            return self.r_min_bps
        x = (effective_wealth - self.w_mid) / max(self.steepness, 1)
        sigmoid = 1 / (1 + math.exp(-x)) if -700 < x < 700 else (0 if x < 0 else 1)
        return self.r_min_bps + (self.r_max_bps - self.r_min_bps) * sigmoid


class UTXOSet:
    """Global set of all unspent outputs."""

    def __init__(self):
        self.utxos: Dict[int, UTXO] = {}  # id -> UTXO
        self.by_owner: Dict[int, List[int]] = {}  # agent_id -> [utxo_ids]
        self.next_id = 0
        self.next_cluster = 0

    def create_utxo(self, value: float, tags: Dict[int, int], owner: int) -> UTXO:
        """Create a new UTXO."""
        utxo = UTXO(id=self.next_id, value=value, tags=tags, owner=owner)
        self.next_id += 1
        self.utxos[utxo.id] = utxo
        if owner not in self.by_owner:
            self.by_owner[owner] = []
        self.by_owner[owner].append(utxo.id)
        return utxo

    def spend_utxo(self, utxo_id: int):
        """Remove a UTXO (mark as spent)."""
        if utxo_id in self.utxos:
            utxo = self.utxos[utxo_id]
            del self.utxos[utxo_id]
            if utxo.owner in self.by_owner:
                self.by_owner[utxo.owner] = [u for u in self.by_owner[utxo.owner] if u != utxo_id]

    def get_owner_utxos(self, owner: int) -> List[UTXO]:
        """Get all UTXOs owned by an agent."""
        return [self.utxos[uid] for uid in self.by_owner.get(owner, []) if uid in self.utxos]

    def get_owner_balance(self, owner: int) -> float:
        """Get total balance for an agent."""
        return sum(u.value for u in self.get_owner_utxos(owner))

    def new_cluster(self) -> int:
        """Create a new cluster ID (for minting)."""
        cid = self.next_cluster
        self.next_cluster += 1
        return cid

    def compute_cluster_wealth(self) -> Dict[int, float]:
        """
        Compute each cluster's total wealth.
        Wealth(k) = sum over all UTXOs (utxo.value × utxo.tags[k] / TAG_SCALE)
        """
        wealth: Dict[int, float] = {}
        for utxo in self.utxos.values():
            for cluster_id, weight in utxo.tags.items():
                if cluster_id not in wealth:
                    wealth[cluster_id] = 0.0
                wealth[cluster_id] += utxo.value * weight / TAG_SCALE
        return wealth


def blend_tags(inputs: List[Tuple[float, Dict[int, int]]]) -> Dict[int, int]:
    """
    Blend tags from multiple inputs, weighted by value.

    inputs: List of (value, tags) tuples
    returns: blended tags
    """
    total_value = sum(v for v, _ in inputs)
    if total_value == 0:
        return {}

    # Accumulate weighted tags
    blended: Dict[int, float] = {}
    for value, tags in inputs:
        weight = value / total_value
        for cluster_id, tag_weight in tags.items():
            if cluster_id not in blended:
                blended[cluster_id] = 0.0
            blended[cluster_id] += tag_weight * weight

    # Convert to integer weights and prune small tags
    result: Dict[int, int] = {}
    for cluster_id, weight in blended.items():
        int_weight = int(weight)
        if int_weight >= TAG_PRUNE_THRESHOLD:
            result[cluster_id] = int_weight

    return result


def apply_decay(tags: Dict[int, int], decay_rate: int) -> Dict[int, int]:
    """
    Apply decay to tags, moving mass to background.

    decay_rate: in parts per million (e.g., 50_000 = 5%)
    """
    result: Dict[int, int] = {}
    for cluster_id, weight in tags.items():
        decayed = weight - (weight * decay_rate // TAG_SCALE)
        if decayed >= TAG_PRUNE_THRESHOLD:
            result[cluster_id] = decayed
    return result


def compute_effective_wealth(utxos: List[UTXO], initial_cluster_wealth: Dict[int, float]) -> float:
    """
    Compute effective wealth for fee calculation.

    KEY FIX: Use INITIAL cluster wealth (wealth at time of minting), not current spread.
    This prevents the feedback loop where tags spreading causes cluster_wealth to grow.

    Returns: MAX(tag_weight × initial_wealth) across all clusters
    """
    if not utxos:
        return 0.0

    # Blend input tags
    total_value = sum(u.value for u in utxos)
    if total_value == 0:
        return 0.0

    blended_tags: Dict[int, float] = {}
    for utxo in utxos:
        for cluster_id, weight in utxo.tags.items():
            if cluster_id not in blended_tags:
                blended_tags[cluster_id] = 0.0
            blended_tags[cluster_id] += (utxo.value / total_value) * (weight / TAG_SCALE)

    # Find MAX(tag_weight × initial_cluster_wealth)
    max_wealth = 0.0
    for cluster_id, tag_weight in blended_tags.items():
        if cluster_id in initial_cluster_wealth:
            effective = tag_weight * initial_cluster_wealth[cluster_id]
            max_wealth = max(max_wealth, effective)

    return max_wealth


# =============================================================================
# Simulation
# =============================================================================

@dataclass
class SimState:
    """Simulation state."""
    agents: List[Agent]
    utxo_set: UTXOSet
    fee_config: FeeConfig
    initial_cluster_wealth: Dict[int, float] = field(default_factory=dict)  # Snapshot at start
    round: int = 0
    total_fees: float = 0.0
    gini_history: List[Tuple[int, float]] = field(default_factory=list)
    avg_fee_rate_history: List[Tuple[int, float, float, float]] = field(default_factory=list)  # (round, retail, merchant, whale)
    tag_concentration_history: List[Tuple[int, float]] = field(default_factory=list)  # avg max tag weight

    decay_rate: int = 50_000  # 5% per hop (increased for faster convergence testing)


def create_initial_state(n_agents: int, fee_config: FeeConfig,
                         log_mean: float = 8.0, log_std: float = 1.8,
                         seed: int = 42) -> SimState:
    """Create initial state with lognormal wealth distribution."""
    rng = np.random.default_rng(seed)
    wealths = rng.lognormal(mean=log_mean, sigma=log_std, size=n_agents)
    sorted_idx = np.argsort(wealths)

    utxo_set = UTXOSet()
    agents = []

    for i, idx in enumerate(sorted_idx):
        pct = i / n_agents
        atype = "retail" if pct < 0.70 else ("merchant" if pct < 0.90 else "whale")
        agent = Agent(id=i, agent_type=atype)
        agents.append(agent)

        # Create initial UTXO with 100% tag to own cluster
        cluster_id = utxo_set.new_cluster()
        utxo_set.create_utxo(
            value=wealths[idx],
            tags={cluster_id: TAG_SCALE},  # 100% attribution
            owner=agent.id
        )

    # Scale fee curve
    p90 = np.percentile(wealths, 90)
    if fee_config.r_min_bps != fee_config.r_max_bps:
        fee_config.w_mid = p90 * 0.3
        fee_config.steepness = p90 * 0.15

    # Capture initial cluster wealth - this is the KEY:
    # We use the owner's INITIAL wealth as the cluster's wealth forever
    # This prevents the feedback loop of tags spreading → cluster wealth growing
    initial_cluster_wealth = utxo_set.compute_cluster_wealth()

    return SimState(agents=agents, utxo_set=utxo_set, fee_config=fee_config,
                    initial_cluster_wealth=initial_cluster_wealth)


def transfer(state: SimState, sender_id: int, receiver_id: int, amount: float,
             rng: random.Random) -> bool:
    """
    Transfer amount from sender to receiver.

    1. Select input UTXOs (simple: use all sender's UTXOs if needed)
    2. Compute fee based on effective wealth of inputs
    3. Blend input tags, apply decay
    4. Create output UTXOs (payment + change)
    5. Burn fee
    """
    sender_utxos = state.utxo_set.get_owner_utxos(sender_id)
    sender_balance = sum(u.value for u in sender_utxos)

    if sender_balance < amount or amount <= 0:
        return False

    # First pass: estimate inputs needed (assume max fee rate for safety)
    max_rate = state.fee_config.r_max_bps
    estimated_fee = amount * max_rate / 10_000
    estimated_total = amount + estimated_fee

    # Select inputs (greedy: use UTXOs until we have enough)
    selected_utxos: List[UTXO] = []
    selected_value = 0.0
    for utxo in sorted(sender_utxos, key=lambda u: -u.value):  # Largest first
        selected_utxos.append(utxo)
        selected_value += utxo.value
        if selected_value >= estimated_total:
            break

    if not selected_utxos:
        return False

    # Compute ACTUAL fee based on SELECTED inputs only
    # Use INITIAL cluster wealth to prevent feedback loop
    effective_wealth = compute_effective_wealth(selected_utxos, state.initial_cluster_wealth)
    rate = state.fee_config.rate_bps(effective_wealth)
    fee = amount * rate / 10_000
    total_needed = amount + fee

    if selected_value < total_needed:
        # Need more inputs - add more UTXOs
        remaining_utxos = [u for u in sender_utxos if u not in selected_utxos]
        for utxo in sorted(remaining_utxos, key=lambda u: -u.value):
            selected_utxos.append(utxo)
            selected_value += utxo.value
            # Recompute fee with new inputs
            effective_wealth = compute_effective_wealth(selected_utxos, state.initial_cluster_wealth)
            rate = state.fee_config.rate_bps(effective_wealth)
            fee = amount * rate / 10_000
            total_needed = amount + fee
            if selected_value >= total_needed:
                break

    if selected_value < total_needed:
        return False

    # Blend tags from selected inputs
    inputs_for_blend = [(u.value, u.tags) for u in selected_utxos]
    blended_tags = blend_tags(inputs_for_blend)

    # Decay only applies to coins moving to a NEW owner
    decayed_tags = apply_decay(blended_tags, state.decay_rate)

    # Spend selected UTXOs
    for utxo in selected_utxos:
        state.utxo_set.spend_utxo(utxo.id)

    # Create output to receiver (with decay - coins changed hands)
    state.utxo_set.create_utxo(
        value=amount,
        tags=decayed_tags.copy(),
        owner=receiver_id
    )

    # Create change output to sender
    # NOTE: In real system with stealth addresses, we can't distinguish change from payment
    # So decay applies to ALL outputs (protocol can't know which is "change to self")
    change = selected_value - total_needed
    if change > 0.01:  # Dust threshold
        state.utxo_set.create_utxo(
            value=change,
            tags=decayed_tags.copy(),  # Decay applies - can't distinguish from payment
            owner=sender_id
        )

    # Record fee (burned)
    state.total_fees += fee

    return True


def calculate_gini(balances: List[float]) -> float:
    """Calculate Gini coefficient."""
    if len(balances) < 2:
        return 0.0
    total = sum(balances)
    if total == 0:
        return 0.0
    sorted_b = sorted(balances)
    n = len(sorted_b)
    sum_idx = sum((i + 1) * b for i, b in enumerate(sorted_b))
    return max(0, min(1, (2 * sum_idx - (n + 1) * total) / (n * total)))


def record_metrics(state: SimState):
    """Record current metrics."""
    balances = [state.utxo_set.get_owner_balance(a.id) for a in state.agents]
    gini = calculate_gini(balances)
    state.gini_history.append((state.round, gini))

    # Compute average fee rates by agent type (using initial cluster wealth)
    rates_by_type = {'retail': [], 'merchant': [], 'whale': []}

    for agent in state.agents:
        utxos = state.utxo_set.get_owner_utxos(agent.id)
        if utxos:
            eff_wealth = compute_effective_wealth(utxos, state.initial_cluster_wealth)
            rate = state.fee_config.rate_bps(eff_wealth)
            rates_by_type[agent.agent_type].append(rate)

    avg_retail = np.mean(rates_by_type['retail']) if rates_by_type['retail'] else 0
    avg_merchant = np.mean(rates_by_type['merchant']) if rates_by_type['merchant'] else 0
    avg_whale = np.mean(rates_by_type['whale']) if rates_by_type['whale'] else 0
    state.avg_fee_rate_history.append((state.round, avg_retail, avg_merchant, avg_whale))

    # Compute average tag concentration (max tag weight per UTXO)
    max_weights = []
    for utxo in state.utxo_set.utxos.values():
        if utxo.tags:
            max_w = max(utxo.tags.values()) / TAG_SCALE
            max_weights.append(max_w)
    avg_concentration = np.mean(max_weights) if max_weights else 0
    state.tag_concentration_history.append((state.round, avg_concentration))


def run_round(state: SimState, rng: random.Random):
    """Run one round of economic activity."""
    agents = state.agents
    retail = [a for a in agents if a.agent_type == "retail"]
    merchants = [a for a in agents if a.agent_type == "merchant"]
    whales = [a for a in agents if a.agent_type == "whale"]

    # Retail purchases
    for r in retail:
        balance = state.utxo_set.get_owner_balance(r.id)
        if rng.random() < 0.20 and balance > 50 and merchants:
            amt = min(balance * 0.10, rng.uniform(20, 100))
            transfer(state, r.id, rng.choice(merchants).id, amt, rng)

    # Whale activity
    for w in whales:
        for _ in range(10):
            balance = state.utxo_set.get_owner_balance(w.id)
            if rng.random() < 0.30 and balance > 1000 and merchants:
                amt = min(balance * 0.03, rng.uniform(2000, 20000))
                transfer(state, w.id, rng.choice(merchants).id, amt, rng)

            balance = state.utxo_set.get_owner_balance(w.id)
            if rng.random() < 0.15 and retail and balance > 1000:
                amt = min(balance * 0.01, rng.uniform(1000, 5000))
                transfer(state, w.id, rng.choice(retail).id, amt, rng)

            balance = state.utxo_set.get_owner_balance(w.id)
            if rng.random() < 0.25 and len(whales) > 1 and balance > 5000:
                other = rng.choice([x for x in whales if x.id != w.id])
                amt = min(balance * 0.05, rng.uniform(10000, 50000))
                transfer(state, w.id, other.id, amt, rng)

    # Merchant wages
    for m in merchants:
        balance = state.utxo_set.get_owner_balance(m.id)
        if rng.random() < 0.25 and retail and balance > 100:
            amt = min(balance * 0.08, rng.uniform(200, 800))
            if amt > 0:
                transfer(state, m.id, rng.choice(retail).id, amt, rng)


def run_simulation(fee_config: FeeConfig, n_agents: int = 100, rounds: int = 500,
                   seed: int = 42) -> Tuple[SimState, dict]:
    """Run full simulation."""
    rng = random.Random(seed)
    state = create_initial_state(n_agents, fee_config, seed=seed)

    initial_supply = sum(state.utxo_set.get_owner_balance(a.id) for a in state.agents)
    record_metrics(state)

    for r in range(1, rounds + 1):
        state.round = r
        run_round(state, rng)
        if r % 50 == 0:
            record_metrics(state)
            # Progress indicator
            if r % 500 == 0:
                print(f"    Round {r}/{rounds}...")

    final_supply = sum(state.utxo_set.get_owner_balance(a.id) for a in state.agents)
    stats = {
        'initial_supply': initial_supply,
        'final_supply': final_supply,
        'total_burned': state.total_fees,
        'burn_pct': (state.total_fees / initial_supply) * 100 if initial_supply > 0 else 0,
        'num_utxos': len(state.utxo_set.utxos)
    }

    return state, stats


# =============================================================================
# Main
# =============================================================================

def main():
    output_dir = "./gini_10yr"
    os.makedirs(output_dir, exist_ok=True)

    print("=" * 70)
    print("UTXO-BASED PROGRESSIVE FEE SIMULATION")
    print("Coins tracked individually with per-UTXO tags")
    print("=" * 70)

    configs = [
        ("Flat 5%", FeeConfig("Flat 5%", 500, 500, 0, 1)),
        ("Prog 1%-30%", FeeConfig("Prog 1%-30%", 100, 3000, 0, 1)),
        ("Prog 0.1%-50%", FeeConfig("Prog 0.1%-50%", 10, 5000, 0, 1)),
    ]

    print("\nBURN MODE (fees destroyed)")
    print("-" * 60)

    results = {}
    for name, config in configs:
        print(f"  Running {name}...")
        state, stats = run_simulation(
            FeeConfig(name, config.r_min_bps, config.r_max_bps, 0, 1),
            n_agents=100,
            rounds=500
        )
        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        results[name] = (state, stats)

        print(f"  {name:20s}: GINI {initial:.3f} → {final:.3f} ({reduction:+.1f}%) | "
              f"burned {stats['burn_pct']:.1f}% | {stats['num_utxos']} UTXOs")

    # Plot results with diagnostics
    fig, axes = plt.subplots(2, 2, figsize=(14, 10))
    fig.suptitle('UTXO-Based Progressive Fees\n(Per-coin tag tracking with diagnostics)',
                 fontsize=12, fontweight='bold')

    # Plot 1: GINI over time
    ax1 = axes[0, 0]
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

    # Plot 2: Tag concentration decay
    ax2 = axes[0, 1]
    for name, (state, _) in results.items():
        rounds = [r for r, _ in state.tag_concentration_history]
        conc = [c for _, c in state.tag_concentration_history]
        linestyle = '--' if 'Flat' in name else '-'
        ax2.plot(rounds, conc, label=name, linestyle=linestyle, linewidth=2)

    ax2.set_xlabel('Round')
    ax2.set_ylabel('Avg Max Tag Weight')
    ax2.set_title('Tag Concentration Decay\n(1.0 = all coins have clear provenance)')
    ax2.legend()
    ax2.grid(True, alpha=0.3)

    # Plot 3: Whale fee rates over time (for first progressive config)
    ax3 = axes[1, 0]
    prog_name = [n for n in results.keys() if 'Prog' in n][0] if any('Prog' in n for n in results.keys()) else None
    if prog_name and prog_name in results:
        state, _ = results[prog_name]
        rounds = [r for r, _, _, _ in state.avg_fee_rate_history]
        retail_rates = [r for _, r, _, _ in state.avg_fee_rate_history]
        merchant_rates = [m for _, _, m, _ in state.avg_fee_rate_history]
        whale_rates = [w for _, _, _, w in state.avg_fee_rate_history]

        ax3.plot(rounds, retail_rates, label='Retail', linewidth=2, color='green')
        ax3.plot(rounds, merchant_rates, label='Merchant', linewidth=2, color='blue')
        ax3.plot(rounds, whale_rates, label='Whale', linewidth=2, color='red')

    ax3.set_xlabel('Round')
    ax3.set_ylabel('Avg Fee Rate (bps)')
    ax3.set_title(f'Fee Rates by Agent Type ({prog_name})')
    ax3.legend()
    ax3.grid(True, alpha=0.3)
    ax3.axhline(y=10, color='gray', linestyle=':', alpha=0.5, label='Min (10 bps)')
    ax3.axhline(y=3000, color='gray', linestyle=':', alpha=0.5, label='Max (3000 bps)')

    # Plot 4: Final comparison
    ax4 = axes[1, 1]
    names = list(results.keys())
    ginis = [results[n][0].gini_history[-1][1] for n in names]
    burns = [results[n][1]['burn_pct'] for n in names]

    x = np.arange(len(names))
    ax4.bar(x, ginis, color='steelblue', alpha=0.7)
    ax4.set_xticks(x)
    ax4.set_xticklabels(names, rotation=45, ha='right')
    ax4.set_ylabel('Final GINI')
    ax4.set_title('Final GINI Comparison')
    ax4.grid(True, alpha=0.3, axis='y')

    for i, (g, b) in enumerate(zip(ginis, burns)):
        ax4.text(i, g + 0.02, f'{b:.0f}%\nburned', ha='center', fontsize=8)

    plt.tight_layout()
    plt.savefig(f"{output_dir}/gini_utxo_model.png", dpi=150, bbox_inches='tight')
    print(f"\nPlot saved: {output_dir}/gini_utxo_model.png")

    # Print diagnostic summary
    print("\n" + "=" * 60)
    print("DIAGNOSTIC SUMMARY")
    print("=" * 60)
    for name, (state, _) in results.items():
        if state.tag_concentration_history:
            initial_conc = state.tag_concentration_history[0][1]
            final_conc = state.tag_concentration_history[-1][1]
            print(f"{name:20s}: Tag concentration {initial_conc:.3f} → {final_conc:.3f}")
        if state.avg_fee_rate_history:
            _, r, m, w = state.avg_fee_rate_history[-1]
            print(f"{'':20s}  Final avg rates: Retail {r:.0f} bps, Merchant {m:.0f} bps, Whale {w:.0f} bps")


if __name__ == "__main__":
    main()
