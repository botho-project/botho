#!/usr/bin/env python3
"""
Debug script to understand UTXO fee behavior.

Key question: Why does progressive burn more but reduce Gini less?
"""

import random
import numpy as np
from dataclasses import dataclass, field
from typing import Dict, List, Tuple
from collections import defaultdict

TAG_SCALE = 1_000_000
TAG_PRUNE_THRESHOLD = 1000  # 0.1%

@dataclass
class UTXO:
    id: int
    value: float
    tags: Dict[int, int]  # cluster_id -> weight
    owner: int

@dataclass
class Agent:
    id: int
    initial_wealth: float
    agent_type: str

class Simulation:
    def __init__(self, seed=42):
        self.rng = random.Random(seed)
        np.random.seed(seed)
        self.utxos: Dict[int, UTXO] = {}
        self.by_owner: Dict[int, List[int]] = defaultdict(list)
        self.next_id = 0
        self.agents: List[Agent] = []
        self.initial_cluster_wealth: Dict[int, float] = {}

    def create_agent(self, wealth: float, agent_type: str) -> Agent:
        agent = Agent(id=len(self.agents), initial_wealth=wealth, agent_type=agent_type)
        self.agents.append(agent)

        # Create UTXO with 100% tag to own cluster
        cluster_id = agent.id
        utxo = UTXO(
            id=self.next_id,
            value=wealth,
            tags={cluster_id: TAG_SCALE},
            owner=agent.id
        )
        self.next_id += 1
        self.utxos[utxo.id] = utxo
        self.by_owner[agent.id].append(utxo.id)

        # Record initial cluster wealth
        self.initial_cluster_wealth[cluster_id] = wealth

        return agent

    def get_balance(self, owner: int) -> float:
        return sum(self.utxos[uid].value for uid in self.by_owner[owner] if uid in self.utxos)

    def get_utxos(self, owner: int) -> List[UTXO]:
        return [self.utxos[uid] for uid in self.by_owner[owner] if uid in self.utxos]

    def compute_effective_wealth_MAX(self, utxos: List[UTXO]) -> float:
        """Current method: MAX(tag_weight × cluster_wealth)"""
        if not utxos:
            return 0.0
        total_value = sum(u.value for u in utxos)
        if total_value == 0:
            return 0.0

        # Blend tags
        blended: Dict[int, float] = {}
        for utxo in utxos:
            for cid, weight in utxo.tags.items():
                if cid not in blended:
                    blended[cid] = 0.0
                blended[cid] += (utxo.value / total_value) * (weight / TAG_SCALE)

        # MAX
        max_wealth = 0.0
        for cid, tag_weight in blended.items():
            if cid in self.initial_cluster_wealth:
                eff = tag_weight * self.initial_cluster_wealth[cid]
                max_wealth = max(max_wealth, eff)
        return max_wealth

    def compute_effective_wealth_SUM(self, utxos: List[UTXO]) -> float:
        """Alternative: SUM(tag_weight × cluster_wealth) - total provenance"""
        if not utxos:
            return 0.0
        total_value = sum(u.value for u in utxos)
        if total_value == 0:
            return 0.0

        blended: Dict[int, float] = {}
        for utxo in utxos:
            for cid, weight in utxo.tags.items():
                if cid not in blended:
                    blended[cid] = 0.0
                blended[cid] += (utxo.value / total_value) * (weight / TAG_SCALE)

        # SUM
        total_wealth = 0.0
        for cid, tag_weight in blended.items():
            if cid in self.initial_cluster_wealth:
                total_wealth += tag_weight * self.initial_cluster_wealth[cid]
        return total_wealth

    def compute_effective_wealth_DOMINANT(self, utxos: List[UTXO]) -> float:
        """Alternative: Use only the dominant cluster's wealth"""
        if not utxos:
            return 0.0
        total_value = sum(u.value for u in utxos)
        if total_value == 0:
            return 0.0

        blended: Dict[int, float] = {}
        for utxo in utxos:
            for cid, weight in utxo.tags.items():
                if cid not in blended:
                    blended[cid] = 0.0
                blended[cid] += (utxo.value / total_value) * (weight / TAG_SCALE)

        # Find dominant cluster (highest tag weight)
        if not blended:
            return 0.0
        dominant_cid = max(blended.keys(), key=lambda k: blended[k])
        dominant_weight = blended[dominant_cid]

        if dominant_cid in self.initial_cluster_wealth:
            return dominant_weight * self.initial_cluster_wealth[dominant_cid]
        return 0.0


def analyze_tag_spreading():
    """Show how tags spread through the economy."""
    print("=" * 70)
    print("TAG SPREADING ANALYSIS")
    print("=" * 70)

    sim = Simulation(seed=42)

    # Create simple economy: 1 whale, 2 merchants, 7 retail
    whale = sim.create_agent(1_000_000, "whale")
    merchants = [sim.create_agent(10_000, "merchant") for _ in range(2)]
    retail = [sim.create_agent(1_000, "retail") for _ in range(7)]

    print("\nInitial state:")
    print(f"  Whale (id={whale.id}): {sim.get_balance(whale.id):,.0f}")
    print(f"  Merchants: {[sim.get_balance(m.id) for m in merchants]}")
    print(f"  Retail: {[sim.get_balance(r.id) for r in retail]}")

    def show_effective_wealth(label):
        print(f"\n{label}")
        for agent in sim.agents:
            utxos = sim.get_utxos(agent.id)
            if utxos:
                eff_max = sim.compute_effective_wealth_MAX(utxos)
                eff_sum = sim.compute_effective_wealth_SUM(utxos)
                eff_dom = sim.compute_effective_wealth_DOMINANT(utxos)
                print(f"  {agent.agent_type:8s} {agent.id}: MAX={eff_max:>12,.0f}  SUM={eff_sum:>12,.0f}  DOM={eff_dom:>12,.0f}")

    show_effective_wealth("Initial effective wealth:")

    # Simple transfer function (no fees for this analysis)
    def transfer(sender_id, receiver_id, amount, decay=0.05):
        sender_utxos = sim.get_utxos(sender_id)
        if not sender_utxos:
            return False

        # Use first UTXO
        utxo = sender_utxos[0]
        if utxo.value < amount:
            return False

        # Blend and decay tags
        decayed_tags = {}
        for cid, weight in utxo.tags.items():
            new_weight = int(weight * (1 - decay))
            if new_weight >= TAG_PRUNE_THRESHOLD:
                decayed_tags[cid] = new_weight

        # Remove old UTXO
        del sim.utxos[utxo.id]
        sim.by_owner[sender_id].remove(utxo.id)

        # Create payment UTXO
        payment = UTXO(
            id=sim.next_id,
            value=amount,
            tags=decayed_tags.copy(),
            owner=receiver_id
        )
        sim.next_id += 1
        sim.utxos[payment.id] = payment
        sim.by_owner[receiver_id].append(payment.id)

        # Create change UTXO
        change_val = utxo.value - amount
        if change_val > 0:
            change = UTXO(
                id=sim.next_id,
                value=change_val,
                tags=decayed_tags.copy(),
                owner=sender_id
            )
            sim.next_id += 1
            sim.utxos[change.id] = change
            sim.by_owner[sender_id].append(change.id)

        return True

    # Round 1: Whale pays both merchants
    print("\n" + "-" * 50)
    print("Round 1: Whale → Merchants")
    transfer(whale.id, merchants[0].id, 50_000)
    transfer(whale.id, merchants[1].id, 50_000)
    show_effective_wealth("After whale pays merchants:")

    # Round 2: Merchants pay retail
    print("\n" + "-" * 50)
    print("Round 2: Merchants → Retail")
    for i, m in enumerate(merchants):
        for j in range(3):
            target = retail[(i * 3 + j) % len(retail)]
            transfer(m.id, target.id, 5_000)
    show_effective_wealth("After merchants pay retail:")

    # Round 3: Retail pays each other
    print("\n" + "-" * 50)
    print("Round 3: Retail → Retail")
    for i in range(len(retail)):
        transfer(retail[i].id, retail[(i + 1) % len(retail)].id, 1_000)
    show_effective_wealth("After retail pays each other:")

    # Analysis
    print("\n" + "=" * 70)
    print("ANALYSIS")
    print("=" * 70)
    print("""
The problem with MAX aggregation:
- Even a small trace of whale tag causes high effective_wealth
- Retail who received from merchant (who received from whale) inherits tag
- MAX picks up this whale attribution even if it's only 5% of their coins

Compare the three methods:
- MAX: Sensitive to ANY wealthy provenance (even small)
- SUM: Accumulates all provenance (scales with mixing)
- DOMINANT: Only considers the strongest attribution

For progressive fees to work correctly, we want:
- Whale's coins to pay high fees
- Direct recipients (merchants) to pay medium fees
- Distant recipients (retail) to pay low fees

This requires effective_wealth to decay with distance from wealthy source.
""")

    # Show tag composition for one retail agent
    print("\nDetailed tag breakdown for retail agent:")
    r = retail[0]
    utxos = sim.get_utxos(r.id)
    for utxo in utxos:
        print(f"  UTXO {utxo.id}: value={utxo.value:,.0f}")
        for cid, weight in utxo.tags.items():
            source_wealth = sim.initial_cluster_wealth.get(cid, 0)
            source_type = "whale" if source_wealth > 100_000 else ("merchant" if source_wealth > 5_000 else "retail")
            pct = weight / TAG_SCALE * 100
            contrib = (weight / TAG_SCALE) * source_wealth
            print(f"    cluster {cid} ({source_type}): {pct:.1f}% weight × {source_wealth:,.0f} wealth = {contrib:,.0f} contribution")


def analyze_fee_rates():
    """Show how fee rates are distributed."""
    print("\n" + "=" * 70)
    print("FEE RATE ANALYSIS")
    print("=" * 70)

    import math

    def sigmoid_rate(effective_wealth, r_min_bps, r_max_bps, w_mid, steepness):
        if r_min_bps == r_max_bps:
            return r_min_bps
        x = (effective_wealth - w_mid) / max(steepness, 1)
        sigmoid = 1 / (1 + math.exp(-x)) if -700 < x < 700 else (0 if x < 0 else 1)
        return r_min_bps + (r_max_bps - r_min_bps) * sigmoid

    # Simulated effective wealth values from tag spreading
    test_values = [
        ("Whale (direct)", 900_000),
        ("Merchant (received whale)", 750_000),
        ("Retail (received from merchant)", 750_000),
        ("Retail (own coins only)", 1_000),
        ("Poor retail", 500),
    ]

    # Fee curve calibrated to initial wealth distribution
    # In simulation: w_mid = p90 * 0.3, steepness = p90 * 0.15
    # If p90 of initial wealth ~ 50,000, then w_mid ~ 15,000, steepness ~ 7,500
    w_mid = 15_000
    steepness = 7_500

    print(f"\nFee curve: w_mid={w_mid:,}, steepness={steepness:,}")
    print(f"Rate range: 10 bps (0.1%) to 5000 bps (50%)\n")

    print(f"{'Agent Type':<35} {'Eff Wealth':>15} {'Fee Rate':>12}")
    print("-" * 65)
    for name, eff_w in test_values:
        rate = sigmoid_rate(eff_w, 10, 5000, w_mid, steepness)
        print(f"{name:<35} {eff_w:>15,} {rate:>10.0f} bps ({rate/100:.1f}%)")

    print("""
PROBLEM IDENTIFIED:
- w_mid is calibrated to INITIAL wealth (~15,000)
- But effective_wealth after tag spreading is much higher (~750,000)
- Everyone ends up FAR above w_mid
- So everyone pays near-maximum fees!

SOLUTIONS:
1. Calibrate w_mid to expected EFFECTIVE wealth distribution
2. Use a different aggregation that doesn't inflate effective_wealth
3. Scale fee by (tag_weight × cluster_wealth) / total_wealth to normalize
""")


def test_alternative_approach():
    """Test simpler 'inherited wealth' tracking on UTXO."""
    print("\n" + "=" * 70)
    print("ALTERNATIVE: INHERITED WEALTH MODEL")
    print("=" * 70)

    print("""
Instead of tracking multiple cluster tags, track a single value:
  inherited_wealth = weighted average of source's wealth at transfer time

This decays naturally through mixing and doesn't require cluster tracking.
""")

    sim = Simulation(seed=42)

    # Store inherited_wealth directly on UTXO
    inherited_wealth = {}  # utxo_id -> inherited_wealth value

    whale = sim.create_agent(1_000_000, "whale")
    merchants = [sim.create_agent(10_000, "merchant") for _ in range(2)]
    retail = [sim.create_agent(1_000, "retail") for _ in range(7)]

    # Initialize inherited_wealth = owner's initial balance
    for uid in sim.utxos:
        utxo = sim.utxos[uid]
        inherited_wealth[uid] = sim.agents[utxo.owner].initial_wealth

    def get_effective_wealth(owner_id):
        """Get value-weighted inherited wealth for an owner."""
        total_value = 0
        weighted_sum = 0
        for uid in sim.by_owner[owner_id]:
            if uid in sim.utxos:
                u = sim.utxos[uid]
                total_value += u.value
                weighted_sum += u.value * inherited_wealth[uid]
        return weighted_sum / total_value if total_value > 0 else 0

    def transfer_with_inherited(sender_id, receiver_id, amount, decay=0.05):
        """Transfer with inherited wealth tracking."""
        sender_utxos = [sim.utxos[uid] for uid in sim.by_owner[sender_id] if uid in sim.utxos]
        if not sender_utxos:
            return False

        utxo = sender_utxos[0]
        if utxo.value < amount:
            return False

        source_inherited = inherited_wealth[utxo.id]
        decayed_inherited = source_inherited * (1 - decay)

        # Remove old UTXO
        del sim.utxos[utxo.id]
        del inherited_wealth[utxo.id]
        sim.by_owner[sender_id].remove(utxo.id)

        # Create payment
        payment_id = sim.next_id
        sim.next_id += 1
        payment = UTXO(id=payment_id, value=amount, tags={}, owner=receiver_id)
        sim.utxos[payment_id] = payment
        sim.by_owner[receiver_id].append(payment_id)
        inherited_wealth[payment_id] = decayed_inherited

        # Create change
        change_val = utxo.value - amount
        if change_val > 0:
            change_id = sim.next_id
            sim.next_id += 1
            change = UTXO(id=change_id, value=change_val, tags={}, owner=sender_id)
            sim.utxos[change_id] = change
            sim.by_owner[sender_id].append(change_id)
            inherited_wealth[change_id] = decayed_inherited

        return True

    def show_state(label):
        print(f"\n{label}")
        for agent in sim.agents:
            eff = get_effective_wealth(agent.id)
            balance = sim.get_balance(agent.id)
            print(f"  {agent.agent_type:8s} {agent.id}: balance={balance:>10,.0f}  inherited_wealth={eff:>12,.0f}")

    show_state("Initial state:")

    # Round 1: Whale → Merchants
    transfer_with_inherited(whale.id, merchants[0].id, 50_000)
    transfer_with_inherited(whale.id, merchants[1].id, 50_000)
    show_state("After whale → merchants:")

    # Round 2: Merchants → Retail
    for i, m in enumerate(merchants):
        for j in range(3):
            target = retail[(i * 3 + j) % len(retail)]
            transfer_with_inherited(m.id, target.id, 5_000)
    show_state("After merchants → retail:")

    # Round 3: More mixing
    for i in range(len(retail)):
        transfer_with_inherited(retail[i].id, retail[(i + 1) % len(retail)].id, 1_000)
    show_state("After retail → retail:")

    print("""
KEY DIFFERENCE:
- inherited_wealth decays with each transfer
- Whale: still high (they kept their coins)
- Merchants: medium (received whale coins, decayed)
- Retail: lower (further from whale source)

This naturally creates the desired gradient without complex cluster tracking!
""")


if __name__ == "__main__":
    analyze_tag_spreading()
    analyze_fee_rates()
    test_alternative_approach()
