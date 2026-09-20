#!/usr/bin/env python3
"""Fixed-input payment sensitivity from exact observed fee histograms, not quantiles."""
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]

def affordable(value, fee, fraction, change_floor):
    for x in (value, fee, fraction, change_floor):
        if type(x) is not int or x < 0:
            raise ValueError('nonnegative integers required')
    if fraction > 100:
        raise ValueError('percentage exceeds 100')
    payment = value * fraction // 100
    return value >= payment + fee + change_floor

class Boundaries(unittest.TestCase):
    def test_rounding_and_change(self):
        self.assertTrue(affordable(101, 51, 50, 0))
        self.assertFalse(affordable(101, 51, 50, 1))
        self.assertTrue(affordable(101, 50, 50, 1))
        self.assertFalse(affordable(0, 1, 10, 0))
    def test_types(self):
        for value in (-1, True, 0.5):
            with self.assertRaises(ValueError): affordable(value, 0, 50, 0)
        with self.assertRaises(ValueError): affordable(100, 0, 101, 0)

def main():
    raw = (HERE / 'results.json').read_bytes()
    data = json.loads(raw)
    marginal_raw = (HERE / 'marginal.json').read_bytes()
    marginal = json.loads(marginal_raw)
    assert len(marginal['rows']) == 32
    rows = []
    for row in data['rows']:
        assert row['selected'] + row['failures'] == row['attempts']
        cases = {e['value']: e for e in row['evaluations'] if e['bits'] == 2 and e['rate'] == 200 and e['outputs'] == 2 and e['output_factor'] == 1000}
        assert len(cases) == 4
        for value, e in cases.items():
            histogram = e['fee_histogram']
            assert sum(histogram.values()) == row['selected']
            for fraction in (10, 50, 90):
                for floor in (0, 1_000_000):
                    funded = sum(count for fee, count in histogram.items() if affordable(int(value), int(fee), fraction, floor))
                    assert 0 <= funded <= row['selected']
                    rows.append(dict(scenario=row['scenario'], route=row['route'], value=value,
                        payment_percent=fraction, change_floor=str(floor), attempts=row['attempts'],
                        selection_failures=row['failures'], selected=row['selected'],
                        affordable=funded, unaffordable=row['selected']-funded,
                        affordable_fraction_of_successful=None if not row['selected'] else funded/row['selected']))
    assert len(rows) == 48*4*3*2
    historical = json.loads((HERE/'summary.json').read_text())
    paths = list(historical['source_sha256']) + ['botho/tests/ct_economics_marginal.rs', 'scripts/research/ct-economics/marginal_summary.py']
    result = dict(scope='Inactive paired finite fixed-capital histories and fixed-input payment sensitivity; no stationary, universal Sybil or profitability claim',
        stacked_base='58cb0345aa23fda047d23f4474fb2562bfe3f6a9',
        runtime=platform.platform(), rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),
        source_sha256={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in paths},
        reviewed_source_changes={'botho/tests/ct_economics_simulation.rs':'Adds exact selected-fee histogram output and count assertion only; sampler and fee calculation unchanged.'},
        full_results_sha256=hashlib.sha256(raw).hexdigest(), marginal_sha256=hashlib.sha256(marginal_raw).hexdigest(),
        sensitivity=rows)
    (HERE/'marginal-summary.json').write_text(json.dumps(result,indent=2)+'\n')
    print(f'{len(rows)} sensitivity rows; 32 fixed-capital histories; {len(paths)} source hashes')

if __name__ == '__main__':
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Boundaries)
    if not unittest.TextTestRunner().run(suite).wasSuccessful(): raise SystemExit(1)
    main()
