#!/usr/bin/env python3
"""
Single-step Gini test.

100 participants, power law wealth.
Each pays a random person a small amount.
Wealthy pay higher fees.
Does Gini go up or down?
"""

import random
import numpy as np

def calculate_gini(wealths):
    """Standard Gini coefficient."""
    if len(wealths) < 2:
        return 0.0
    total = sum(wealths)
    if total == 0:
        return 0.0
    sorted_w = sorted(wealths)
    n = len(sorted_w)
    sum_idx = sum((i + 1) * w for i, w in enumerate(sorted_w))
    return (2 * sum_idx - (n + 1) * total) / (n * total)


def progressive_fee_rate(wealth, max_wealth):
    """Higher wealth = higher fee rate. Returns rate as fraction (0-0.30)."""
    # Linear from 0.01 (1%) for poorest to 0.30 (30%) for wealthiest
    return 0.01 + 0.29 * (wealth / max_wealth)


def flat_fee_rate(wealth, max_wealth):
    """Everyone pays same rate."""
    return 0.05  # 5%


def run_single_step(n=100, seed=42, fee_fn=progressive_fee_rate):
    """Run single round where everyone pays someone."""
    rng = random.Random(seed)
    np.random.seed(seed)

    # Power law distribution (Pareto)
    # Shape parameter alpha=1.5 gives realistic wealth distribution
    wealths = (np.random.pareto(1.5, n) + 1) * 1000
    wealths = list(wealths)

    initial_gini = calculate_gini(wealths)
    initial_total = sum(wealths)
    max_wealth = max(wealths)

    print(f"Initial state:")
    print(f"  Total wealth: {initial_total:,.0f}")
    print(f"  Gini: {initial_gini:.4f}")
    print(f"  Top 10% share: {sum(sorted(wealths)[-10:]) / initial_total * 100:.1f}%")
    print(f"  Bottom 50% share: {sum(sorted(wealths)[:50]) / initial_total * 100:.1f}%")
    print()

    # Each person pays a random other person a small amount
    # Payment = 5% of their wealth
    total_fees = 0

    for i in range(n):
        sender_wealth = wealths[i]
        payment = sender_wealth * 0.05  # 5% of wealth

        # Fee based on sender's wealth
        fee_rate = fee_fn(sender_wealth, max_wealth)
        fee = payment * fee_rate

        # Pick random recipient (not self)
        recipient = rng.choice([j for j in range(n) if j != i])

        # Execute transfer: sender pays (payment + fee), recipient gets payment
        wealths[i] -= (payment + fee)
        wealths[recipient] += payment
        total_fees += fee

    final_gini = calculate_gini(wealths)
    final_total = sum(wealths)

    print(f"After one round:")
    print(f"  Total wealth: {final_total:,.0f} (burned {total_fees:,.0f})")
    print(f"  Gini: {final_gini:.4f}")
    print(f"  Top 10% share: {sum(sorted(wealths)[-10:]) / final_total * 100:.1f}%")
    print(f"  Bottom 50% share: {sum(sorted(wealths)[:50]) / final_total * 100:.1f}%")
    print()
    print(f"  Gini change: {final_gini - initial_gini:+.4f} ({'decreased' if final_gini < initial_gini else 'increased'})")

    return initial_gini, final_gini


def run_multiple_rounds(n=100, rounds=100, seed=42, fee_fn=progressive_fee_rate):
    """Run multiple rounds, return Gini history."""
    rng = random.Random(seed)
    np.random.seed(seed)

    # Power law distribution
    wealths = (np.random.pareto(1.5, n) + 1) * 1000
    wealths = list(wealths)

    gini_history = [calculate_gini(wealths)]

    for _ in range(rounds):
        max_wealth = max(wealths) if max(wealths) > 0 else 1

        for i in range(n):
            if wealths[i] <= 0:
                continue
            sender_wealth = wealths[i]
            payment = sender_wealth * 0.05

            fee_rate = fee_fn(sender_wealth, max_wealth)
            fee = payment * fee_rate

            recipient = rng.choice([j for j in range(n) if j != i])

            wealths[i] -= (payment + fee)
            wealths[recipient] += payment

        gini_history.append(calculate_gini(wealths))

    return gini_history


if __name__ == "__main__":
    print("=" * 60)
    print("MULTI-ROUND COMPARISON (100 rounds)")
    print("=" * 60)

    prog_history = run_multiple_rounds(fee_fn=progressive_fee_rate)
    flat_history = run_multiple_rounds(fee_fn=flat_fee_rate)

    print(f"\nProgressive (1%-30% fee):")
    print(f"  Round 0:   Gini = {prog_history[0]:.4f}")
    print(f"  Round 10:  Gini = {prog_history[10]:.4f}")
    print(f"  Round 50:  Gini = {prog_history[50]:.4f}")
    print(f"  Round 100: Gini = {prog_history[100]:.4f}")

    print(f"\nFlat (5% fee):")
    print(f"  Round 0:   Gini = {flat_history[0]:.4f}")
    print(f"  Round 10:  Gini = {flat_history[10]:.4f}")
    print(f"  Round 50:  Gini = {flat_history[50]:.4f}")
    print(f"  Round 100: Gini = {flat_history[100]:.4f}")

    print(f"\nFinal comparison:")
    print(f"  Progressive: {prog_history[0]:.4f} → {prog_history[-1]:.4f} (Δ = {prog_history[-1] - prog_history[0]:+.4f})")
    print(f"  Flat:        {flat_history[0]:.4f} → {flat_history[-1]:.4f} (Δ = {flat_history[-1] - flat_history[0]:+.4f})")
