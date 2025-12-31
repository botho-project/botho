#!/usr/bin/env python3
"""
Public Tag Drift Simulation

Exploring: each minted coin gets random u64, outputs get weighted average.
Questions:
1. How fast do tags converge?
2. Can tag distance/distribution indicate wealth concentration?
3. What privacy does ring selection with similar tags provide?
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
    tag: float  # tag value in [0, 1) - represents u64 normalized
    owner: int  # agent who owns it

class Simulation:
    def __init__(self, n_agents: int = 100, seed: int = 42):
        self.rng = random.Random(seed)
        np.random.seed(seed)
        self.n_agents = n_agents
        self.utxo_counter = 0
        self.utxos: Dict[int, UTXO] = {}  # utxo_id -> UTXO
        self.agent_utxos: Dict[int, List[int]] = defaultdict(list)  # agent -> [utxo_ids]

    def mint(self, owner: int, value: int) -> UTXO:
        """Mint new coin with random tag."""
        # Use float in [0, 1) range to avoid overflow in weighted averages
        # In real impl would be u64, but math is equivalent
        utxo = UTXO(
            id=self.utxo_counter,
            value=value,
            tag=self.rng.random(),  # Random in [0, 1)
            owner=owner
        )
        self.utxo_counter += 1
        self.utxos[utxo.id] = utxo
        self.agent_utxos[owner].append(utxo.id)
        return utxo

    def transfer(self, sender: int, receiver: int, amount: int) -> bool:
        """Transfer amount from sender to receiver."""
        # Gather sender's UTXOs
        sender_utxos = [self.utxos[uid] for uid in self.agent_utxos[sender]]
        total = sum(u.value for u in sender_utxos)

        if total < amount:
            return False

        # Select UTXOs to spend (greedy)
        to_spend = []
        spent_value = 0
        for u in sorted(sender_utxos, key=lambda x: x.value):
            to_spend.append(u)
            spent_value += u.value
            if spent_value >= amount:
                break

        # Calculate weighted average tag of inputs
        total_input_value = sum(u.value for u in to_spend)
        weighted_tag_sum = sum(float(u.value) * u.tag for u in to_spend)
        avg_tag = weighted_tag_sum / total_input_value

        # Remove spent UTXOs
        for u in to_spend:
            del self.utxos[u.id]
            self.agent_utxos[sender].remove(u.id)

        # Create output UTXOs with averaged tag
        # Payment to receiver
        payment_utxo = UTXO(
            id=self.utxo_counter,
            value=amount,
            tag=avg_tag,  # Inherits weighted average
            owner=receiver
        )
        self.utxo_counter += 1
        self.utxos[payment_utxo.id] = payment_utxo
        self.agent_utxos[receiver].append(payment_utxo.id)

        # Change back to sender (if any)
        change = spent_value - amount
        if change > 0:
            change_utxo = UTXO(
                id=self.utxo_counter,
                value=change,
                tag=avg_tag,  # Same tag as payment
                owner=sender
            )
            self.utxo_counter += 1
            self.utxos[change_utxo.id] = change_utxo
            self.agent_utxos[sender].append(change_utxo.id)

        return True

    def get_agent_wealth(self, agent: int) -> int:
        return sum(self.utxos[uid].value for uid in self.agent_utxos[agent])

    def get_agent_tags(self, agent: int) -> List[Tuple[int, int]]:
        """Return [(tag, value), ...] for agent's UTXOs."""
        return [(self.utxos[uid].tag, self.utxos[uid].value) for uid in self.agent_utxos[agent]]

    def get_all_tags(self) -> List[int]:
        """Return all tag values in circulation."""
        return [u.tag for u in self.utxos.values()]

    def tag_statistics(self) -> dict:
        """Compute statistics about tag distribution."""
        tags = np.array(self.get_all_tags(), dtype=np.float64)
        if len(tags) == 0:
            return {}
        return {
            'mean': np.mean(tags),
            'std': np.std(tags),
            'min': np.min(tags),
            'max': np.max(tags),
            'median': np.median(tags),
            'n_unique': len(set(round(t, 6) for t in tags)),  # Approximate uniqueness
            'n_total': len(tags)
        }


def run_simulation(n_agents=100, initial_wealth_pareto=0.7, rounds=1000, seed=42):
    """Run simulation and track tag evolution."""
    sim = Simulation(n_agents=n_agents, seed=seed)

    # Initial distribution: Pareto wealth, each agent gets one UTXO
    np.random.seed(seed)
    raw = np.random.pareto(initial_wealth_pareto, n_agents) + 1
    wealths = (raw / raw.sum() * 10_000_000).astype(int)  # 10M total

    for agent, wealth in enumerate(wealths):
        sim.mint(agent, wealth)

    print("=" * 70)
    print("TAG DRIFT SIMULATION")
    print("=" * 70)
    print(f"\nInitial state: {n_agents} agents, 10M coins")
    print(f"Each agent has 1 UTXO with random u64 tag")

    initial_stats = sim.tag_statistics()
    print(f"\nInitial tag distribution:")
    print(f"  Unique tags: {initial_stats['n_unique']}")
    print(f"  Std dev: {initial_stats['std']:.2e}")

    # Track tag convergence
    tag_std_history = [initial_stats['std']]
    unique_tags_history = [initial_stats['n_unique']]

    # Run rounds
    for round_num in range(1, rounds + 1):
        # Random transactions
        for _ in range(n_agents // 2):  # Half of agents transact each round
            sender = sim.rng.randint(0, n_agents - 1)
            receiver = sim.rng.choice([i for i in range(n_agents) if i != sender])
            sender_wealth = sim.get_agent_wealth(sender)
            if sender_wealth > 100:
                amount = sim.rng.randint(50, min(sender_wealth // 2, 10000))
                sim.transfer(sender, receiver, amount)

        if round_num % 100 == 0:
            stats = sim.tag_statistics()
            tag_std_history.append(stats['std'])
            unique_tags_history.append(stats['n_unique'])

    final_stats = sim.tag_statistics()
    print(f"\nAfter {rounds} rounds:")
    print(f"  Unique tags: {final_stats['n_unique']} (was {initial_stats['n_unique']})")
    print(f"  Std dev: {final_stats['std']:.2e} (was {initial_stats['std']:.2e})")
    print(f"  Total UTXOs: {final_stats['n_total']}")

    # Analyze wealth vs tag homogeneity
    print(f"\n" + "-" * 70)
    print("WEALTH vs TAG ANALYSIS")
    print("-" * 70)

    # Sort agents by wealth
    agent_data = []
    for agent in range(n_agents):
        wealth = sim.get_agent_wealth(agent)
        tags = sim.get_agent_tags(agent)
        if tags:
            tag_values = [t for t, v in tags]
            tag_std = np.std(tag_values) if len(tag_values) > 1 else 0
            avg_tag = np.mean(tag_values)
            agent_data.append({
                'agent': agent,
                'wealth': wealth,
                'n_utxos': len(tags),
                'tag_std': tag_std,
                'avg_tag': avg_tag
            })

    agent_data.sort(key=lambda x: x['wealth'], reverse=True)

    print("\nTop 10 wealthiest agents:")
    print(f"{'Agent':<8} {'Wealth':>12} {'#UTXOs':>8} {'Tag Std':>15} {'Avg Tag':>20}")
    for d in agent_data[:10]:
        print(f"{d['agent']:<8} {d['wealth']:>12,} {d['n_utxos']:>8} {d['tag_std']:>15.2e} {d['avg_tag']:>20.2e}")

    print("\nBottom 10 (by wealth):")
    for d in agent_data[-10:]:
        print(f"{d['agent']:<8} {d['wealth']:>12,} {d['n_utxos']:>8} {d['tag_std']:>15.2e} {d['avg_tag']:>20.2e}")

    # Key question: do rich people have more homogeneous tags?
    top_10_pct = agent_data[:n_agents//10]
    bottom_50_pct = agent_data[n_agents//2:]

    top_tags = []
    for d in top_10_pct:
        tags = sim.get_agent_tags(d['agent'])
        top_tags.extend([t for t, v in tags])

    bottom_tags = []
    for d in bottom_50_pct:
        tags = sim.get_agent_tags(d['agent'])
        bottom_tags.extend([t for t, v in tags])

    if top_tags and bottom_tags:
        print(f"\nTag spread by wealth bracket:")
        print(f"  Top 10% wealth: {len(top_tags)} UTXOs, tag std = {np.std(top_tags):.2e}")
        print(f"  Bottom 50% wealth: {len(bottom_tags)} UTXOs, tag std = {np.std(bottom_tags):.2e}")

    return sim, tag_std_history, unique_tags_history


def analyze_ring_selection(sim: Simulation):
    """Analyze privacy implications of similar-tag ring selection."""
    print(f"\n" + "=" * 70)
    print("RING SELECTION ANALYSIS")
    print("=" * 70)

    # Get all UTXOs sorted by tag
    all_utxos = list(sim.utxos.values())
    all_utxos.sort(key=lambda u: u.tag)

    # For each UTXO, find closest 10 by tag (potential ring members)
    print("\nSample: 5 random UTXOs and their closest neighbors by tag")
    samples = sim.rng.sample(all_utxos, min(5, len(all_utxos)))

    for sample in samples:
        # Find closest by tag
        distances = [(abs(u.tag - sample.tag), u) for u in all_utxos if u.id != sample.id]
        distances.sort(key=lambda x: x[0])
        closest = distances[:10]

        print(f"\n  UTXO {sample.id}: tag={sample.tag:.2e}, value={sample.value}, owner={sample.owner}")
        print(f"  Closest 10 by tag:")
        same_owner = sum(1 for _, u in closest if u.owner == sample.owner)
        print(f"    Same owner in ring: {same_owner}/10")
        for dist, u in closest[:5]:
            owner_mark = " <-- SAME OWNER" if u.owner == sample.owner else ""
            print(f"      dist={dist:.2e}, owner={u.owner}, value={u.value}{owner_mark}")


def analyze_cluster_detection(sim: Simulation):
    """
    Key insight from user: can we detect wealth concentration from tag patterns?

    Hypothesis: coins from the same source have similar tags.
    If someone accumulates wealth, their UTXOs might cluster in tag space.
    """
    print(f"\n" + "=" * 70)
    print("CLUSTER DETECTION ANALYSIS")
    print("=" * 70)

    # For each agent, compute their tag "footprint"
    agent_data = []
    for agent in range(sim.n_agents):
        utxo_ids = sim.agent_utxos[agent]
        if not utxo_ids:
            continue

        tags = [sim.utxos[uid].tag for uid in utxo_ids]
        values = [sim.utxos[uid].value for uid in utxo_ids]
        total_wealth = sum(values)

        # Compute tag "spread" - how dispersed are this agent's tags?
        tag_spread = np.std(tags) if len(tags) > 1 else 0

        # Value-weighted tag center
        weighted_center = sum(t * v for t, v in zip(tags, values)) / total_wealth if total_wealth > 0 else 0

        agent_data.append({
            'agent': agent,
            'wealth': total_wealth,
            'n_utxos': len(tags),
            'tag_spread': tag_spread,
            'tag_center': weighted_center,
            'tags': tags
        })

    # Sort by wealth
    agent_data.sort(key=lambda x: x['wealth'], reverse=True)

    # Question: do wealthy agents have recognizable tag patterns?
    print("\nDoes wealth correlate with tag spread?")

    top_10 = agent_data[:10]
    bottom_50 = agent_data[50:]

    avg_spread_top = np.mean([d['tag_spread'] for d in top_10])
    avg_spread_bottom = np.mean([d['tag_spread'] for d in bottom_50])

    print(f"  Top 10 by wealth: avg tag spread = {avg_spread_top:.4f}")
    print(f"  Bottom 50 by wealth: avg tag spread = {avg_spread_bottom:.4f}")

    # Can we IDENTIFY wealthy agents by looking at tag clustering on chain?
    print("\n" + "-" * 50)
    print("UTXO CLUSTERING ON CHAIN (what observers see)")
    print("-" * 50)

    # All UTXOs sorted by tag
    all_utxos = list(sim.utxos.values())
    all_utxos.sort(key=lambda u: u.tag)

    # Find clusters: groups of UTXOs with similar tags
    # If same owner has many UTXOs with similar tags, that's a cluster
    print("\nLooking for tag-based clusters...")

    # Cluster by tag proximity (within 0.01 of each other)
    clusters = []
    current_cluster = [all_utxos[0]]

    for i in range(1, len(all_utxos)):
        if all_utxos[i].tag - current_cluster[-1].tag < 0.01:
            current_cluster.append(all_utxos[i])
        else:
            if len(current_cluster) >= 3:
                clusters.append(current_cluster)
            current_cluster = [all_utxos[i]]

    if len(current_cluster) >= 3:
        clusters.append(current_cluster)

    print(f"  Found {len(clusters)} clusters (3+ UTXOs within 0.01 tag distance)")

    # Analyze clusters: do they reveal ownership?
    print("\nCluster analysis (top 5 by size):")
    clusters.sort(key=lambda c: len(c), reverse=True)

    for i, cluster in enumerate(clusters[:5]):
        owners = [u.owner for u in cluster]
        unique_owners = len(set(owners))
        total_value = sum(u.value for u in cluster)
        owner_counts = defaultdict(int)
        for o in owners:
            owner_counts[o] += 1
        dominant_owner, dom_count = max(owner_counts.items(), key=lambda x: x[1])
        dom_pct = dom_count / len(cluster) * 100

        print(f"  Cluster {i+1}: {len(cluster)} UTXOs, {unique_owners} owners")
        print(f"    Total value: {total_value:,}")
        print(f"    Dominant owner: agent {dominant_owner} ({dom_pct:.1f}% of cluster)")
        print(f"    Tag range: {cluster[0].tag:.4f} - {cluster[-1].tag:.4f}")

    # THE KEY QUESTION: can we compute a "cluster wealth" metric from public data?
    print("\n" + "=" * 70)
    print("KEY QUESTION: PROGRESSIVE FEE FROM PUBLIC TAGS?")
    print("=" * 70)

    print("""
The challenge: compute fee rate from public tag data without knowing owners.

Approach 1: Tag Density
  - UTXOs with many neighbors (similar tags) pay higher fees
  - Rationale: accumulation creates clustering

Approach 2: Tag Distance from Mean
  - Tags far from average are "fresher" (less circulated)
  - Fresher coins might correlate with wealth

Approach 3: Inferred Cluster Wealth
  - Find clusters by tag proximity
  - Sum all values in cluster
  - Fee based on cluster total

Let's test Approach 3...
""")

    # For each UTXO, compute "cluster wealth" = sum of values of nearby UTXOs
    print("Computing cluster-based fee rates...")

    # Build tag index for fast lookup
    tag_sorted = sorted(all_utxos, key=lambda u: u.tag)
    tag_values = np.array([u.tag for u in tag_sorted])

    results = []
    for utxo in sim.rng.sample(all_utxos, min(20, len(all_utxos))):
        # Find UTXOs within 0.02 tag distance
        idx = np.searchsorted(tag_values, utxo.tag)
        left = max(0, idx - 50)
        right = min(len(tag_sorted), idx + 50)

        nearby = [u for u in tag_sorted[left:right] if abs(u.tag - utxo.tag) < 0.02]
        cluster_value = sum(u.value for u in nearby)

        # True owner's wealth
        true_wealth = sim.get_agent_wealth(utxo.owner)

        results.append({
            'utxo_id': utxo.id,
            'owner': utxo.owner,
            'value': utxo.value,
            'true_wealth': true_wealth,
            'cluster_value': cluster_value,
            'n_nearby': len(nearby)
        })

    print(f"\nSample of 20 UTXOs:")
    print(f"{'UTXO':<8} {'Owner':<6} {'Value':>10} {'True Wealth':>14} {'Cluster Val':>14} {'Near':>6}")
    for r in sorted(results, key=lambda x: x['true_wealth'], reverse=True)[:20]:
        print(f"{r['utxo_id']:<8} {r['owner']:<6} {r['value']:>10,} {r['true_wealth']:>14,} {r['cluster_value']:>14,} {r['n_nearby']:>6}")

    # Correlation?
    true_wealths = [r['true_wealth'] for r in results]
    cluster_values = [r['cluster_value'] for r in results]
    corr = np.corrcoef(true_wealths, cluster_values)[0, 1]
    print(f"\nCorrelation between true wealth and cluster value: {corr:.3f}")

    if corr > 0.3:
        print("  → PROMISING: cluster value correlates with true wealth")
    elif corr > 0:
        print("  → WEAK: some signal but not strong")
    else:
        print("  → FAILED: no correlation")


def analyze_fundamental_problem():
    """
    The core tension in public tag design for progressive fees.
    """
    print("\n" + "=" * 70)
    print("FUNDAMENTAL ANALYSIS: WHY PUBLIC TAGS MAY NOT WORK")
    print("=" * 70)

    print("""
WHAT WE OBSERVED:
1. Tags converge to average as economy mixes (std: 0.29 → 0.025)
2. Most UTXOs end up in one big cluster
3. Wealthy HODLers stay OUTSIDE the main cluster (tags don't drift)
4. Cluster value doesn't correlate well with true owner wealth

THE PARADOX:
- Tags track COIN history, not OWNER wealth
- Rich person accumulates from diverse sources → tags average out
- Poor person receives from one source → tag inherited
- Both end up with similar tags after mixing!

WHAT TAGS ACTUALLY ENCODE:
- "How much has this coin circulated?"
- "What mix of sources contributed to this coin?"
- NOT: "How much does the current owner have?"

THE ROOT PROBLEM:
Progressive fees require knowing: "How wealthy is the SENDER?"
Tags tell us: "Where did these COINS come from?"
These are fundamentally different questions!

CONSIDER THIS SCENARIO:
- Whale W has 1M coins (tag = 0.7, unique)
- W pays Merchant M 100 coins
- M now has UTXO with tag ~0.7
- M (not wealthy) now pays "wealthy" fees because of the tag!
- This is backwards - we're taxing the RECIPIENT's coins' origin,
  not the SENDER's true wealth.

POSSIBLE SOLUTIONS:

1. TAG RESET ON TRANSFER
   - Recipient's new UTXO gets fresh random tag
   - Breaks link to source entirely
   - But then tags encode nothing useful!

2. TAG = OWNER ID (defeats privacy)
   - Each owner has a tag
   - All their UTXOs share it
   - But this reveals ownership!

3. TAG ENCODES VALUE PERCENTILE
   - Large UTXOs get high tags
   - Problem: rich can split into small UTXOs

4. MULTI-HOP CLUSTER ANALYSIS
   - Track transaction graphs
   - Identify clusters by spending patterns
   - But this is what chain analysis does anyway!

5. ZERO-KNOWLEDGE PROOFS
   - Prove "my total balance is in range [X, Y]"
   - Don't reveal exact amount or UTXOs
   - Fee based on proven range
   - Most promising but complex

CONCLUSION:
Public tags that drift toward average cannot encode wealth.
They encode circulation history, which is orthogonal to wealth.
For progressive fees, we likely need either:
- Private commitment with ZK proof of balance range
- Accepting some privacy loss (cluster analysis)
- A fundamentally different approach

""")

    # Let's verify with a concrete example
    print("-" * 70)
    print("CONCRETE EXAMPLE")
    print("-" * 70)

    sim = Simulation(n_agents=10, seed=123)

    # Create specific wealth distribution
    # Agent 0: Whale (1M)
    # Agents 1-9: Poor (100 each)
    sim.mint(0, 1_000_000)
    for i in range(1, 10):
        sim.mint(i, 100)

    print("\nInitial state:")
    print(f"  Whale (agent 0): {sim.get_agent_wealth(0):,} coins, tag = {sim.get_agent_tags(0)[0][0]:.4f}")
    print(f"  Poor (agent 1): {sim.get_agent_wealth(1):,} coins, tag = {sim.get_agent_tags(1)[0][0]:.4f}")

    # Whale pays agent 1
    whale_tag = sim.get_agent_tags(0)[0][0]
    sim.transfer(0, 1, 10_000)

    print(f"\nAfter whale pays agent 1 10,000 coins:")
    print(f"  Agent 1 wealth: {sim.get_agent_wealth(1):,}")
    tags_1 = sim.get_agent_tags(1)
    print(f"  Agent 1 UTXOs:")
    for tag, val in tags_1:
        source = "from whale" if abs(tag - whale_tag) < 0.01 else "original"
        print(f"    value={val}, tag={tag:.4f} ({source})")

    print(f"""
THE PROBLEM:
Agent 1 (poor, 10,100 total) now has a UTXO with the whale's tag!
If fees are based on tag, agent 1 pays "whale" fees on that UTXO.
This punishes receiving from the wealthy, not being wealthy.
""")


if __name__ == "__main__":
    sim, tag_std, unique_tags = run_simulation(n_agents=100, rounds=1000, seed=42)
    analyze_ring_selection(sim)
    analyze_cluster_detection(sim)
    analyze_fundamental_problem()
