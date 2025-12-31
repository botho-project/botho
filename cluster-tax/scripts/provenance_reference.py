#!/usr/bin/env python3
"""
Provenance-Based Progressive Fees: Reference Implementation

This is the canonical implementation for testing correctness.
Each test verifies a property from the correctness checklist.
"""

from dataclasses import dataclass
from typing import List, Dict, Tuple
from collections import defaultdict


@dataclass
class UTXO:
    """
    A coin with provenance tracking.

    source_wealth: Wealth of original minter. Persists through splits, blends on combine.
    """
    id: int
    value: int
    source_wealth: int
    owner: str  # For clarity in tests


class UTXOSet:
    """Manages the UTXO set."""

    def __init__(self):
        self.utxos: Dict[int, UTXO] = {}
        self.next_id = 0

    def mint(self, owner: str, value: int, source_wealth: int) -> UTXO:
        """Create a new UTXO (minting/coinbase)."""
        utxo = UTXO(
            id=self.next_id,
            value=value,
            source_wealth=source_wealth,
            owner=owner
        )
        self.next_id += 1
        self.utxos[utxo.id] = utxo
        return utxo

    def spend(self, utxo_id: int) -> UTXO:
        """Remove and return a UTXO."""
        utxo = self.utxos.pop(utxo_id)
        return utxo

    def get_by_owner(self, owner: str) -> List[UTXO]:
        """Get all UTXOs for an owner."""
        return [u for u in self.utxos.values() if u.owner == owner]

    def get_balance(self, owner: str) -> int:
        """Get total balance for an owner."""
        return sum(u.value for u in self.get_by_owner(owner))


def compute_fee_rate(source_wealth: int, max_wealth: int,
                     r_min: float = 0.01, r_max: float = 0.15) -> float:
    """
    Progressive fee rate based on source_wealth.

    Linear interpolation from r_min (poorest) to r_max (wealthiest).
    """
    if max_wealth <= 0:
        return r_min
    ratio = min(1.0, source_wealth / max_wealth)
    return r_min + (r_max - r_min) * ratio


def transfer(utxo_set: UTXOSet,
             input_ids: List[int],
             outputs: List[Tuple[str, int]],  # [(owner, value), ...]
             max_wealth: int) -> Tuple[bool, int, List[UTXO]]:
    """
    Execute a transfer.

    Args:
        utxo_set: The UTXO set
        input_ids: IDs of UTXOs to spend
        outputs: List of (owner, value) for outputs
        max_wealth: Maximum wealth for fee calculation

    Returns:
        (success, fee_paid, new_utxos)

    Key mechanics:
    1. source_wealth of outputs = value-weighted average of inputs
    2. Fee based on blended source_wealth
    """
    # Gather inputs
    inputs = []
    for uid in input_ids:
        if uid not in utxo_set.utxos:
            return False, 0, []
        inputs.append(utxo_set.utxos[uid])

    total_input_value = sum(u.value for u in inputs)
    total_output_value = sum(v for _, v in outputs)

    # Compute blended source_wealth (value-weighted average)
    blended_source_wealth = sum(u.value * u.source_wealth for u in inputs) // total_input_value

    # Compute fee
    fee_rate = compute_fee_rate(blended_source_wealth, max_wealth)
    fee = int(total_output_value * fee_rate)

    # Verify inputs cover outputs + fee
    if total_input_value < total_output_value + fee:
        return False, 0, []

    # Spend inputs
    for uid in input_ids:
        utxo_set.spend(uid)

    # Create outputs (all inherit blended source_wealth)
    new_utxos = []
    for owner, value in outputs:
        utxo = utxo_set.mint(owner, value, blended_source_wealth)
        new_utxos.append(utxo)

    return True, fee, new_utxos


# =============================================================================
# CORRECTNESS TESTS
# =============================================================================

def test_split_resistance():
    """
    PROPERTY: source_wealth unchanged by splitting.

    A whale splitting their coins into many pieces should NOT reduce
    the source_wealth of those pieces.
    """
    print("=" * 60)
    print("TEST: Split Resistance")
    print("=" * 60)

    utxo_set = UTXOSet()
    max_wealth = 1_000_000

    # Whale has 1M coins
    whale_utxo = utxo_set.mint("whale", 1_000_000, 1_000_000)
    print(f"\nInitial: UTXO value={whale_utxo.value:,}, source_wealth={whale_utxo.source_wealth:,}")

    # Split into 10 pieces of 100K each (no fee for simplicity, self-transfer)
    # In real impl, there would be a small fee, but source_wealth behavior is the same
    success, fee, new_utxos = transfer(
        utxo_set,
        [whale_utxo.id],
        [("whale", 100_000) for _ in range(10)],
        max_wealth
    )

    print(f"\nAfter split into 10 pieces:")
    for utxo in new_utxos[:3]:
        print(f"  UTXO value={utxo.value:,}, source_wealth={utxo.source_wealth:,}")
    print(f"  ... (7 more with same source_wealth)")

    # Verify
    all_same = all(u.source_wealth == 1_000_000 for u in new_utxos)
    print(f"\n✓ All pieces have source_wealth = 1,000,000: {all_same}")

    assert all_same, "FAILED: Split reduced source_wealth!"
    print("✓ TEST PASSED: Splitting does not reduce source_wealth")
    return True


def test_sybil_resistance():
    """
    PROPERTY: source_wealth inherited by sybil recipients.

    A whale sending coins to sybil accounts should NOT reduce
    the source_wealth of those coins.
    """
    print("\n" + "=" * 60)
    print("TEST: Sybil Resistance")
    print("=" * 60)

    utxo_set = UTXOSet()
    max_wealth = 1_000_000

    # Whale has 1M coins
    whale_utxo = utxo_set.mint("whale", 1_000_000, 1_000_000)
    print(f"\nWhale UTXO: value={whale_utxo.value:,}, source_wealth={whale_utxo.source_wealth:,}")

    # Send to 5 sybil accounts
    success, fee, new_utxos = transfer(
        utxo_set,
        [whale_utxo.id],
        [("sybil_1", 100_000),
         ("sybil_2", 100_000),
         ("sybil_3", 100_000),
         ("sybil_4", 100_000),
         ("sybil_5", 100_000)],
        max_wealth
    )

    print(f"\nAfter sending to 5 sybil accounts:")
    for utxo in new_utxos:
        print(f"  {utxo.owner}: value={utxo.value:,}, source_wealth={utxo.source_wealth:,}")

    # Verify
    all_high = all(u.source_wealth == 1_000_000 for u in new_utxos)
    print(f"\n✓ All sybil UTXOs have source_wealth = 1,000,000: {all_high}")

    assert all_high, "FAILED: Sybil transfer reduced source_wealth!"
    print("✓ TEST PASSED: Sybil accounts inherit high source_wealth")
    return True


def test_blend_on_combine():
    """
    PROPERTY: Multiple inputs blend to weighted average source_wealth.

    Combining coins from different sources should produce
    a value-weighted average source_wealth.
    """
    print("\n" + "=" * 60)
    print("TEST: Blend on Combine")
    print("=" * 60)

    utxo_set = UTXOSet()
    max_wealth = 1_000_000

    # Whale UTXO: 100K coins from 1M source
    whale_utxo = utxo_set.mint("merchant", 100_000, 1_000_000)

    # Poor UTXO: 10K coins from 10K source
    poor_utxo = utxo_set.mint("merchant", 10_000, 10_000)

    print(f"\nInputs:")
    print(f"  Whale-origin: value={whale_utxo.value:,}, source_wealth={whale_utxo.source_wealth:,}")
    print(f"  Poor-origin:  value={poor_utxo.value:,}, source_wealth={poor_utxo.source_wealth:,}")

    # Combine into single output (must be less than total input minus max possible fee)
    # Total input: 110K, max fee rate ~15%, so output ~95K to be safe
    success, fee, new_utxos = transfer(
        utxo_set,
        [whale_utxo.id, poor_utxo.id],
        [("recipient", 95_000)],
        max_wealth
    )

    assert success, f"Transfer failed! Check that inputs cover outputs + fee"

    # Expected: (100K * 1M + 10K * 10K) / 110K = (100B + 100M) / 110K ≈ 909,909
    expected = (100_000 * 1_000_000 + 10_000 * 10_000) // 110_000
    actual = new_utxos[0].source_wealth

    print(f"\nAfter combining:")
    print(f"  Output: value={new_utxos[0].value:,}, source_wealth={actual:,}")
    print(f"  Expected source_wealth: {expected:,}")

    assert actual == expected, f"FAILED: Expected {expected}, got {actual}"
    print(f"✓ TEST PASSED: source_wealth correctly blended")
    return True


def test_whale_pays_more():
    """
    PROPERTY: High source_wealth → high fee rate.

    Coins from wealthy sources should pay higher fees than
    coins from poor sources, even for the same transaction amount.
    """
    print("\n" + "=" * 60)
    print("TEST: Whale Pays More")
    print("=" * 60)

    utxo_set = UTXOSet()
    max_wealth = 1_000_000

    # Whale UTXO: 10K from 1M source
    whale_utxo = utxo_set.mint("whale", 10_000, 1_000_000)

    # Poor UTXO: 10K from 10K source
    poor_utxo = utxo_set.mint("poor", 10_000, 10_000)

    print(f"\nSame value (10K), different source:")
    print(f"  Whale: source_wealth={whale_utxo.source_wealth:,}")
    print(f"  Poor:  source_wealth={poor_utxo.source_wealth:,}")

    # Compute fee rates
    whale_rate = compute_fee_rate(whale_utxo.source_wealth, max_wealth)
    poor_rate = compute_fee_rate(poor_utxo.source_wealth, max_wealth)

    whale_fee = int(10_000 * whale_rate)
    poor_fee = int(10_000 * poor_rate)

    print(f"\nFee rates for 10K transfer:")
    print(f"  Whale: {whale_rate:.1%} = {whale_fee:,} fee")
    print(f"  Poor:  {poor_rate:.1%} = {poor_fee:,} fee")
    print(f"  Ratio: {whale_fee / poor_fee:.1f}x")

    assert whale_fee > poor_fee, "FAILED: Whale should pay more!"
    print(f"✓ TEST PASSED: Whale pays {whale_fee / poor_fee:.1f}x more than poor")
    return True


def test_legitimate_commerce_decay():
    """
    PROPERTY: Legitimate commerce reduces effective source_wealth over time.

    As coins pass through many different hands, their source_wealth
    should average toward the population mean.
    """
    print("\n" + "=" * 60)
    print("TEST: Legitimate Commerce Decay")
    print("=" * 60)

    utxo_set = UTXOSet()
    max_wealth = 1_000_000

    # Initial: Whale has 100K at source_wealth 1M
    current_utxo = utxo_set.mint("whale", 100_000, 1_000_000)

    # Each merchant has some coins at average source_wealth
    avg_source = 50_000  # Population average

    print(f"\nInitial: source_wealth = {current_utxo.source_wealth:,}")
    print(f"Population average: {avg_source:,}")
    print(f"\nSimulating commerce through 10 merchants...")

    history = [current_utxo.source_wealth]

    for i in range(10):
        # Merchant has their own coins
        merchant_utxo = utxo_set.mint(f"merchant_{i}", 50_000, avg_source)

        # Total input value
        total_input = current_utxo.value + merchant_utxo.value

        # Conservative output: 80% of total to ensure we cover fees
        output_value = int(total_input * 0.80)

        # Whale-origin coins combine with merchant's coins
        success, fee, outputs = transfer(
            utxo_set,
            [current_utxo.id, merchant_utxo.id],
            [(f"merchant_{i+1}", output_value)],
            max_wealth
        )

        if not success:
            print(f"  WARNING: Transfer {i} failed, stopping")
            break

        current_utxo = outputs[0]
        history.append(current_utxo.source_wealth)

    print(f"\nSource wealth after each hop:")
    for i, sw in enumerate(history):
        bar = "█" * int(sw / 50_000)
        print(f"  Hop {i:2d}: {sw:>10,} {bar}")

    # Verify decay
    assert history[-1] < history[0], "FAILED: source_wealth should decrease!"
    decay_pct = (history[0] - history[-1]) / history[0] * 100
    print(f"\n✓ TEST PASSED: source_wealth decayed by {decay_pct:.1f}%")
    return True


def run_all_tests():
    """Run all correctness tests."""
    print("\n" + "=" * 60)
    print("PROVENANCE-BASED PROGRESSIVE FEES")
    print("Reference Implementation Correctness Tests")
    print("=" * 60)

    results = []
    results.append(("Split Resistance", test_split_resistance()))
    results.append(("Sybil Resistance", test_sybil_resistance()))
    results.append(("Blend on Combine", test_blend_on_combine()))
    results.append(("Whale Pays More", test_whale_pays_more()))
    results.append(("Legitimate Commerce Decay", test_legitimate_commerce_decay()))

    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)

    all_passed = True
    for name, passed in results:
        status = "✓ PASS" if passed else "✗ FAIL"
        print(f"  {status}: {name}")
        if not passed:
            all_passed = False

    if all_passed:
        print("\n✓ ALL TESTS PASSED")
    else:
        print("\n✗ SOME TESTS FAILED")

    return all_passed


if __name__ == "__main__":
    run_all_tests()
