# Asymmetric UTXO Fees: Simulation Specification

## Status

**Draft** - Simulation framework to be implemented

## Purpose

This document specifies the simulation required to validate the combined progressive mechanism proposed in [Asymmetric UTXO Fees](asymmetric-utxo-fees.md). The mechanism has four components:

1. **Asymmetric fees**: Splitting expensive, consolidation cheap
2. **Value-weighted lottery with floor**: Tickets = max(1, value/threshold)
3. **Lottery eligibility decay**: Inactive UTXOs lose lottery weight over time
4. **Minimum UTXO size**: Caps maximum splitting advantage

The simulation must answer:

1. At what split penalty is the parking attack unprofitable?
2. Does meaningful Gini reduction occur?
3. Is privacy (anonymity set size) preserved?
4. Does normal commerce remain practical?
5. How do the four components interact?

## Simulation Architecture

### Core Components

```
┌─────────────────────────────────────────────────────────────┐
│                    SIMULATION ENGINE                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐     │
│  │   Agents    │───▶│ Transaction │───▶│   UTXO Set  │     │
│  │  (Actors)   │    │   Engine    │    │   Manager   │     │
│  └─────────────┘    └─────────────┘    └─────────────┘     │
│         │                  │                  │             │
│         ▼                  ▼                  ▼             │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐     │
│  │  Behavior   │    │     Fee     │    │   Lottery   │     │
│  │   Models    │    │ Calculator  │    │   Engine    │     │
│  └─────────────┘    └─────────────┘    └─────────────┘     │
│                            │                  │             │
│                            ▼                  ▼             │
│                     ┌─────────────────────────┐            │
│                     │    Metrics Collector    │            │
│                     └─────────────────────────┘            │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Data Structures

```python
from dataclasses import dataclass
from enum import Enum
from typing import List, Dict, Optional
import numpy as np

@dataclass
class UTXO:
    id: str
    value: int                    # In picocredits
    source_wealth: int            # Minting proximity tag
    owner_id: str
    creation_round: int
    last_activity_round: int      # For eligibility decay
    lottery_wins_this_epoch: int = 0

@dataclass
class Agent:
    id: str
    agent_type: AgentType
    wealth: int                   # Total value of owned UTXOs
    utxos: List[str]             # UTXO IDs
    rationality: float           # 0-1: how optimally they behave
    behavior_params: Dict

class AgentType(Enum):
    WEALTHY_HOLDER = "wealthy"    # Large holder, fee-minimizing
    SMALL_HOLDER = "small"        # Normal user
    MERCHANT = "merchant"         # Receives many, consolidates
    SPLIT_ATTACKER = "split_attacker"    # Tries to split for lottery tickets
    PARKING_ATTACKER = "parking_attacker" # Splits, parks, collects lottery
    MINER = "miner"               # Creates new coins

@dataclass
class Transaction:
    inputs: List[UTXO]
    outputs: List[UTXO]
    fee: int
    tx_type: TransactionType

class TransactionType(Enum):
    SPLIT = "split"               # 1 input → many outputs
    CONSOLIDATE = "consolidate"   # many inputs → 1 output
    TRANSFER = "transfer"         # 1 → 1
    COMMERCE = "commerce"         # N → 2 (payment + change)
    MINT = "mint"                 # 0 → 1 (block reward)
```

## Agent Behavior Models

### Wealthy Holder

```python
class WealthyHolderBehavior:
    """
    Fee-minimizing behavior for large holders.
    Will consolidate UTXOs when economically rational.
    """

    def decide_action(self, agent: Agent, utxo_set: UtxoSet, params: SimParams) -> Action:
        # Calculate cost of current UTXO structure
        current_utxos = [utxo_set.get(uid) for uid in agent.utxos]

        if len(current_utxos) > params.consolidation_threshold:
            # Consider consolidation
            consolidation_cost = self.estimate_consolidation_cost(current_utxos, params)
            holding_cost = self.estimate_holding_cost(current_utxos, params)

            if consolidation_cost < holding_cost:
                return ConsolidateAction(current_utxos)

        # Otherwise, random commerce activity
        if random.random() < params.commerce_probability:
            return CommerceAction(
                amount=random.choice(current_utxos).value * random.uniform(0.1, 0.5),
                recipient=random.choice(other_agents)
            )

        return HoldAction()

    def estimate_consolidation_cost(self, utxos: List[UTXO], params: SimParams) -> int:
        # Many inputs → 1 output: discounted
        base_fee = params.base_fee * estimate_tx_size(len(utxos), 1)
        return int(base_fee * params.consolidation_discount)

    def estimate_holding_cost(self, utxos: List[UTXO], params: SimParams) -> int:
        # Cost of future transactions with fragmented UTXOs
        expected_txs = params.expected_tx_per_round * params.planning_horizon
        return expected_txs * params.base_fee * len(utxos)
```

### Parking Attacker (Primary Threat)

The parking attack is the main threat the mechanism must counter. The attacker:
1. Splits into many UTXOs (pays one-time penalty)
2. Parks them to collect lottery winnings
3. Consolidates when they need to move the funds

```python
class ParkingAttackerBehavior:
    """
    Attempts to maximize lottery winnings by:
    1. Splitting into many small UTXOs
    2. Parking them (not transacting)
    3. Collecting lottery over time
    4. Consolidating when needed

    The mechanism counters this through eligibility decay.
    """

    def __init__(self, split_target: int = 100):
        self.split_target = split_target  # Desired number of UTXOs
        self.has_split = False
        self.parking_rounds = 0

    def decide_action(self, agent: Agent, utxo_set: UtxoSet,
                     current_round: int, params: SimParams) -> Action:
        current_utxos = [utxo_set.get(uid) for uid in agent.utxos]

        if not self.has_split and len(current_utxos) < self.split_target:
            # Phase 1: Split to target UTXO count
            largest = max(current_utxos, key=lambda u: u.value)
            split_count = min(
                self.split_target - len(current_utxos) + 1,
                largest.value // params.min_utxo_value  # Can't split below minimum
            )

            if split_count > 1:
                self.has_split = True
                return SplitAction(
                    source_utxo=largest,
                    num_outputs=split_count
                )

        # Phase 2: Park and collect lottery
        self.parking_rounds += 1

        # Check if eligibility has decayed too much - may need to refresh
        avg_eligibility = self.calculate_avg_eligibility(current_utxos, current_round, params)

        if avg_eligibility < params.refresh_threshold:
            # Eligibility too low, refresh with self-transfer
            # This costs fees but restores eligibility
            return RefreshAction(current_utxos[:10])  # Refresh subset

        return HoldAction()  # Stay parked, collect lottery

    def calculate_avg_eligibility(self, utxos: List[UTXO],
                                  current_round: int, params: SimParams) -> float:
        total = 0.0
        for utxo in utxos:
            age_rounds = current_round - utxo.last_activity_round
            age_days = age_rounds / params.rounds_per_day
            decay = (1.0 - params.decay_rate_per_day) ** age_days
            eligibility = max(params.min_eligibility, decay)
            total += eligibility
        return total / len(utxos) if utxos else 0.0

    def calculate_expected_roi(self, params: SimParams) -> float:
        """
        Calculate expected ROI of the parking attack.

        ROI = (lottery_winnings_present_value - split_cost - refresh_costs) / initial_investment

        If eligibility decay is properly calibrated, this should be < 1.0
        """
        # One-time split cost
        split_cost = params.split_penalty_multiplier * (self.split_target - 1) * params.base_fee

        # Daily lottery winnings (decaying)
        daily_pool = params.avg_fee * params.lottery_fraction * params.txs_per_day
        total_tickets = self.estimate_total_tickets(params)
        our_tickets = self.split_target  # Each UTXO gets floor of 1 ticket

        # Integrate decaying winnings over time
        total_winnings = 0.0
        eligibility = 1.0
        for day in range(params.planning_horizon_days):
            daily_share = (our_tickets * eligibility) / (total_tickets + our_tickets * eligibility)
            total_winnings += daily_share * daily_pool
            eligibility = max(params.min_eligibility, eligibility * (1 - params.decay_rate_per_day))

        return total_winnings / split_cost if split_cost > 0 else float('inf')
```

### Merchant

```python
class MerchantBehavior:
    """
    Receives many small payments, periodically consolidates.
    """

    def decide_action(self, agent: Agent, utxo_set: UtxoSet, params: SimParams) -> Action:
        current_utxos = [utxo_set.get(uid) for uid in agent.utxos]

        # Check if consolidation makes sense
        if len(current_utxos) > params.merchant_consolidation_threshold:
            small_utxos = [u for u in current_utxos if u.value < params.consolidation_value_threshold]
            if len(small_utxos) > params.min_consolidation_batch:
                return ConsolidateAction(small_utxos)

        # Merchants mostly receive, occasionally pay suppliers
        if random.random() < params.merchant_payment_probability:
            return CommerceAction(
                amount=self.select_payment_amount(current_utxos),
                recipient=random.choice(other_agents)
            )

        return ReceiveAction()  # Passive - waiting for payments
```

## Fee Calculator

```python
class FeeCalculator:
    def __init__(self, params: SimParams):
        self.params = params

    def calculate(self, tx: Transaction) -> int:
        base_fee = self.params.base_fee * self.estimate_size(tx)

        # Minting proximity factor (existing mechanism)
        cluster_factor = self.calculate_cluster_factor(tx)

        # Structure factor (new mechanism)
        structure_factor = self.calculate_structure_factor(tx)

        return int(base_fee * cluster_factor * structure_factor)

    def calculate_structure_factor(self, tx: Transaction) -> float:
        input_count = len(tx.inputs)
        output_count = len(tx.outputs)

        if output_count > input_count + self.params.allowed_extra_outputs:
            # Splitting: penalize
            extra = output_count - input_count - self.params.allowed_extra_outputs
            return 1.0 + extra * self.params.split_penalty_multiplier

        elif output_count < input_count:
            # Consolidating: discount
            return self.params.consolidation_discount

        else:
            # Normal: no adjustment
            return 1.0

    def calculate_cluster_factor(self, tx: Transaction) -> float:
        # Value-weighted average of input source_wealth
        total_value = sum(u.value for u in tx.inputs)
        weighted_source = sum(u.value * u.source_wealth for u in tx.inputs)
        avg_source_wealth = weighted_source / total_value if total_value > 0 else 0

        # Map to fee multiplier using existing curve
        return self.source_wealth_to_factor(avg_source_wealth)
```

## Lottery Engine

The lottery uses **value-weighted selection with floor** combined with **eligibility decay**.

### Core Formula

```
tickets(utxo) = max(1, utxo.value / TICKET_THRESHOLD)
eligibility(utxo) = max(MIN_ELIGIBILITY, (1 - DECAY_RATE)^age_days)
effective_tickets(utxo) = tickets(utxo) × eligibility(utxo)
```

### Why This Works

| Holder Type | Value | Tickets | Tickets per BTH |
|-------------|-------|---------|-----------------|
| Small holder (100 BTH) | Below threshold | 1 | 0.01 |
| Normal holder (1000 BTH) | At threshold | 1 | 0.001 |
| Wealthy holder (1M BTH) | 1000× threshold | 1000 | 0.001 |

Small holders get **10x more tickets per BTH** than wealthy holders.

Splitting doesn't help wealthy holders because:
- Split 1M BTH into 1000 × 1K BTH → 1000 tickets (same as consolidated)
- Split further below threshold → capped by minimum UTXO size

```python
class LotteryEngine:
    def __init__(self, params: SimParams):
        self.params = params

    def distribute(self, fee: int, utxo_set: UtxoSet,
                   current_round: int) -> List[Tuple[str, int]]:
        pool = int(fee * self.params.lottery_fraction)
        burned = fee - pool

        winners = []
        per_winner = pool // self.params.winners_per_tx

        for _ in range(self.params.winners_per_tx):
            winner_id = self.select_winner(utxo_set, current_round)
            winners.append((winner_id, per_winner))
            utxo = utxo_set.get(winner_id)
            utxo.lottery_wins_this_epoch += 1
            # Note: Winning lottery doesn't refresh eligibility
            # Only actual transactions do

        return winners

    def select_winner(self, utxo_set: UtxoSet, current_round: int) -> str:
        """
        Select winner using effective tickets (value-weighted with floor × eligibility)
        """
        weights = []
        ids = []

        for utxo in utxo_set.all():
            effective = self.effective_tickets(utxo, current_round)
            ids.append(utxo.id)
            weights.append(effective)

        total = sum(weights)
        probs = [w / total for w in weights]
        return np.random.choice(ids, p=probs)

    def effective_tickets(self, utxo: UTXO, current_round: int) -> float:
        """
        Calculate effective lottery tickets for a UTXO.

        effective_tickets = base_tickets × eligibility
        """
        base_tickets = self.base_tickets(utxo)
        eligibility = self.eligibility(utxo, current_round)
        return base_tickets * eligibility

    def base_tickets(self, utxo: UTXO) -> int:
        """
        Value-weighted with floor.

        tickets = max(1, value / threshold)

        Small UTXOs (below threshold) get 1 ticket.
        Large UTXOs get tickets proportional to value.
        """
        return max(1, utxo.value // self.params.ticket_threshold)

    def eligibility(self, utxo: UTXO, current_round: int) -> float:
        """
        Eligibility decays for inactive UTXOs.

        This counters the parking attack: parked UTXOs
        gradually lose lottery weight.
        """
        age_rounds = current_round - utxo.last_activity_round
        age_days = age_rounds / self.params.rounds_per_day
        decay = (1.0 - self.params.decay_rate_per_day) ** age_days
        return max(self.params.min_eligibility, decay)
```

### Eligibility Decay Examples

```
DECAY_RATE = 0.03 (3% per day)
MIN_ELIGIBILITY = 0.10 (10% floor)

Day 0:   100% eligibility
Day 10:  74% eligibility ((0.97)^10)
Day 30:  40% eligibility ((0.97)^30)
Day 77:  10% eligibility (hits floor)
Day 100: 10% eligibility (floor)
```

The floor prevents complete exclusion of long-term holders, but
significantly reduces the parking attack's effectiveness.

## Metrics Collection

```python
@dataclass
class SimulationMetrics:
    # Per-round metrics
    round_number: int

    # Wealth distribution
    gini_coefficient: float
    wealth_share_top_1_percent: float
    wealth_share_bottom_50_percent: float
    wealth_transfer_this_round: float

    # UTXO distribution
    total_utxo_count: int
    utxo_count_by_value_bin: Dict[str, int]
    avg_utxo_value: float
    median_utxo_value: float

    # Privacy metrics
    anonymity_set_size_by_bin: Dict[str, int]
    min_anonymity_set: int

    # Economic metrics
    total_fees_collected: int
    lottery_pool_distributed: int
    total_burned: int

    # Eligibility metrics (new)
    avg_eligibility: float                    # Average across all UTXOs
    parked_utxo_avg_eligibility: float        # Average for parked UTXOs
    eligibility_distribution: Dict[str, int]  # Binned distribution

    # Parking attack metrics (new)
    parking_attacker_spend_on_splits: int
    parking_attacker_spend_on_refresh: int
    parking_attacker_lottery_winnings: int
    parking_attacker_roi: float               # Key metric: < 1.0 = attack defeated

    # Splitting attack metrics (new)
    split_attacker_utxo_count: int
    split_attacker_effective_tickets: float
    split_advantage_ratio: float              # Actual vs unsplit tickets

    # Transaction breakdown
    tx_count_by_type: Dict[str, int]


class MetricsCollector:
    def __init__(self):
        self.history: List[SimulationMetrics] = []

    def collect(self, round_num: int, agents: List[Agent],
                utxo_set: UtxoSet, tx_log: List[Transaction]) -> SimulationMetrics:

        # Calculate Gini coefficient
        wealth_values = [a.wealth for a in agents]
        gini = self.calculate_gini(wealth_values)

        # Wealth distribution
        sorted_wealth = sorted(wealth_values)
        total_wealth = sum(sorted_wealth)
        top_1_pct = sum(sorted_wealth[-len(agents)//100:]) / total_wealth
        bottom_50_pct = sum(sorted_wealth[:len(agents)//2]) / total_wealth

        # UTXO distribution
        utxo_values = [u.value for u in utxo_set.all()]
        bins = self.bin_utxos(utxo_values)

        # Privacy metrics
        anonymity_sets = {bin_name: count for bin_name, count in bins.items()}
        min_anon = min(anonymity_sets.values()) if anonymity_sets else 0

        # Attack metrics
        attackers = [a for a in agents if a.agent_type == AgentType.ATTACKER]
        attacker_spend = sum(self.get_split_spend(a, tx_log) for a in attackers)
        attacker_wins = sum(self.get_lottery_wins(a, tx_log) for a in attackers)
        attacker_roi = attacker_wins / attacker_spend if attacker_spend > 0 else 0

        metrics = SimulationMetrics(
            round_number=round_num,
            gini_coefficient=gini,
            wealth_share_top_1_percent=top_1_pct,
            wealth_share_bottom_50_percent=bottom_50_pct,
            total_utxo_count=utxo_set.count(),
            utxo_count_by_value_bin=bins,
            avg_utxo_value=np.mean(utxo_values),
            median_utxo_value=np.median(utxo_values),
            anonymity_set_size_by_bin=anonymity_sets,
            min_anonymity_set=min_anon,
            attacker_spend_on_splits=attacker_spend,
            attacker_lottery_winnings=attacker_wins,
            attacker_roi=attacker_roi,
            # ... other metrics
        )

        self.history.append(metrics)
        return metrics

    def calculate_gini(self, values: List[float]) -> float:
        sorted_values = sorted(values)
        n = len(sorted_values)
        cumsum = np.cumsum(sorted_values)
        return (2 * sum((i + 1) * v for i, v in enumerate(sorted_values)) /
                (n * sum(sorted_values)) - (n + 1) / n)
```

## Parameter Space

### Primary Parameters to Sweep

```python
PARAMETER_SPACE = {
    # Component 1: Asymmetric fee structure
    "split_penalty_multiplier": [0.1, 0.5, 1.0, 2.0, 5.0],  # Per extra output
    "consolidation_discount": [0.1, 0.3, 0.5, 0.7],          # Multiplier
    "allowed_extra_outputs": [1, 2],                          # Before penalty

    # Component 2: Value-weighted lottery with floor
    "ticket_threshold": [100, 500, 1000, 5000],              # BTH per ticket
    "lottery_fraction": [0.6, 0.8, 0.9],                     # Of fees
    "winners_per_tx": [1, 4, 10],

    # Component 3: Eligibility decay
    "decay_rate_per_day": [0.01, 0.03, 0.05, 0.10],          # Daily decay rate
    "min_eligibility": [0.0, 0.05, 0.1, 0.2],                # Eligibility floor

    # Component 4: Minimum UTXO (affects splitting cap)
    "min_utxo_value": [10, 50, 100, 500],                    # BTH

    # Population composition
    "wealthy_fraction": [0.01, 0.05, 0.1],                   # Top wealth holders
    "parking_attacker_fraction": [0.0, 0.01, 0.05],          # Parking gamers
    "split_attacker_fraction": [0.0, 0.01, 0.05],            # Split gamers
    "merchant_fraction": [0.05, 0.1, 0.2],                   # Commerce nodes
}
```

### Fixed Parameters

```python
FIXED_PARAMS = {
    "num_agents": 1000,
    "initial_supply": 100_000_000_000_000,               # 100M BTH in picocredits
    "initial_gini": 0.85,
    "simulation_rounds": 1000,
    "rounds_per_day": 4320,                              # ~20 sec blocks
    "txs_per_round": 500,
    "base_fee": 1_000_000_000,                           # 1 BTH in picocredits
    "minting_reward_per_round": 50_000_000_000_000,      # 50 BTH
}
```

### Key Parameter Interactions

The four components interact in important ways:

```
Value-weighted lottery ←→ Minimum UTXO
├── ticket_threshold sets where floor kicks in
├── min_utxo_value caps maximum splitting advantage
└── Optimal: min_utxo ≤ ticket_threshold / 10

Eligibility decay ←→ Split penalty
├── decay_rate determines parking attack window
├── split_penalty_multiplier determines one-time cost
└── Break-even: If decay too slow, penalty must be higher

All components together:
├── High decay + moderate penalty → Good
├── Low decay + high penalty → Good (different tradeoff)
├── Low decay + low penalty → VULNERABLE to parking
└── High decay + high penalty → Over-punishment (hurts commerce)
```

## Simulation Scenarios

### Scenario 1: Baseline (Uniform Lottery, No Protections)

```python
baseline_params = SimParams(
    # No asymmetric fees
    split_penalty_multiplier=0,
    consolidation_discount=1.0,

    # Uniform lottery (original vulnerable design)
    ticket_threshold=float('inf'),    # Everything gets 1 ticket
    min_eligibility=1.0,              # No decay

    lottery_fraction=0.8,
    parking_attacker_fraction=0.05,
)

# Expected outcome:
# - Parking attackers achieve ~1000x lottery advantage (unlimited splitting)
# - No Gini reduction
# - This establishes the vulnerability baseline
```

### Scenario 2: Value-Weighted Lottery Only

```python
value_weighted_params = SimParams(
    # No asymmetric fees
    split_penalty_multiplier=0,
    consolidation_discount=1.0,

    # Value-weighted with floor
    ticket_threshold=1000,            # 1000 BTH per ticket
    min_utxo_value=100,               # 100 BTH minimum

    # No decay
    min_eligibility=1.0,

    lottery_fraction=0.8,
    parking_attacker_fraction=0.05,
)

# Expected outcome:
# - Parking attackers achieve ~10x lottery advantage (capped by min UTXO)
# - Some Gini reduction (progressive per-BTH tickets)
# - Shows value of min UTXO + ticket threshold
```

### Scenario 3: Full Combined Mechanism

```python
combined_params = SimParams(
    # Asymmetric fees
    split_penalty_multiplier=1.0,
    consolidation_discount=0.3,
    allowed_extra_outputs=1,

    # Value-weighted lottery
    ticket_threshold=1000,
    min_utxo_value=100,

    # Eligibility decay
    decay_rate_per_day=0.03,
    min_eligibility=0.10,

    lottery_fraction=0.8,
    parking_attacker_fraction=0.05,
)

# Expected outcome:
# - Parking attack ROI < 1.0 (unprofitable)
# - Meaningful Gini reduction
# - Good privacy (reasonable UTXO distribution)
```

### Scenario 4: Parking Attack Deep Dive

```python
parking_attack_params = SimParams(
    # Full protections
    split_penalty_multiplier=1.0,
    decay_rate_per_day=0.03,
    min_eligibility=0.10,
    ticket_threshold=1000,
    min_utxo_value=100,

    # High attacker fraction
    parking_attacker_fraction=0.10,

    # Long simulation to see decay effects
    simulation_rounds=10000,  # ~2+ days simulated
)

# Track over time:
# - Attacker effective tickets (should decay)
# - Attacker ROI (should go negative)
# - Whether attackers start refreshing (costs fees)
```

### Scenario 5: Decay Rate Sensitivity

```python
# Test different decay rates
for decay_rate in [0.01, 0.03, 0.05, 0.10]:
    params = SimParams(
        decay_rate_per_day=decay_rate,
        min_eligibility=0.10,
        split_penalty_multiplier=1.0,
        # ... other params
    )

# Expected trade-off:
# - Low decay (0.01): Longer parking window, higher penalty needed
# - High decay (0.10): Normal users inconvenienced, lower penalty sufficient
```

### Scenario 6: Merchant Viability

```python
merchant_test_params = SimParams(
    merchant_fraction=0.2,
    split_penalty_multiplier=1.0,
    consolidation_discount=0.3,
    decay_rate_per_day=0.03,
    # ... full mechanism
)

# Track:
# - Merchant fee burden (total fees / value transacted)
# - Consolidation frequency
# - Merchant wealth trajectory
# - Do merchants' eligibilities stay high? (active commerce)
```

### Scenario 7: Privacy Impact

```python
privacy_test_params = SimParams(
    # Full mechanism
    split_penalty_multiplier=1.0,
    consolidation_discount=0.3,
    ticket_threshold=1000,
    min_utxo_value=100,
    # ...
)

# Track:
# - Total UTXO count over time
# - UTXO value distribution (histogram)
# - Minimum anonymity set size per value bin
# - Do wealthy consolidate too much?
```

## Expected Outcomes

### Success Criteria

```python
def evaluate_success(final_metrics: SimulationMetrics,
                     baseline_metrics: SimulationMetrics) -> Dict[str, bool]:
    return {
        # Gini reduction: At least 5% improvement over baseline
        "gini_reduction": (
            final_metrics.gini_coefficient <
            baseline_metrics.gini_coefficient - 0.05
        ),

        # Parking attack defeated: Attacker ROI < 1
        "parking_attack_defeated": final_metrics.parking_attacker_roi < 1.0,

        # Splitting attack defeated: Splitting advantage < threshold
        "splitting_attack_defeated": final_metrics.split_advantage_ratio < 2.0,

        # Privacy preserved: Min anonymity set > ring size
        "privacy_preserved": final_metrics.min_anonymity_set > 20,

        # Commerce viable: Merchants don't go bankrupt
        "commerce_viable": all(
            m.wealth > initial_wealth * 0.5
            for m in merchants
        ),

        # Stability: UTXO count doesn't collapse
        "stable_utxo_count": (
            final_metrics.total_utxo_count >
            initial_utxo_count * 0.3
        ),

        # Eligibility decay working: Parked UTXOs have low eligibility
        "decay_effective": (
            final_metrics.parked_utxo_avg_eligibility < 0.3
        ),
    }
```

### Expected Trade-off Curves

```
Decay Rate vs Parking Attack:
    Low decay (0.01)  → Parking profitable longer, need higher penalties
    Medium decay (0.03) → Good balance
    High decay (0.10) → Inconveniences normal users

Ticket Threshold vs Progressivity:
    Low threshold (100 BTH)  → More progressive, higher splitting advantage
    High threshold (5000 BTH) → Less progressive, lower splitting advantage

Minimum UTXO vs Splitting Cap:
    Low min (10 BTH)   → 100x max splitting advantage
    High min (500 BTH) → 2x max splitting advantage (but limits commerce)

Combined Parameter Space:
    ┌─────────────────────────────────────────────────────┐
    │              Decay Rate                             │
    │         Low        Medium         High              │
    │  ┌─────────────────────────────────────────────┐   │
    │  │ Penalty │ Penalty │ Penalty │               │   │
    │P │  HIGH   │  MED    │  LOW    │ Works         │   │
    │e │  ───────┼─────────┼─────────│               │   │
    │n │  HIGH   │  MED    │ OVER-   │               │   │
    │a │         │ (sweet  │  KILL   │               │   │
    │l │         │  spot)  │         │               │   │
    │t │  ───────┼─────────┼─────────│               │   │
    │y │  VULN   │  VULN   │  BAD    │ Doesn't work  │   │
    │  │ (low)   │ (low)   │  UX     │               │   │
    │  └─────────────────────────────────────────────┘   │
    └─────────────────────────────────────────────────────┘
```

## Output Format

### Summary Report

```python
def generate_report(results: Dict[str, List[SimulationMetrics]]) -> str:
    report = """
    # Combined Progressive Mechanism Simulation Results

    ## Parameter Sweep Results

    | Penalty | Decay | Threshold | Parking ROI | Split Adv | Gini Δ | Min Anon |
    |---------|-------|-----------|-------------|-----------|--------|----------|
    """

    for params, metrics in results.items():
        final = metrics[-1]
        baseline_gini = 0.85  # Assumed initial
        gini_delta = baseline_gini - final.gini_coefficient

        report += f"| {params.split_penalty_multiplier:.1f} "
        report += f"| {params.decay_rate_per_day:.2f} "
        report += f"| {params.ticket_threshold} "
        report += f"| {final.parking_attacker_roi:.2f} "
        report += f"| {final.split_advantage_ratio:.1f}x "
        report += f"| {gini_delta:.3f} "
        report += f"| {final.min_anonymity_set} |\n"

    report += """
    ## Recommended Configuration

    Based on the simulation results, the recommended parameters are:

    ### Component 1: Asymmetric Fees
    - Split penalty multiplier: {recommended_penalty}
    - Consolidation discount: {recommended_discount}
    - Allowed extra outputs: 1

    ### Component 2: Value-Weighted Lottery
    - Ticket threshold: {recommended_threshold} BTH
    - Minimum UTXO: {recommended_min_utxo} BTH

    ### Component 3: Eligibility Decay
    - Decay rate: {recommended_decay} per day
    - Minimum eligibility: {recommended_min_elig}

    ## Attack Resistance Summary

    | Attack | Metric | Target | Achieved |
    |--------|--------|--------|----------|
    | Parking | ROI | < 1.0 | {parking_roi} |
    | Splitting | Advantage | < 2.0x | {split_adv}x |

    ## Trade-offs

    This configuration achieves:
    - Gini reduction: {gini_delta}
    - Privacy: {min_anon} minimum anonymity set
    - Merchant viability: {merchant_viable}

    """

    return report
```

### Visualization Requirements

1. **Gini coefficient over time** (line plot per configuration)
2. **UTXO count over time** (line plot)
3. **UTXO value distribution** (histogram at checkpoints)
4. **Eligibility distribution over time** (stacked area chart)
5. **Parking attack ROI over time** (shows decay effect)
6. **Parameter heatmaps**:
   - Parking ROI as function of (decay rate × penalty)
   - Gini reduction as function of (threshold × min UTXO)
   - Privacy impact as function of (penalty × discount)
7. **Attack strategy comparison** (bar chart: parking vs splitting vs honest)
8. **Merchant burden over time** (line plot comparing to small holders)

## Implementation Notes

### Performance Considerations

```python
# For large simulations, optimize:
# 1. UTXO set operations (use efficient data structures)
# 2. Lottery selection (pre-compute cumulative weights)
# 3. Metrics calculation (batch operations with numpy)

class OptimizedUtxoSet:
    def __init__(self):
        self._utxos: Dict[str, UTXO] = {}
        self._by_owner: Dict[str, Set[str]] = defaultdict(set)
        self._by_value_bin: Dict[str, Set[str]] = defaultdict(set)
        self._total_value: int = 0

        # For fast lottery selection
        self._cumulative_weights: Optional[np.ndarray] = None
        self._weights_dirty: bool = True
```

### Reproducibility

```python
# All simulations should be seeded for reproducibility
def run_simulation(params: SimParams, seed: int) -> List[SimulationMetrics]:
    random.seed(seed)
    np.random.seed(seed)
    # ... simulation code
```

## Next Steps

1. **Implement simulation framework** in Python
   - Core UTXO management with eligibility tracking
   - Agent behavior models (especially ParkingAttacker)
   - Fee calculator with structure factor
   - Lottery engine with value-weighted floor + decay

2. **Run baseline scenarios** to validate model
   - Verify uniform lottery is vulnerable (~1000x advantage)
   - Verify value-weighted with floor caps at ~10x
   - Confirm eligibility decay reduces over time

3. **Execute parameter sweep**
   - Primary: decay_rate × split_penalty grid
   - Secondary: ticket_threshold × min_utxo grid
   - Full sweep for publication

4. **Identify viable configurations**
   - Find "sweet spot" where parking ROI < 1.0
   - Measure Gini reduction at sweet spot
   - Verify privacy is preserved

5. **Document findings** with visualizations
   - All 8 visualization types
   - Recommended parameter table
   - Trade-off analysis

6. **Refine mechanism** based on insights
   - Adjust parameters if needed
   - Consider edge cases found in simulation
   - Update design document with final recommendations

## References

- [Asymmetric UTXO Fees](asymmetric-utxo-fees.md) - Combined mechanism design
- [Lottery Redistribution](lottery-redistribution.md) - Original lottery analysis
- [Minting Proximity Fees](minting-proximity-fees.md) - Complementary mechanism for miner wealth
- [Tokenomics](../concepts/tokenomics.md) - Overall economic model

## Changelog

- 2026-01-09: Initial draft with basic simulation framework
- 2026-01-09: Updated for combined mechanism with four components:
  - Added value-weighted lottery with floor
  - Added eligibility decay mechanism
  - Added parking attacker behavior model
  - Updated metrics for attack detection
  - Revised parameter space and scenarios
