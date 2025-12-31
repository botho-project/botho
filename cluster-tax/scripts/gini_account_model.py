#!/usr/bin/env python3
"""
Account-Based Progressive Fee Simulation

Matches the actual Botho implementation in transfer.rs:
- Account-based (not UTXO-based)
- Fee is FROM the transfer (receiver gets amount - fee)
- Only transferred tags get decayed
- Receiver mixes incoming tags with their existing tags
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
# Core Types (matching src/tag.rs)
# =============================================================================

TAG_SCALE = 1_000_000
TAG_PRUNE_THRESHOLD = 100  # 0.01%


def apply_decay(tags: Dict[int, int], decay_rate: int) -> Dict[int, int]:
    """Apply decay to tags, moving mass to background."""
    result = {}
    for cluster_id, weight in tags.items():
        decay_amount = weight * decay_rate // TAG_SCALE
        new_weight = weight - decay_amount
        if new_weight >= TAG_PRUNE_THRESHOLD:
            result[cluster_id] = new_weight
    return result


def mix_tags(self_tags: Dict[int, int], self_value: int,
             incoming_tags: Dict[int, int], incoming_value: int) -> Dict[int, int]:
    """Mix incoming tags into existing tags (value-weighted average)."""
    total_value = self_value + incoming_value
    if total_value == 0:
        return {}

    # Collect all clusters
    all_clusters = set(self_tags.keys()) | set(incoming_tags.keys())

    result = {}
    for cluster in all_clusters:
        self_weight = self_tags.get(cluster, 0)
        incoming_weight = incoming_tags.get(cluster, 0)

        # Weighted average
        new_weight = (self_value * self_weight + incoming_value * incoming_weight) // total_value
        if new_weight >= TAG_PRUNE_THRESHOLD:
            result[cluster] = new_weight

    return result


@dataclass
class Account:
    """An account with balance and tag vector."""
    id: int
    balance: int  # Using int for precision
    agent_type: str
    tags: Dict[int, int] = field(default_factory=dict)  # cluster_id -> weight


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


class ClusterWealth:
    """Tracks total wealth attributed to each cluster."""

    def __init__(self):
        self.wealth: Dict[int, int] = {}
        self.next_cluster = 0

    def new_cluster(self) -> int:
        cid = self.next_cluster
        self.next_cluster += 1
        self.wealth[cid] = 0
        return cid

    def apply_delta(self, cluster_id: int, delta: int):
        if cluster_id not in self.wealth:
            self.wealth[cluster_id] = 0
        self.wealth[cluster_id] = max(0, self.wealth[cluster_id] + delta)

    def get(self, cluster_id: int) -> int:
        return self.wealth.get(cluster_id, 0)


def effective_fee_rate(account: Account, cluster_wealth: ClusterWealth,
                       fee_config: FeeConfig) -> float:
    """Compute effective fee rate based on account's cluster attribution."""
    # Find MAX(tag_weight × cluster_wealth) across all clusters
    max_effective = 0.0
    for cluster_id, weight in account.tags.items():
        cw = cluster_wealth.get(cluster_id)
        effective = (weight / TAG_SCALE) * cw
        max_effective = max(max_effective, effective)

    return fee_config.rate_bps(max_effective)


# =============================================================================
# Transfer Logic (matching src/transfer.rs)
# =============================================================================

def execute_transfer(
    sender: Account,
    receiver: Account,
    amount: int,
    decay_rate: int,
    fee_config: FeeConfig,
    cluster_wealth: ClusterWealth
) -> Tuple[bool, int]:
    """
    Execute transfer matching real implementation.

    Returns (success, fee_burned)
    """
    if sender.balance < amount or amount <= 0:
        return False, 0

    # 1. Compute fee based on sender's effective rate
    fee_rate = effective_fee_rate(sender, cluster_wealth, fee_config)
    fee = int(amount * fee_rate / 10_000)
    net_amount = amount - fee

    # 2. Update sender balance
    sender.balance -= amount

    # 3. Compute tags for transferred coins (with decay)
    transferred_tags = apply_decay(sender.tags.copy(), decay_rate)

    # 4. Update cluster wealth
    # Mass leaving sender
    for cluster_id, weight in sender.tags.items():
        mass_leaving = amount * weight // TAG_SCALE
        cluster_wealth.apply_delta(cluster_id, -mass_leaving)

    # Mass arriving at receiver (after decay)
    for cluster_id, weight in transferred_tags.items():
        mass_arriving = net_amount * weight // TAG_SCALE
        cluster_wealth.apply_delta(cluster_id, mass_arriving)

    # 5. Mix into receiver's tags
    receiver_balance_before = receiver.balance
    receiver.tags = mix_tags(receiver.tags, receiver_balance_before,
                             transferred_tags, net_amount)
    receiver.balance += net_amount

    return True, fee


# =============================================================================
# Simulation
# =============================================================================

@dataclass
class SimState:
    """Simulation state."""
    accounts: List[Account]
    cluster_wealth: ClusterWealth
    fee_config: FeeConfig
    decay_rate: int = 50_000  # 5% per hop
    round: int = 0
    total_fees: int = 0
    gini_history: List[Tuple[int, float]] = field(default_factory=list)
    fee_rate_history: List[Tuple[int, float, float, float]] = field(default_factory=list)
    # Store original whale cluster IDs at initialization
    original_whale_clusters: List[int] = field(default_factory=list)
    # Track whale cluster wealth over time
    whale_cluster_wealth_history: List[Tuple[int, float]] = field(default_factory=list)


def create_accounts(n: int, cluster_wealth: ClusterWealth,
                    log_mean: float = 8.0, log_std: float = 1.8,
                    seed: int = 42) -> Tuple[List[Account], List[int]]:
    """Create accounts with lognormal wealth distribution.

    Returns (accounts, whale_cluster_ids)
    """
    rng = np.random.default_rng(seed)
    wealths = rng.lognormal(mean=log_mean, sigma=log_std, size=n)
    # Scale to integer (satoshis)
    wealths = (wealths * 1000).astype(int)
    sorted_idx = np.argsort(wealths)

    accounts = []
    whale_cluster_ids = []
    for i, idx in enumerate(sorted_idx):
        pct = i / n
        atype = "retail" if pct < 0.70 else ("merchant" if pct < 0.90 else "whale")

        # Each account starts with own cluster
        cluster_id = cluster_wealth.new_cluster()
        balance = int(wealths[idx])
        cluster_wealth.apply_delta(cluster_id, balance)

        account = Account(
            id=i,
            balance=balance,
            agent_type=atype,
            tags={cluster_id: TAG_SCALE}  # 100% attribution to own cluster
        )
        accounts.append(account)

        if atype == "whale":
            whale_cluster_ids.append(cluster_id)

    return accounts, whale_cluster_ids


def calculate_gini(balances: List[int]) -> float:
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
    balances = [a.balance for a in state.accounts]
    gini = calculate_gini(balances)
    state.gini_history.append((state.round, gini))

    # Average fee rates by type
    rates = {'retail': [], 'merchant': [], 'whale': []}
    for account in state.accounts:
        rate = effective_fee_rate(account, state.cluster_wealth, state.fee_config)
        rates[account.agent_type].append(rate)

    avg_r = np.mean(rates['retail']) if rates['retail'] else 0
    avg_m = np.mean(rates['merchant']) if rates['merchant'] else 0
    avg_w = np.mean(rates['whale']) if rates['whale'] else 0
    state.fee_rate_history.append((state.round, avg_r, avg_m, avg_w))

    # Track original whale clusters' total wealth
    if state.original_whale_clusters:
        total_whale_cluster_wealth = sum(
            state.cluster_wealth.get(cid) for cid in state.original_whale_clusters
        )
        state.whale_cluster_wealth_history.append((state.round, total_whale_cluster_wealth))


def run_round(state: SimState, rng: random.Random):
    """Run one round of economic activity."""
    accounts = state.accounts
    retail = [a for a in accounts if a.agent_type == "retail"]
    merchants = [a for a in accounts if a.agent_type == "merchant"]
    whales = [a for a in accounts if a.agent_type == "whale"]

    # Retail purchases
    for r in retail:
        if rng.random() < 0.20 and r.balance > 50000 and merchants:
            amt = min(r.balance // 10, int(rng.uniform(20000, 100000)))
            success, fee = execute_transfer(
                r, rng.choice(merchants), amt,
                state.decay_rate, state.fee_config, state.cluster_wealth
            )
            if success:
                state.total_fees += fee

    # Whale activity (high velocity)
    for w in whales:
        for _ in range(10):
            if rng.random() < 0.30 and w.balance > 1000000 and merchants:
                amt = min(w.balance * 3 // 100, int(rng.uniform(2000000, 20000000)))
                success, fee = execute_transfer(
                    w, rng.choice(merchants), amt,
                    state.decay_rate, state.fee_config, state.cluster_wealth
                )
                if success:
                    state.total_fees += fee

            if rng.random() < 0.15 and retail and w.balance > 1000000:
                amt = min(w.balance // 100, int(rng.uniform(1000000, 5000000)))
                success, fee = execute_transfer(
                    w, rng.choice(retail), amt,
                    state.decay_rate, state.fee_config, state.cluster_wealth
                )
                if success:
                    state.total_fees += fee

            if rng.random() < 0.25 and len(whales) > 1 and w.balance > 5000000:
                other = rng.choice([x for x in whales if x.id != w.id])
                amt = min(w.balance * 5 // 100, int(rng.uniform(10000000, 50000000)))
                success, fee = execute_transfer(
                    w, other, amt,
                    state.decay_rate, state.fee_config, state.cluster_wealth
                )
                if success:
                    state.total_fees += fee

    # Merchant wages to retail
    for m in merchants:
        if rng.random() < 0.25 and retail and m.balance > 100000:
            amt = min(m.balance * 8 // 100, int(rng.uniform(200000, 800000)))
            if amt > 0:
                success, fee = execute_transfer(
                    m, rng.choice(retail), amt,
                    state.decay_rate, state.fee_config, state.cluster_wealth
                )
                if success:
                    state.total_fees += fee


def run_simulation(fee_config: FeeConfig, n_agents: int = 200, rounds: int = 2000,
                   seed: int = 42) -> Tuple[SimState, dict]:
    """Run full simulation."""
    rng = random.Random(seed)
    cluster_wealth = ClusterWealth()
    accounts, whale_cluster_ids = create_accounts(n_agents, cluster_wealth, seed=seed)

    # Scale fee curve to wealth distribution
    balances = [a.balance for a in accounts]
    p90 = np.percentile(balances, 90)

    if fee_config.r_min_bps != fee_config.r_max_bps:
        fee_config.w_mid = p90 * 0.3
        fee_config.steepness = p90 * 0.15

    state = SimState(
        accounts=accounts,
        cluster_wealth=cluster_wealth,
        fee_config=fee_config,
        original_whale_clusters=whale_cluster_ids
    )
    initial_supply = sum(a.balance for a in accounts)
    record_metrics(state)

    for r in range(1, rounds + 1):
        state.round = r
        run_round(state, rng)
        if r % 50 == 0:
            record_metrics(state)
            if r % 500 == 0:
                print(f"    Round {r}/{rounds}...")

    final_supply = sum(a.balance for a in accounts)
    stats = {
        'initial_supply': initial_supply,
        'final_supply': final_supply,
        'total_burned': state.total_fees,
        'burn_pct': (state.total_fees / initial_supply) * 100 if initial_supply > 0 else 0,
    }

    return state, stats


# =============================================================================
# Main
# =============================================================================

def main():
    output_dir = "./gini_10yr"
    os.makedirs(output_dir, exist_ok=True)

    print("=" * 70)
    print("ACCOUNT-BASED PROGRESSIVE FEE SIMULATION")
    print("Matches actual Botho transfer.rs implementation")
    print("=" * 70)

    configs = [
        ("Flat 1%", FeeConfig("Flat 1%", 100, 100, 0, 1)),
        ("Prog 0.1%-5%", FeeConfig("Prog 0.1%-5%", 10, 500, 0, 1)),
        ("Prog 0.1%-10%", FeeConfig("Prog 0.1%-10%", 10, 1000, 0, 1)),
        ("Prog 0.1%-30%", FeeConfig("Prog 0.1%-30%", 10, 3000, 0, 1)),
    ]

    print("\nBURN MODE (fees destroyed)")
    print("-" * 60)

    results = {}
    for name, config in configs:
        print(f"  Running {name}...")
        state, stats = run_simulation(
            FeeConfig(name, config.r_min_bps, config.r_max_bps, 0, 1),
            n_agents=200,
            rounds=2000
        )
        initial = state.gini_history[0][1]
        final = state.gini_history[-1][1]
        reduction = (initial - final) / initial * 100
        results[name] = (state, stats)

        print(f"  {name:20s}: GINI {initial:.3f} → {final:.3f} ({reduction:+.1f}%) | "
              f"burned {stats['burn_pct']:.1f}%")

    # Plot results
    fig, axes = plt.subplots(2, 2, figsize=(14, 10))
    fig.suptitle('Account-Based Progressive Fees (matches transfer.rs)\n'
                 'Fee FROM transfer, decay on transferred tags only',
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

    # Plot 2: Fee rates by agent type (for Prog 0.1%-10%)
    ax2 = axes[0, 1]
    prog_name = "Prog 0.1%-10%"
    if prog_name in results:
        state, _ = results[prog_name]
        rounds = [r for r, _, _, _ in state.fee_rate_history]
        retail_rates = [r for _, r, _, _ in state.fee_rate_history]
        merchant_rates = [m for _, _, m, _ in state.fee_rate_history]
        whale_rates = [w for _, _, _, w in state.fee_rate_history]

        ax2.plot(rounds, retail_rates, label='Retail', linewidth=2, color='green')
        ax2.plot(rounds, merchant_rates, label='Merchant', linewidth=2, color='blue')
        ax2.plot(rounds, whale_rates, label='Whale', linewidth=2, color='red')

    ax2.set_xlabel('Round')
    ax2.set_ylabel('Avg Fee Rate (bps)')
    ax2.set_title(f'Fee Rates by Agent Type ({prog_name})')
    ax2.legend()
    ax2.grid(True, alpha=0.3)

    # Plot 3: Whale share over time
    ax3 = axes[1, 0]
    for name, (state, _) in results.items():
        rounds = [r for r, _ in state.gini_history]
        # Compute whale share at each recorded round
        whale_shares = []
        balances_history = []
        # We don't track this directly, so just plot GINI
        linestyle = '--' if 'Flat' in name else '-'

    # Use this for final GINI comparison
    ax3 = axes[1, 0]
    names = list(results.keys())
    final_ginis = [results[n][0].gini_history[-1][1] for n in names]
    burns = [results[n][1]['burn_pct'] for n in names]

    x = np.arange(len(names))
    ax3.bar(x, final_ginis, color='steelblue', alpha=0.7)
    ax3.set_xticks(x)
    ax3.set_xticklabels(names, rotation=45, ha='right')
    ax3.set_ylabel('Final GINI')
    ax3.set_title('Final GINI Comparison')
    ax3.grid(True, alpha=0.3, axis='y')

    for i, (g, b) in enumerate(zip(final_ginis, burns)):
        ax3.text(i, g + 0.02, f'{b:.0f}%\nburned', ha='center', fontsize=8)

    # Plot 4: Summary
    ax4 = axes[1, 1]
    ax4.axis('off')

    initial_gini = list(results.values())[0][0].gini_history[0][1]

    # Final fee rates for Prog 0.1%-10%
    if prog_name in results:
        state, _ = results[prog_name]
        _, final_r, final_m, final_w = state.fee_rate_history[-1]
        _, init_r, init_m, init_w = state.fee_rate_history[0]
    else:
        final_r, final_m, final_w = 0, 0, 0
        init_r, init_m, init_w = 0, 0, 0

    summary = f"""
KEY METRICS ({prog_name})

Initial Fee Rates:
  Retail:   {init_r:7.0f} bps
  Merchant: {init_m:7.0f} bps
  Whale:    {init_w:7.0f} bps

Final Fee Rates:
  Retail:   {final_r:7.0f} bps
  Merchant: {final_m:7.0f} bps
  Whale:    {final_w:7.0f} bps

Fee Rate Spread:
  Initial: {(init_w - init_r):.0f} bps (whale - retail)
  Final:   {(final_w - final_r):.0f} bps (whale - retail)

GINI:
  Initial: {initial_gini:.3f}
  Final:   {results[prog_name][0].gini_history[-1][1]:.3f}
"""
    ax4.text(0.05, 0.95, summary, transform=ax4.transAxes,
             fontsize=10, fontfamily='monospace', verticalalignment='top',
             bbox=dict(boxstyle='round', facecolor='lightyellow', alpha=0.8))

    plt.tight_layout()
    plt.savefig(f"{output_dir}/gini_account_model.png", dpi=150, bbox_inches='tight')
    print(f"\nPlot saved: {output_dir}/gini_account_model.png")

    # Print diagnostic summary
    print("\n" + "=" * 60)
    print("DIAGNOSTIC SUMMARY")
    print("=" * 60)
    for name, (state, _) in results.items():
        if state.fee_rate_history:
            _, r0, m0, w0 = state.fee_rate_history[0]
            _, rf, mf, wf = state.fee_rate_history[-1]
            print(f"{name:20s}:")
            print(f"  Initial rates: Retail {r0:.0f}, Merchant {m0:.0f}, Whale {w0:.0f} bps")
            print(f"  Final rates:   Retail {rf:.0f}, Merchant {mf:.0f}, Whale {wf:.0f} bps")
            print(f"  Spread:        {w0-r0:.0f} bps → {wf-rf:.0f} bps")
        if state.whale_cluster_wealth_history:
            _, initial_wcw = state.whale_cluster_wealth_history[0]
            _, final_wcw = state.whale_cluster_wealth_history[-1]
            pct_remaining = final_wcw / initial_wcw * 100 if initial_wcw > 0 else 0
            print(f"  Whale cluster wealth: {initial_wcw/1e6:.1f}M → {final_wcw/1e6:.1f}M ({pct_remaining:.1f}% remaining)")


if __name__ == "__main__":
    main()
