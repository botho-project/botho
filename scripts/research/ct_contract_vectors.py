#!/usr/bin/env python3
"""Inactive CT1 integer specification checks; no crypto/production parity claim."""
import hashlib
import json
from pathlib import Path
import random
import unittest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path(__file__).with_name('fixtures')
U = (1 << 64) - 1
M = (1 << 128) - 1
YEAR = 6_307_200
HORIZON = 31_536_000
SCALE = 1_000_000


def uint(value, bits):
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value < 1 << bits:
        raise ValueError('noncanonical unsigned integer')
    return value


def charge(value, factor, elapsed, rate, year):
    for n in (value, factor, elapsed, year):
        uint(n, 64)
    uint(rate, 32)
    if not rate or not elapsed or not year:
        return 0
    progress = min(6000, max(1000, factor)) - 1000
    time = elapsed * SCALE // year
    annual = value * rate * progress // 10_000 // 5000
    return min(U, min(M, annual * time) // SCALE)


def reset(value, factor, output_factor, rate, year=YEAR, horizon=HORIZON):
    if output_factor >= factor:
        return 0
    return max(0, charge(value, factor, horizon, rate, year)
               - charge(value, output_factor, horizon, rate, year))


def spend(value, factor, output_factor, elapsed, rate):
    return max(charge(value, factor, elapsed, rate, YEAR),
               reset(value, factor, output_factor, rate))


def bucket(value, bits=2):
    uint(value, 68)  # 16 input charge sum fits; no u64 clamp here.
    if bits not in (2, 3, 4):
        raise ValueError('unratified bucket policy')
    if value == 0:
        return 0
    step = 1 << max(0, value.bit_length() - bits)
    return ((value + step - 1) // step) * step


def preimage(q, bits=2, upper=16 * U):
    """Exact bounded inverse, not an assumed q/2..q inequality."""
    def first_above(limit):
        low, high = 0, upper + 1
        while low < high:
            mid = (low + high) // 2
            if bucket(mid, bits) <= limit:
                low = mid + 1
            else:
                high = mid
        return low
    lo, hi = first_above(q - 1), first_above(q) - 1
    if lo > hi or lo > upper or bucket(lo, bits) != q:
        raise ValueError('not a canonical bucket endpoint')
    return lo, hi


def aggregate_fee(charges, outputs):
    if not 1 <= len(charges) <= 16 or not 1 <= outputs <= 16:
        raise ValueError('transaction count bounds')
    for d in charges:
        uint(d, 64)
    result = 250_000_000_000 * max(len(charges), outputs) + bucket(sum(charges))
    return uint(result, 64)


def tags(entries, bases):
    if len(entries) > 32 or entries != sorted(entries):
        raise ValueError('noncanonical tags')
    if len({c for c, _ in entries}) != len(entries):
        raise ValueError('duplicate origin')
    if any(c not in bases or not 1 <= w <= SCALE for c, w in entries):
        raise ValueError('unknown origin/invalid weight')
    if any(not 1500 <= b <= 6000 for b in bases.values()) or sum(w for _, w in entries) > SCALE:
        raise ValueError('factor/weight bound')
    return 1000 + sum(w * (bases[c] - 1000) for c, w in entries) // SCALE


def inherit(outputs, rings, bases):
    for entries in rings + outputs:
        tags(entries, bases)
    maxima = {}
    for entries in rings:
        for c, w in entries:
            maxima[c] = max(maxima.get(c, 0), w)
    return all(w <= maxima.get(c, 0) for entries in outputs for c, w in entries)


def age(heights, current):
    uint(current, 64)
    if len(heights) != 20 or any(not 0 <= h < current for h in heights):
        raise ValueError('invalid ring heights')
    return max(current - h for h in heights)


class ContractVectors(unittest.TestCase):
    def test_golden_arithmetic(self):
        data = json.loads((FIXTURES / 'ct-contract-vectors.json').read_text())
        for vector in data['charge']:
            with self.subTest(vector=vector['name']):
                self.assertEqual(charge(*vector['args']), vector['expected'])
        for vector in data['reset']:
            with self.subTest(vector=vector['name']):
                self.assertEqual(reset(*vector['args']), vector['expected'])
        for vector in data['bucket']:
            self.assertEqual(bucket(vector['input'], vector['bits']), vector['expected'])
        for vector in data['preimage']:
            self.assertEqual(preimage(vector['q'], vector['bits']), tuple(vector['expected']))

    def test_all_bucket_boundaries(self):
        # Every significant-bit endpoint across the entire supported 68-bit domain.
        checks = 0
        for bits in (2, 3, 4):
            candidates = set(range(40))
            for exponent in range(68):
                step = 1 << max(0, exponent - bits + 1)
                for digit in range(1 << (bits - 1), (1 << bits) + 1):
                    endpoint = digit * step
                    candidates.update((endpoint - 1, endpoint, endpoint + 1))
            for value in sorted(v for v in candidates if 0 <= v <= 16 * U):
                q = bucket(value, bits)
                self.assertGreaterEqual(q, value)
                self.assertLessEqual(q, bucket(value + 1, bits))
                if value:
                    self.assertLess((q - value) * (1 << (bits - 1)), value)
                lo, hi = preimage(q, bits)
                self.assertLessEqual(lo, value)
                self.assertGreaterEqual(hi, value)
                self.assertEqual(bucket(lo, bits), q)
                self.assertEqual(bucket(hi, bits), q)
                if lo:
                    self.assertLess(bucket(lo - 1, bits), q)
                if hi < 16 * U:
                    self.assertGreater(bucket(hi + 1, bits), q)
                checks += 1
        print(f'checked {checks} full-domain bucket boundary cases')
        with self.assertRaises(ValueError):
            preimage(5, 2)

    def test_rounding_is_not_linear_and_caps_are_independent(self):
        self.assertEqual(charge(49, 6000, HORIZON, 200, YEAR), 0)
        self.assertEqual(49 * 200 * 5000 * HORIZON // (50_000_000 * YEAR), 4)
        self.assertEqual(reset(51, 6000, 3500, 200), 5)
        self.assertEqual(51 * 200 * 2500 * HORIZON // (50_000_000 * YEAR), 2)
        self.assertEqual(reset(U, 6000, 5999, (1 << 32) - 1, 1), 0)
        self.assertEqual(reset(100, 6000, 3500, 200, 1, U - 1), 1)
        self.assertEqual(charge(U, 6000, U, (1 << 32) - 1, 1), U)
        self.assertEqual(bucket(5 + 5), 12)
        self.assertEqual(bucket(5) + bucket(5), 12)
        self.assertNotEqual(bucket(9 + 1), bucket(9) + bucket(1))

    def test_exact_integer_relations_and_field_bounds(self):
        # Check independently expressed quotient/remainder identities, not crypto.
        rng = random.Random(1301)
        for _ in range(4096):
            v, t, y = rng.getrandbits(64), rng.getrandbits(64), rng.getrandbits(64) or 1
            r, p = rng.getrandbits(32), rng.randrange(5001)
            product = v * r * p
            a, rem = divmod(product, 50_000_000)
            time, time_rem = divmod(t * SCALE, y)
            raw = a * time
            result = charge(v, p + 1000, t, r, y)
            self.assertEqual(product, a * 50_000_000 + rem)
            self.assertTrue(0 <= rem < 50_000_000)
            self.assertEqual(t * SCALE, time * y + time_rem)
            self.assertTrue(0 <= time_rem < y)
            self.assertLess(a, 1 << 91)
            self.assertLess(time, 1 << 84)
            self.assertLess(raw, 1 << 175)
            self.assertEqual(result, min(U, min(M, raw) // SCALE))
            self.assertNotEqual(product, (a + 1) * 50_000_000 + rem)
        self.assertLess(16 * U, 1 << 68)
        self.assertLess(1 << 175, (1 << 252) + 27742317777372353535851937790883648493)
        with self.assertRaises(ValueError):
            charge(U + 1, 6000, 1, 200, YEAR)

    def test_tags_dilution_and_honest_decoy_cost(self):
        bases = {1: 6000, 2: 1500}
        wealthy, background = [(1, SCALE)], []
        self.assertEqual(tags([], bases), 1000)
        self.assertEqual(tags([(2, SCALE)], bases), 1500)
        self.assertEqual(tags([(2, 500_000)], bases), 1250)
        self.assertEqual(tags([(1, 500_000), (2, 500_000)], bases), 3750)
        self.assertTrue(inherit([background], [wealthy] + [[]] * 19, bases))
        factors = [tags(t, bases) for t in [wealthy] + [[]] * 19]
        self.assertEqual(sum(factors) // 20, 1250)
        self.assertEqual(max(factors), 6000)
        self.assertEqual(spend(10**12, max(factors), 1000, 0, 200), 10**11)
        # Same public ring means same floor irrespective of actual real index.
        self.assertEqual(spend(10**12, 1000, 1000, 0, 200), 0)
        self.assertFalse(inherit([wealthy], [[]] * 20, bases))
        for malformed in ([(1, 0)], [(1, SCALE + 1)], [(1, 1), (1, 1)],
                          [(2, 1), (1, 1)], [(3, 1)], [(1, SCALE), (2, 1)]):
            with self.assertRaises(ValueError):
                tags(malformed, bases)
        with self.assertRaises(ValueError):
            tags([(i, 1) for i in range(33)], {i: 1500 for i in range(33)})
        self.assertEqual(age([99] * 19 + [1], 100), 99)
        for heights in ([], [100] * 20, [101] * 20, [-1] * 20):
            with self.assertRaises(ValueError):
                age(heights, 100)

    def test_decoy_cost_model(self):
        # Exact finite-population-style independent sampling toy model, not a
        # claim about production decoy distribution or the EpochOrigin Gini run.
        from fractions import Fraction
        high_cost = spend(10**12, 6000, 1000, 0, 200)
        self.assertEqual(high_cost, 10**11)
        report = []
        for percent in (0, 1, 10, 50, 100):
            p = Fraction(percent, 100)
            hit = 1 - (1 - p)**19
            self.assertTrue(0 <= hit <= 1)
            # A low-factor real input's 19 independently sampled decoys contain
            # a high member with this probability. Max then forces full reset.
            expected = hit * high_cost
            report.append({'high_factor_decoy_percent': percent,
                           'probability_high_max': f'{float(hit):.9f}',
                           'expected_reset_picocredits': str(expected.numerator // expected.denominator)})
        self.assertEqual(report[0]['expected_reset_picocredits'], '0')
        self.assertEqual(report[-1]['expected_reset_picocredits'], '100000000000')
        expected_report = json.loads((FIXTURES / 'ct-decoy-cost-model.json').read_text())
        self.assertEqual(report, expected_report['rows'])

    def test_fee_overflow_zero_and_conservation_bounds(self):
        self.assertEqual(aggregate_fee([0], 1), 250_000_000_000)
        self.assertEqual(aggregate_fee([0, 0], 1), 500_000_000_000)
        self.assertEqual(aggregate_fee([9, 1], 2), 500_000_000_012)
        with self.assertRaises(ValueError):
            aggregate_fee([U], 1)
        with self.assertRaises(ValueError):
            aggregate_fee([U] * 16, 16)
        with self.assertRaises(ValueError):
            aggregate_fee([], 1)
        with self.assertRaises(ValueError):
            aggregate_fee([0], 0)
        self.assertEqual(bucket(U), 1 << 64)  # no saturating underpayment
        q = bucket(9)
        lo, hi = preimage(q)
        self.assertTrue(9 <= q)
        self.assertFalse(lo <= 1 <= hi)  # upper-bound-only proof would accept 1
        fee = aggregate_fee([0], 1)
        self.assertEqual((10**12 - fee) + fee, 10**12)
        self.assertNotEqual((10**12 - fee + 1) + fee, 10**12)

    def test_source_inventory(self):
        inventory = json.loads((FIXTURES / 'ct-contract-inventory.json').read_text())
        for item in inventory['production']:
            data = (ROOT / item['path']).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), item['sha256'], item['path'])
            for symbol in item['symbols']:
                self.assertTrue(symbol.encode() in data, (item['path'], symbol))
        self.assertEqual(len(inventory['experiments']), 3)
        print(f"checked {len(inventory['production'])} SHA-bound source files; "
              'experimental commits recorded, not re-executed')


if __name__ == '__main__':
    unittest.main(verbosity=2)
