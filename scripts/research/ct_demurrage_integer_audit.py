#!/usr/bin/env python3
"""Reproduce CT design counterexamples; not a consensus implementation.

The reference is transcribed from cluster-tax/src/demurrage.rs at 65687275.
Keep the source comparison in the companion specification review: these
checks validate the arithmetic examples, not automatic Rust/Python parity.
"""

from fractions import Fraction

U64_MAX = (1 << 64) - 1
U128_MAX = (1 << 128) - 1
TIME_SCALE = 1_000_000
YEAR = 6_307_200


def current_charge(value, factor, elapsed, rate=200, year=YEAR):
    if rate == 0 or elapsed == 0 or year == 0:
        return 0
    progressivity = min(6000, max(1000, factor)) - 1000
    time_fraction = elapsed * TIME_SCALE // year
    annual = value * rate * progressivity // 10_000 // 5000
    charge = min(U128_MAX, annual * time_fraction) // TIME_SCALE
    return min(U64_MAX, charge)


def rational_coefficient(factor, elapsed, rate=200, year=YEAR):
    progressivity = min(6000, max(1000, factor)) - 1000
    return Fraction(rate * progressivity * (elapsed * TIME_SCALE // year),
                    10_000 * 5000 * TIME_SCALE)


def ceil_fraction(value):
    return (value.numerator + value.denominator - 1) // value.denominator


def main():
    horizon = 5 * YEAR
    assert current_charge(49, 6000, horizon) == 0
    assert ceil_fraction(49 * rational_coefficient(6000, horizon)) == 5
    downgrade = (current_charge(51, 6000, horizon)
                 - current_charge(51, 3500, horizon))
    collapsed = ceil_fraction(51 * (rational_coefficient(6000, horizon)
                                   - rational_coefficient(3500, horizon)))
    assert (downgrade, collapsed) == (5, 3)
    print("V=49: current five-year charge=0; single-rational ceiling=5 pico")
    print("V=51: current downgrade charge=5; collapsed ceiling=3 pico")

    # Illustrative 1-BTH bucket, not a proposed consensus parameter.
    value, quantum = 10**18, 10**12
    coefficient = rational_coefficient(5745, 1_234_567)
    p, q = coefficient.numerator, coefficient.denominator
    minimum = ceil_fraction(coefficient * value)
    charge = ((minimum + quantum - 1) // quantum) * quantum
    slack = q * charge - p * value
    assert (p, q) == (185_756_311, 50_000_000_000)
    assert charge == 3_716_000_000_000_000
    assert 0 <= value <= U64_MAX and 0 <= charge <= U64_MAX
    assert slack == 43_689_000_000_000_000_000_000
    assert slack.bit_length() == 76
    print(f"p={p}, q={q}, V={value}, d={charge}")
    print(f"q*d-p*V={slack}: {slack.bit_length()} bits, exceeds u64")


if __name__ == "__main__":
    main()
