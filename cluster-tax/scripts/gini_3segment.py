#!/usr/bin/env python3
"""
3-Segment Piecewise Linear Fee Curve Simulation

Tests the ZK-compatible fee model:
- Segment 1 (Poor):   [0, w1)      → flat low rate
- Segment 2 (Middle): [w1, w2)     → linear interpolation
- Segment 3 (Rich):   [w2, ∞)      → flat high rate

This "flat-linear-flat" model preserves S-curve economics while being ZK-provable.
"""

import random
from dataclasses import dataclass
from typing import Dict, List, Tuple
from collections import defaultdict

import numpy as np


@dataclass
class UTXO:
    """UTXO with source wealth tracking."""
    id: int
    value: float
    source_wealth: float
    tag_weight: float
    owner: int

    @property
    def effective_wealth(self) -> float:
        return self.source_wealth * self.tag_weight


@dataclass
class Agent:
    id: int
    initial_wealth: float
    agent_type: str


class UTXOSet:
    def __init__(self):
        self.utxos: Dict[int, UTXO] = {}
        self.by_owner: Dict[int, List[int]] = defaultdict(list)
        self.next_id = 0

    def create(self, value: float, source_wealth: float, tag_weight: float, owner: int) -> UTXO:
        utxo = UTXO(id=self.next_id, value=value, source_wealth=source_wealth,
                    tag_weight=tag_weight, owner=owner)
        self.next_id += 1
        self.utxos[utxo.id] = utxo
        self.by_owner[owner].append(utxo.id)
        return utxo

    def spend(self, utxo_id: int):
        if utxo_id in self.utxos:
            owner = self.utxos[utxo_id].owner
            del self.utxos[utxo_id]
            self.by_owner[owner] = [u for u in self.by_owner[owner] if u != utxo_id]

    def get_owner_utxos(self, owner: int) -> List[UTXO]:
        return [self.utxos[uid] for uid in self.by_owner.get(owner, []) if uid in self.utxos]

    def get_balance(self, owner: int) -> float:
        return sum(u.value for u in self.get_owner_utxos(owner))


# =============================================================================
# FEE CURVE IMPLEMENTATIONS
# =============================================================================

def flat_fee_rate(effective_wealth: float, max_wealth: float, rate: float = 0.05) -> float:
    """Flat fee rate for comparison."""
    return rate


def linear_fee_rate(effective_wealth: float, max_wealth: float,
                    r_min: float = 0.01, r_max: float = 0.15) -> float:
    """Pure linear interpolation from r_min to r_max."""
    if max_wealth <= 0:
        return r_min
    ratio = min(1.0, effective_wealth / max_wealth)
    return r_min + (r_max - r_min) * ratio


def three_segment_fee_rate(effective_wealth: float, max_wealth: float,
                           w1_frac: float = 0.1,   # boundary 1 as fraction of max
                           w2_frac: float = 0.5,   # boundary 2 as fraction of max
                           r_poor: float = 0.01,   # rate for poor segment
                           r_mid_start: float = 0.02,  # rate at start of middle
                           r_mid_end: float = 0.12,    # rate at end of middle
                           r_rich: float = 0.15) -> float:
    """
    3-segment piecewise linear fee rate (flat-linear-flat).

    Segment 1 (Poor):   [0, w1)     → flat at r_poor
    Segment 2 (Middle): [w1, w2)    → linear from r_mid_start to r_mid_end
    Segment 3 (Rich):   [w2, ∞)     → flat at r_rich
    """
    if max_wealth <= 0:
        return r_poor

    w1 = max_wealth * w1_frac
    w2 = max_wealth * w2_frac

    if effective_wealth < w1:
        # Poor segment: flat low rate
        return r_poor
    elif effective_wealth < w2:
        # Middle segment: linear interpolation
        t = (effective_wealth - w1) / (w2 - w1)
        return r_mid_start + t * (r_mid_end - r_mid_start)
    else:
        # Rich segment: flat high rate
        return r_rich


def sigmoid_fee_rate(effective_wealth: float, max_wealth: float,
                     r_min: float = 0.01, r_max: float = 0.15,
                     steepness: float = 5.0) -> float:
    """
    Sigmoid fee rate (current production model).
    For comparison with piecewise approximation.
    """
    if max_wealth <= 0:
        return r_min

    # Normalize wealth to [-6, 6] range centered at midpoint
    w_mid = max_wealth * 0.5
    x = steepness * (effective_wealth - w_mid) / max_wealth

    # Clamp to prevent overflow
    x = max(-10, min(10, x))

    # Sigmoid
    sigmoid = 1.0 / (1.0 + np.exp(-x))

    return r_min + (r_max - r_min) * sigmoid


# =============================================================================
# TRANSFER LOGIC
# =============================================================================

def transfer(utxo_set: UTXOSet, sender: int, receiver: int, amount: float,
             max_wealth: float, decay: float = 0.05,
             fee_fn=linear_fee_rate) -> Tuple[bool, float]:
    """Transfer with provenance tracking."""
    sender_utxos = utxo_set.get_owner_utxos(sender)
    total_balance = sum(u.value for u in sender_utxos)

    if total_balance < amount or amount <= 0:
        return False, 0

    # Select inputs
    selected = []
    selected_value = 0
    for utxo in sorted(sender_utxos, key=lambda u: u.value):
        selected.append(utxo)
        selected_value += utxo.value
        if selected_value >= amount * 1.5:
            break

    if not selected:
        return False, 0

    # Compute blended effective_wealth
    total_input = sum(u.value for u in selected)
    avg_effective = sum(u.value * u.effective_wealth for u in selected) / total_input

    # Compute fee
    rate = fee_fn(avg_effective, max_wealth)
    fee = amount * rate
    total_needed = amount + fee

    # Add more inputs if needed
    if selected_value < total_needed:
        remaining = [u for u in sender_utxos if u not in selected]
        for utxo in sorted(remaining, key=lambda u: u.value):
            selected.append(utxo)
            selected_value += utxo.value
            total_input = sum(u.value for u in selected)
            avg_effective = sum(u.value * u.effective_wealth for u in selected) / total_input
            rate = fee_fn(avg_effective, max_wealth)
            fee = amount * rate
            total_needed = amount + fee
            if selected_value >= total_needed:
                break

    if selected_value < total_needed:
        return False, 0

    # Compute blended values
    avg_source_wealth = sum(u.value * u.source_wealth for u in selected) / total_input
    avg_tag_weight = sum(u.value * u.tag_weight for u in selected) / total_input

    # Partial decay
    transfer_fraction = amount / total_input
    effective_decay = decay * transfer_fraction
    new_tag_weight = avg_tag_weight * (1 - effective_decay)

    # Spend inputs
    for utxo in selected:
        utxo_set.spend(utxo.id)

    # Create outputs
    utxo_set.create(value=amount, source_wealth=avg_source_wealth,
                    tag_weight=new_tag_weight, owner=receiver)

    change = selected_value - total_needed
    if change > 0.01:
        utxo_set.create(value=change, source_wealth=avg_source_wealth,
                        tag_weight=new_tag_weight, owner=sender)

    return True, fee


def calculate_gini(values: List[float]) -> float:
    if len(values) < 2:
        return 0.0
    total = sum(values)
    if total == 0:
        return 0.0
    sorted_v = sorted(values)
    n = len(sorted_v)
    sum_idx = sum((i + 1) * v for i, v in enumerate(sorted_v))
    return max(0, min(1, (2 * sum_idx - (n + 1) * total) / (n * total)))


# =============================================================================
# SIMULATION
# =============================================================================

def run_simulation(n_agents: int = 100, rounds: int = 500, seed: int = 42,
                   fee_fn=linear_fee_rate, decay: float = 0.05,
                   label: str = "unnamed") -> dict:
    """Run simulation with given fee function."""
    rng = random.Random(seed)
    np.random.seed(seed)

    # Pareto wealth distribution
    raw = np.random.pareto(0.7, n_agents) + 1
    wealths = (raw / raw.sum() * 10_000_000).astype(float)
    sorted_idx = np.argsort(wealths)

    utxo_set = UTXOSet()
    agents = []

    for i, idx in enumerate(sorted_idx):
        pct = i / n_agents
        atype = "retail" if pct < 0.70 else ("merchant" if pct < 0.90 else "whale")
        agent = Agent(id=i, initial_wealth=wealths[idx], agent_type=atype)
        agents.append(agent)
        utxo_set.create(value=wealths[idx], source_wealth=wealths[idx],
                        tag_weight=1.0, owner=agent.id)

    max_wealth = max(a.initial_wealth for a in agents)
    initial_supply = sum(utxo_set.get_balance(a.id) for a in agents)

    retail = [a for a in agents if a.agent_type == "retail"]
    merchants = [a for a in agents if a.agent_type == "merchant"]
    whales = [a for a in agents if a.agent_type == "whale"]

    total_fees = 0

    for r in range(rounds):
        # Retail purchases
        for agent in retail:
            balance = utxo_set.get_balance(agent.id)
            if rng.random() < 0.20 and balance > 50 and merchants:
                amt = min(balance * 0.10, rng.uniform(20, 100))
                success, fee = transfer(utxo_set, agent.id, rng.choice(merchants).id,
                                        amt, max_wealth, decay, fee_fn)
                if success:
                    total_fees += fee

        # Whale activity
        for whale in whales:
            for _ in range(10):
                balance = utxo_set.get_balance(whale.id)
                if rng.random() < 0.30 and balance > 1000 and merchants:
                    amt = min(balance * 0.03, rng.uniform(2000, 20000))
                    success, fee = transfer(utxo_set, whale.id, rng.choice(merchants).id,
                                            amt, max_wealth, decay, fee_fn)
                    if success:
                        total_fees += fee

                balance = utxo_set.get_balance(whale.id)
                if rng.random() < 0.15 and retail and balance > 1000:
                    amt = min(balance * 0.01, rng.uniform(1000, 5000))
                    success, fee = transfer(utxo_set, whale.id, rng.choice(retail).id,
                                            amt, max_wealth, decay, fee_fn)
                    if success:
                        total_fees += fee

                balance = utxo_set.get_balance(whale.id)
                if rng.random() < 0.25 and len(whales) > 1 and balance > 5000:
                    other = rng.choice([w for w in whales if w.id != whale.id])
                    amt = min(balance * 0.05, rng.uniform(10000, 50000))
                    success, fee = transfer(utxo_set, whale.id, other.id,
                                            amt, max_wealth, decay, fee_fn)
                    if success:
                        total_fees += fee

        # Merchant wages
        for merchant in merchants:
            balance = utxo_set.get_balance(merchant.id)
            if rng.random() < 0.25 and retail and balance > 100:
                amt = min(balance * 0.08, rng.uniform(200, 800))
                success, fee = transfer(utxo_set, merchant.id, rng.choice(retail).id,
                                        amt, max_wealth, decay, fee_fn)
                if success:
                    total_fees += fee

    # Final metrics
    final_balances = [utxo_set.get_balance(a.id) for a in agents]
    initial_balances = [a.initial_wealth for a in agents]

    return {
        'label': label,
        'initial_gini': calculate_gini(initial_balances),
        'final_gini': calculate_gini(final_balances),
        'total_fees': total_fees,
        'burn_pct': total_fees / initial_supply * 100,
    }


def main():
    print("=" * 70)
    print("3-SEGMENT PIECEWISE LINEAR FEE CURVE SIMULATION")
    print("Comparing: Flat vs Linear vs 3-Segment vs Sigmoid")
    print("=" * 70)

    # Define fee functions to test
    configs = [
        ("Flat 5%", lambda ew, mw: flat_fee_rate(ew, mw, rate=0.05)),
        ("Linear 1%-15%", lambda ew, mw: linear_fee_rate(ew, mw, r_min=0.01, r_max=0.15)),
        ("Sigmoid", lambda ew, mw: sigmoid_fee_rate(ew, mw)),
        # 3-segment variants - tuning for optimal Gini/burn tradeoff
        ("3-Seg wide", lambda ew, mw: three_segment_fee_rate(
            ew, mw, w1_frac=0.10, w2_frac=0.60,  # wider middle segment
            r_poor=0.01, r_mid_start=0.01, r_mid_end=0.12, r_rich=0.15)),
        ("3-Seg balanced", lambda ew, mw: three_segment_fee_rate(
            ew, mw, w1_frac=0.15, w2_frac=0.70,  # even wider, less aggressive
            r_poor=0.01, r_mid_start=0.02, r_mid_end=0.10, r_rich=0.15)),
        ("3-Seg sigmoid-match", lambda ew, mw: three_segment_fee_rate(
            ew, mw, w1_frac=0.20, w2_frac=0.80,  # matches sigmoid shape
            r_poor=0.02, r_mid_start=0.03, r_mid_end=0.12, r_rich=0.14)),
    ]

    print(f"\n{'Model':<20} {'Init Gini':>10} {'Final Gini':>10} {'ΔGini':>10} {'Burned':>10}")
    print("-" * 62)

    results = []
    for name, fee_fn in configs:
        result = run_simulation(
            n_agents=100, rounds=500, seed=42,
            fee_fn=fee_fn, decay=0.05, label=name
        )
        results.append(result)

        delta = result['final_gini'] - result['initial_gini']
        print(f"{name:<20} {result['initial_gini']:>10.4f} {result['final_gini']:>10.4f} "
              f"{delta:>+10.4f} {result['burn_pct']:>9.1f}%")

    # Analysis
    print("\n" + "=" * 70)
    print("ANALYSIS")
    print("=" * 70)

    flat = next(r for r in results if r['label'] == "Flat 5%")
    linear = next(r for r in results if r['label'] == "Linear 1%-15%")
    sigmoid = next(r for r in results if r['label'] == "Sigmoid")
    seg3_wide = next(r for r in results if r['label'] == "3-Seg wide")
    seg3_balanced = next(r for r in results if r['label'] == "3-Seg balanced")
    seg3_match = next(r for r in results if r['label'] == "3-Seg sigmoid-match")

    def delta(r):
        return r['final_gini'] - r['initial_gini']

    print(f"""
Gini Reduction Comparison (more negative = better):
  Flat 5%:           {delta(flat):+.4f}  (burn: {flat['burn_pct']:.1f}%)
  Linear 1%-15%:     {delta(linear):+.4f}  (burn: {linear['burn_pct']:.1f}%)
  Sigmoid:           {delta(sigmoid):+.4f}  (burn: {sigmoid['burn_pct']:.1f}%)
  3-Seg wide:        {delta(seg3_wide):+.4f}  (burn: {seg3_wide['burn_pct']:.1f}%)
  3-Seg balanced:    {delta(seg3_balanced):+.4f}  (burn: {seg3_balanced['burn_pct']:.1f}%)
  3-Seg sigmoid-match:{delta(seg3_match):+.4f}  (burn: {seg3_match['burn_pct']:.1f}%)

Best 3-Seg vs Sigmoid: {(delta(seg3_match) - delta(sigmoid)) / abs(delta(sigmoid)) * 100:+.1f}% Gini diff
Best 3-Seg vs Sigmoid: {(seg3_match['burn_pct'] - sigmoid['burn_pct']):+.1f}% burn diff
""")

    # Visualize the fee curves
    print("\n" + "=" * 70)
    print("FEE CURVE VISUALIZATION (rate at different wealth levels)")
    print("=" * 70)

    max_w = 1_000_000
    levels = [0, 50_000, 100_000, 200_000, 400_000, 600_000, 800_000, 1_000_000]

    print(f"\n{'Wealth':>12} {'Flat':>8} {'Linear':>8} {'3-Seg':>8} {'Sigmoid':>8}")
    print("-" * 48)
    for w in levels:
        f_flat = flat_fee_rate(w, max_w, rate=0.05)
        f_linear = linear_fee_rate(w, max_w, r_min=0.01, r_max=0.15)
        f_3seg = three_segment_fee_rate(w, max_w, w1_frac=0.20, w2_frac=0.80,
                                        r_poor=0.02, r_mid_start=0.03,
                                        r_mid_end=0.12, r_rich=0.14)
        f_sigmoid = sigmoid_fee_rate(w, max_w)
        print(f"{w:>12,} {f_flat:>7.1%} {f_linear:>7.1%} {f_3seg:>7.1%} {f_sigmoid:>7.1%}")


if __name__ == "__main__":
    main()
