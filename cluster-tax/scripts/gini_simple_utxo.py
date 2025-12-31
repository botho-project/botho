#!/usr/bin/env python3
"""
Simple UTXO-Based Progressive Fee Simulation

Bitcoin-like transactions with single inherited_wealth per UTXO.
Fee based on value-weighted average of inputs being spent.
"""

import os
import random
import math
from dataclasses import dataclass, field
from typing import Dict, List, Tuple
from collections import defaultdict

import numpy as np


@dataclass
class UTXO:
    """UTXO with source wealth tracking."""
    id: int
    value: float
    source_wealth: float  # Wealth of original minter (FIXED)
    tag_weight: float     # Attribution weight (DECAYS)
    owner: int

    @property
    def effective_wealth(self) -> float:
        """Effective wealth = source_wealth × tag_weight"""
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


def progressive_fee_rate(effective_wealth: float, max_initial_wealth: float,
                         r_min: float = 0.001, r_max: float = 0.30) -> float:
    """
    Progressive fee rate based on effective wealth.

    Linear interpolation: poorest pays r_min, wealthiest pays r_max.
    """
    if max_initial_wealth <= 0:
        return r_min
    ratio = min(1.0, effective_wealth / max_initial_wealth)
    return r_min + (r_max - r_min) * ratio


def flat_fee_rate(effective_wealth: float, max_initial_wealth: float,
                  rate: float = 0.05) -> float:
    """Flat fee rate for comparison."""
    return rate


def transfer(utxo_set: UTXOSet, sender: int, receiver: int, amount: float,
             max_wealth: float, decay: float = 0.05,
             fee_fn=progressive_fee_rate) -> Tuple[bool, float]:
    """
    Transfer amount from sender to receiver.

    Key design:
    - source_wealth BLENDS (value-weighted average of inputs)
    - tag_weight DECAYS with each transfer
    - effective_wealth = source_wealth × tag_weight
    - Fee based on effective_wealth of inputs

    Returns (success, fee_paid).
    """
    sender_utxos = utxo_set.get_owner_utxos(sender)
    total_balance = sum(u.value for u in sender_utxos)

    if total_balance < amount or amount <= 0:
        return False, 0

    # Select inputs (smallest first to consolidate UTXOs)
    selected = []
    selected_value = 0
    for utxo in sorted(sender_utxos, key=lambda u: u.value):
        selected.append(utxo)
        selected_value += utxo.value
        if selected_value >= amount * 1.5:  # Some buffer for fees
            break

    if not selected:
        return False, 0

    # Compute value-weighted average effective_wealth of inputs
    total_input = sum(u.value for u in selected)
    avg_effective = sum(u.value * u.effective_wealth for u in selected) / total_input

    # Compute fee based on average effective wealth
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

    # Compute blended source_wealth (mixes) and tag_weight (decays)
    avg_source_wealth = sum(u.value * u.source_wealth for u in selected) / total_input
    avg_tag_weight = sum(u.value * u.tag_weight for u in selected) / total_input

    # Only decay when coins change hands (receiver != sender)
    # But we can't know which output is change with stealth addresses...
    # So we decay ALL outputs but at a rate proportional to how much
    # value actually left (amount / total_input)
    transfer_fraction = amount / total_input
    effective_decay = decay * transfer_fraction  # Partial decay
    new_tag_weight = avg_tag_weight * (1 - effective_decay)

    # Spend selected inputs
    for utxo in selected:
        utxo_set.spend(utxo.id)

    # Create payment output (to receiver)
    utxo_set.create(value=amount, source_wealth=avg_source_wealth,
                    tag_weight=new_tag_weight, owner=receiver)

    # Create change output (back to sender)
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


def run_simulation(n_agents: int = 100, rounds: int = 500, seed: int = 42,
                   fee_fn=progressive_fee_rate, decay: float = 0.05,
                   r_min: float = 0.01, r_max: float = 0.30) -> dict:
    """Run simulation with given fee function."""
    rng = random.Random(seed)
    np.random.seed(seed)

    # Create agents with Pareto wealth distribution
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
        # Initial UTXO: source_wealth = initial_wealth, tag_weight = 1.0
        utxo_set.create(value=wealths[idx], source_wealth=wealths[idx],
                        tag_weight=1.0, owner=agent.id)

    max_wealth = max(a.initial_wealth for a in agents)
    initial_supply = sum(utxo_set.get_balance(a.id) for a in agents)

    # Custom fee function wrapper with our r_min/r_max
    def custom_fee_fn(iw, mw):
        if fee_fn == flat_fee_rate:
            return (r_min + r_max) / 2  # Use average for flat
        return progressive_fee_rate(iw, mw, r_min, r_max)

    retail = [a for a in agents if a.agent_type == "retail"]
    merchants = [a for a in agents if a.agent_type == "merchant"]
    whales = [a for a in agents if a.agent_type == "whale"]

    gini_history = []
    total_fees = 0

    def record():
        balances = [utxo_set.get_balance(a.id) for a in agents]
        gini_history.append(calculate_gini(balances))

    record()

    for r in range(rounds):
        # Retail purchases
        for agent in retail:
            balance = utxo_set.get_balance(agent.id)
            if rng.random() < 0.20 and balance > 50 and merchants:
                amt = min(balance * 0.10, rng.uniform(20, 100))
                success, fee = transfer(utxo_set, agent.id, rng.choice(merchants).id,
                                        amt, max_wealth, decay, custom_fee_fn)
                if success:
                    total_fees += fee

        # Whale activity
        for whale in whales:
            for _ in range(10):
                balance = utxo_set.get_balance(whale.id)
                if rng.random() < 0.30 and balance > 1000 and merchants:
                    amt = min(balance * 0.03, rng.uniform(2000, 20000))
                    success, fee = transfer(utxo_set, whale.id, rng.choice(merchants).id,
                                            amt, max_wealth, decay, custom_fee_fn)
                    if success:
                        total_fees += fee

                balance = utxo_set.get_balance(whale.id)
                if rng.random() < 0.15 and retail and balance > 1000:
                    amt = min(balance * 0.01, rng.uniform(1000, 5000))
                    success, fee = transfer(utxo_set, whale.id, rng.choice(retail).id,
                                            amt, max_wealth, decay, custom_fee_fn)
                    if success:
                        total_fees += fee

                balance = utxo_set.get_balance(whale.id)
                if rng.random() < 0.25 and len(whales) > 1 and balance > 5000:
                    other = rng.choice([w for w in whales if w.id != whale.id])
                    amt = min(balance * 0.05, rng.uniform(10000, 50000))
                    success, fee = transfer(utxo_set, whale.id, other.id,
                                            amt, max_wealth, decay, custom_fee_fn)
                    if success:
                        total_fees += fee

        # Merchant wages
        for merchant in merchants:
            balance = utxo_set.get_balance(merchant.id)
            if rng.random() < 0.25 and retail and balance > 100:
                amt = min(balance * 0.08, rng.uniform(200, 800))
                success, fee = transfer(utxo_set, merchant.id, rng.choice(retail).id,
                                        amt, max_wealth, decay, custom_fee_fn)
                if success:
                    total_fees += fee

        if r % 50 == 0:
            record()

    record()

    final_supply = sum(utxo_set.get_balance(a.id) for a in agents)

    # Compute final average effective_wealth by type
    avg_effective = {'retail': [], 'merchant': [], 'whale': []}
    avg_tag_weight = {'retail': [], 'merchant': [], 'whale': []}
    for agent in agents:
        utxos = utxo_set.get_owner_utxos(agent.id)
        if utxos:
            total_val = sum(u.value for u in utxos)
            avg_ew = sum(u.value * u.effective_wealth for u in utxos) / total_val
            avg_tw = sum(u.value * u.tag_weight for u in utxos) / total_val
            avg_effective[agent.agent_type].append(avg_ew)
            avg_tag_weight[agent.agent_type].append(avg_tw)

    return {
        'gini_history': gini_history,
        'initial_gini': gini_history[0],
        'final_gini': gini_history[-1],
        'initial_supply': initial_supply,
        'final_supply': final_supply,
        'total_fees': total_fees,
        'burn_pct': total_fees / initial_supply * 100,
        'avg_effective_retail': np.mean(avg_effective['retail']) if avg_effective['retail'] else 0,
        'avg_effective_merchant': np.mean(avg_effective['merchant']) if avg_effective['merchant'] else 0,
        'avg_effective_whale': np.mean(avg_effective['whale']) if avg_effective['whale'] else 0,
        'avg_tag_weight_retail': np.mean(avg_tag_weight['retail']) if avg_tag_weight['retail'] else 0,
        'avg_tag_weight_whale': np.mean(avg_tag_weight['whale']) if avg_tag_weight['whale'] else 0,
    }


def main():
    print("=" * 70)
    print("SIMPLE UTXO PROGRESSIVE FEE SIMULATION")
    print("Single inherited_wealth per UTXO, fee on average of inputs spent")
    print("=" * 70)

    configs = [
        ("Flat 5%", flat_fee_rate, 0.05, 0.05),
        ("Prog 0.5%-5%", progressive_fee_rate, 0.005, 0.05),
        ("Prog 0.5%-10%", progressive_fee_rate, 0.005, 0.10),
        ("Prog 1%-15%", progressive_fee_rate, 0.01, 0.15),
    ]

    print(f"\n{'Config':<20} {'Initial':>8} {'Final':>8} {'ΔGini':>8} {'Burned':>8} {'Whale EW':>12} {'Retail EW':>12} {'Whale TW':>8}")
    print("-" * 100)

    results = {}
    for name, fee_fn, r_min, r_max in configs:
        result = run_simulation(
            n_agents=100, rounds=500, seed=42,
            fee_fn=fee_fn, decay=0.05, r_min=r_min, r_max=r_max
        )
        results[name] = result

        delta = result['final_gini'] - result['initial_gini']
        print(f"{name:<20} {result['initial_gini']:>8.4f} {result['final_gini']:>8.4f} "
              f"{delta:>+8.4f} {result['burn_pct']:>7.1f}% "
              f"{result['avg_effective_whale']:>12,.0f} {result['avg_effective_retail']:>12,.0f} "
              f"{result['avg_tag_weight_whale']:>8.2%}")

    print("\n" + "=" * 70)
    print("ANALYSIS")
    print("=" * 70)

    flat = results["Flat 5%"]
    prog = results["Prog 1%-15%"]

    whale_retail_ratio = prog['avg_effective_whale'] / prog['avg_effective_retail'] if prog['avg_effective_retail'] > 0 else 0

    print(f"""
Flat 5%:
  Gini: {flat['initial_gini']:.4f} → {flat['final_gini']:.4f} (Δ = {flat['final_gini'] - flat['initial_gini']:+.4f})
  Burned: {flat['burn_pct']:.1f}%
  Final effective_wealth: Whale={flat['avg_effective_whale']:,.0f}, Retail={flat['avg_effective_retail']:,.0f}
  Tag weight remaining: Whale={flat['avg_tag_weight_whale']:.1%}, Retail={flat['avg_tag_weight_retail']:.1%}

Progressive 1%-15%:
  Gini: {prog['initial_gini']:.4f} → {prog['final_gini']:.4f} (Δ = {prog['final_gini'] - prog['initial_gini']:+.4f})
  Burned: {prog['burn_pct']:.1f}%
  Final effective_wealth: Whale={prog['avg_effective_whale']:,.0f}, Retail={prog['avg_effective_retail']:,.0f}
  Tag weight remaining: Whale={prog['avg_tag_weight_whale']:.1%}, Retail={prog['avg_tag_weight_retail']:.1%}

Key metrics:
  Progressive vs Flat Gini reduction: {(flat['final_gini'] - prog['final_gini']) / flat['final_gini'] * 100:+.1f}%
  Progressive burns: {prog['burn_pct'] - flat['burn_pct']:+.1f}% {'more' if prog['burn_pct'] > flat['burn_pct'] else 'less'} supply
  Whale/Retail effective_wealth ratio: {whale_retail_ratio:.1f}x
""")


if __name__ == "__main__":
    main()
