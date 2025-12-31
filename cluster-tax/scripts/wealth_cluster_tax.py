#!/usr/bin/env python3
"""
Wealth Cluster Tax Simulation

Key insight: we're taxing WEALTH CLUSTERS, not individuals.
Coins from wealthy sources carry fee burden that decays over hops.
This prevents sybil evasion - you can't escape fees by shuffling coins.
"""

import random
import numpy as np
from dataclasses import dataclass
from typing import List, Dict, Tuple
from collections import defaultdict


@dataclass
class UTXO:
    id: int
    value: int
    cluster_wealth: float  # Tracks "how wealthy was the source"
    owner: int


class Simulation:
    def __init__(self, n_agents: int = 100, seed: int = 42):
        self.rng = random.Random(seed)
        np.random.seed(seed)
        self.n_agents = n_agents
        self.utxo_counter = 0
        self.utxos: Dict[int, UTXO] = {}
        self.agent_utxos: Dict[int, List[int]] = defaultdict(list)
        self.total_fees_burned = 0

    def get_agent_wealth(self, agent: int) -> int:
        return sum(self.utxos[uid].value for uid in self.agent_utxos[agent])

    def mint(self, owner: int, value: int) -> UTXO:
        """Mint new coin. Cluster wealth = owner's total wealth at mint time."""
        owner_wealth = self.get_agent_wealth(owner) + value
        utxo = UTXO(
            id=self.utxo_counter,
            value=value,
            cluster_wealth=float(owner_wealth),  # Initial cluster = owner's wealth
            owner=owner
        )
        self.utxo_counter += 1
        self.utxos[utxo.id] = utxo
        self.agent_utxos[owner].append(utxo.id)
        return utxo

    def progressive_fee_rate(self, cluster_wealth: float, max_wealth: float) -> float:
        """Fee rate based on cluster wealth. Returns rate as fraction."""
        # Linear from 0.01 (1%) for poorest to 0.30 (30%) for wealthiest
        if max_wealth <= 0:
            return 0.01
        ratio = min(1.0, cluster_wealth / max_wealth)
        return 0.01 + 0.29 * ratio

    def flat_fee_rate(self, cluster_wealth: float, max_wealth: float) -> float:
        """Flat fee rate for comparison."""
        return 0.05  # 5%

    def transfer(self, sender: int, receiver: int, amount: int,
                 decay: float = 0.05, fee_fn=None) -> bool:
        """
        Transfer coins from sender to receiver.

        Key mechanics:
        - Fee based on cluster_wealth of coins being spent
        - Recipient inherits cluster_wealth (decayed)
        - This is anti-sybil: can't escape fees by shuffling
        """
        if fee_fn is None:
            fee_fn = self.progressive_fee_rate

        sender_utxos = [self.utxos[uid] for uid in self.agent_utxos[sender]]
        total = sum(u.value for u in sender_utxos)

        if total < amount:
            return False

        # Select UTXOs to spend (smallest first)
        to_spend = []
        spent_value = 0
        for u in sorted(sender_utxos, key=lambda x: x.value):
            to_spend.append(u)
            spent_value += u.value
            if spent_value >= amount:
                break

        # Compute value-weighted cluster wealth of inputs
        total_input_value = sum(u.value for u in to_spend)
        weighted_cluster = sum(u.value * u.cluster_wealth for u in to_spend) / total_input_value

        # Compute max wealth for fee scaling
        max_wealth = max(self.get_agent_wealth(a) for a in range(self.n_agents))

        # Fee based on cluster wealth
        fee_rate = fee_fn(weighted_cluster, max_wealth)
        fee = int(amount * fee_rate)

        if spent_value < amount + fee:
            return False

        # Remove spent UTXOs
        for u in to_spend:
            del self.utxos[u.id]
            self.agent_utxos[sender].remove(u.id)

        # Decay the cluster wealth attribution
        decayed_cluster = weighted_cluster * (1 - decay)

        # Create payment UTXO for receiver
        payment_utxo = UTXO(
            id=self.utxo_counter,
            value=amount,
            cluster_wealth=decayed_cluster,
            owner=receiver
        )
        self.utxo_counter += 1
        self.utxos[payment_utxo.id] = payment_utxo
        self.agent_utxos[receiver].append(payment_utxo.id)

        # Create change UTXO for sender (if any)
        change = spent_value - amount - fee
        if change > 0:
            change_utxo = UTXO(
                id=self.utxo_counter,
                value=change,
                cluster_wealth=decayed_cluster,
                owner=sender
            )
            self.utxo_counter += 1
            self.utxos[change_utxo.id] = change_utxo
            self.agent_utxos[sender].append(change_utxo.id)

        self.total_fees_burned += fee
        return True


def calculate_gini(wealths: List[int]) -> float:
    """Calculate Gini coefficient."""
    if len(wealths) < 2:
        return 0.0
    total = sum(wealths)
    if total == 0:
        return 0.0
    sorted_w = sorted(wealths)
    n = len(sorted_w)
    sum_idx = sum((i + 1) * w for i, w in enumerate(sorted_w))
    return (2 * sum_idx - (n + 1) * total) / (n * total)


def run_comparison(n_agents=100, rounds=500, seed=42):
    """Compare progressive cluster tax vs flat fee."""

    print("=" * 70)
    print("WEALTH CLUSTER TAX SIMULATION")
    print("=" * 70)
    print("""
Design principle: Tax WEALTH CLUSTERS, not individuals.
- Coins from wealthy sources carry fee burden
- Sybil accounts don't help - coins remember their origin
- Decay gradually reduces fee burden over many hops
""")

    for fee_name, fee_fn in [("Progressive (1-30%)", None),
                              ("Flat (5%)", lambda c, m: 0.05)]:
        np.random.seed(seed)
        sim = Simulation(n_agents=n_agents, seed=seed)

        # Power law initial distribution
        raw = np.random.pareto(0.7, n_agents) + 1
        wealths = (raw / raw.sum() * 10_000_000).astype(int)

        for agent, wealth in enumerate(wealths):
            sim.mint(agent, int(wealth))

        initial_gini = calculate_gini([sim.get_agent_wealth(a) for a in range(n_agents)])
        initial_total = sum(sim.get_agent_wealth(a) for a in range(n_agents))

        # Run simulation
        for _ in range(rounds):
            for _ in range(n_agents // 2):
                sender = sim.rng.randint(0, n_agents - 1)
                receiver = sim.rng.choice([i for i in range(n_agents) if i != sender])
                sender_wealth = sim.get_agent_wealth(sender)
                if sender_wealth > 100:
                    amount = sim.rng.randint(50, min(sender_wealth // 2, 10000))
                    sim.transfer(sender, receiver, amount, fee_fn=fee_fn)

        final_gini = calculate_gini([sim.get_agent_wealth(a) for a in range(n_agents)])
        final_total = sum(sim.get_agent_wealth(a) for a in range(n_agents))

        print(f"\n{fee_name}:")
        print(f"  Initial: Gini = {initial_gini:.4f}, Total = {initial_total:,}")
        print(f"  Final:   Gini = {final_gini:.4f}, Total = {final_total:,}")
        print(f"  Change:  ΔGini = {final_gini - initial_gini:+.4f}")
        print(f"  Burned:  {sim.total_fees_burned:,} ({sim.total_fees_burned/initial_total*100:.1f}%)")


def demonstrate_sybil_resistance():
    """Show that sybil accounts don't help avoid cluster tax."""

    print("\n" + "=" * 70)
    print("SYBIL RESISTANCE DEMONSTRATION")
    print("=" * 70)

    sim = Simulation(n_agents=20, seed=42)

    # Agent 0: Whale with 1M
    sim.mint(0, 1_000_000)
    # Agents 1-9: Normal people with 1K each
    for i in range(1, 10):
        sim.mint(i, 1_000)
    # Agents 10-19: Whale's sybil accounts (empty)

    print("\nSetup:")
    print(f"  Agent 0 (whale): {sim.get_agent_wealth(0):,}")
    print(f"  Agents 1-9 (normal): {sim.get_agent_wealth(1):,} each")
    print(f"  Agents 10-19 (sybils): empty")

    # Whale tries to launder through sybils
    print("\nWhale tries to move 100K through sybil chain...")

    # Check cluster_wealth at each step
    whale_utxo = sim.utxos[sim.agent_utxos[0][0]]
    print(f"\n  Step 0: Whale's UTXO cluster_wealth = {whale_utxo.cluster_wealth:,.0f}")

    # Whale → Sybil1 (use flat fee to simplify demo)
    flat_fee = lambda c, m: 0.01
    sim.transfer(0, 10, 100_000, fee_fn=flat_fee)
    sybil1_utxo = sim.utxos[sim.agent_utxos[10][0]]
    print(f"  Step 1: Sybil1's UTXO cluster_wealth = {sybil1_utxo.cluster_wealth:,.0f}")

    # Sybil1 → Sybil2
    sim.transfer(10, 11, 95_000, fee_fn=flat_fee)
    sybil2_utxo = sim.utxos[sim.agent_utxos[11][0]]
    print(f"  Step 2: Sybil2's UTXO cluster_wealth = {sybil2_utxo.cluster_wealth:,.0f}")

    # Sybil2 → Sybil3
    sim.transfer(11, 12, 90_000, fee_fn=flat_fee)
    sybil3_utxo = sim.utxos[sim.agent_utxos[12][0]]
    print(f"  Step 3: Sybil3's UTXO cluster_wealth = {sybil3_utxo.cluster_wealth:,.0f}")

    # Compare to direct transfer from normal person
    sim.transfer(1, 15, 500, fee_fn=flat_fee)
    normal_utxo = sim.utxos[sim.agent_utxos[15][0]]
    print(f"\n  Normal person → Sybil: cluster_wealth = {normal_utxo.cluster_wealth:,.0f}")

    print(f"""
RESULT:
- After 3 hops through sybils, cluster_wealth is still {sybil3_utxo.cluster_wealth:,.0f}
- Compare to {normal_utxo.cluster_wealth:,.0f} from a normal 1K account
- Whale's coins still carry ~{sybil3_utxo.cluster_wealth/normal_utxo.cluster_wealth:.0f}x higher fee burden
- Sybil laundering doesn't help! The coins remember their wealthy origin.
""")


def demonstrate_decay_over_time():
    """Show how cluster wealth decays through legitimate commerce."""

    print("\n" + "=" * 70)
    print("DECAY THROUGH LEGITIMATE COMMERCE")
    print("=" * 70)

    sim = Simulation(n_agents=50, seed=42)

    # One whale, many merchants
    sim.mint(0, 1_000_000)
    for i in range(1, 50):
        sim.mint(i, 10_000)

    print("\nScenario: Whale's coins flow through economy...")
    print("Tracking cluster_wealth as coins change hands legitimately\n")

    # Whale buys from Merchant1
    sim.transfer(0, 1, 50_000)
    m1_utxo = sim.utxos[sim.agent_utxos[1][-1]]
    print(f"Whale → Merchant1 (50K): cluster_wealth = {m1_utxo.cluster_wealth:,.0f}")

    # Merchant1 pays Supplier2
    sim.transfer(1, 2, 40_000)
    m2_utxo = sim.utxos[sim.agent_utxos[2][-1]]
    print(f"Merchant1 → Supplier2 (40K): cluster_wealth = {m2_utxo.cluster_wealth:,.0f}")

    # Supplier2 pays Worker3
    sim.transfer(2, 3, 30_000)
    m3_utxo = sim.utxos[sim.agent_utxos[3][-1]]
    print(f"Supplier2 → Worker3 (30K): cluster_wealth = {m3_utxo.cluster_wealth:,.0f}")

    # Worker3 buys from Retailer4
    sim.transfer(3, 4, 20_000)
    m4_utxo = sim.utxos[sim.agent_utxos[4][-1]]
    print(f"Worker3 → Retailer4 (20K): cluster_wealth = {m4_utxo.cluster_wealth:,.0f}")

    # More hops...
    current_holder = 4
    current_amount = 15_000
    for i in range(5, 20):
        sim.transfer(current_holder, i, current_amount)
        utxo = sim.utxos[sim.agent_utxos[i][-1]]
        if i % 3 == 0:
            print(f"... hop {i}: cluster_wealth = {utxo.cluster_wealth:,.0f}")
        current_holder = i
        current_amount = int(current_amount * 0.8)

    final_utxo = sim.utxos[sim.agent_utxos[19][-1]]
    initial = 1_000_000

    print(f"""
RESULT:
- Started at cluster_wealth = {initial:,} (whale's balance)
- After 19 hops: cluster_wealth = {final_utxo.cluster_wealth:,.0f}
- Decay factor: {final_utxo.cluster_wealth / initial:.4f} ({final_utxo.cluster_wealth / initial * 100:.2f}%)
- With 5% decay per hop: expected 0.95^19 = {0.95**19:.4f}

The "whale tax" fades naturally through commerce.
Fresh whale money = expensive. Well-circulated money = cheap.
""")


if __name__ == "__main__":
    run_comparison()
    demonstrate_sybil_resistance()
    demonstrate_decay_over_time()
