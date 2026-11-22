import copy
import json
from pathlib import Path
import tempfile
import unittest

from plan import expand
from recovery_receipts import inspect_fleet, inspect_step


class RecoveryReceiptTests(unittest.TestCase):
    def setUp(self):
        self.step = dict(id='D4', action='netem', expect='progress', targets=['n5'], target_hosts=['observer'],
                         fault_start_offset=10, restore_offset=50, window_end_offset=80)
        self.controller = dict(action='netem', expect='progress', targets=['observer'], verdict='pass',
                               fault_observed='unknown', failures=[], awaiting=[])
        self.expected = [('apply', 'n5', 'netem'), ('restore', 'n5', 'restore'),
                         ('verify', 'n5', 'verify'), ('disarm', 'n5', 'disarm')]
        self.rows = [dict(phase='precheck', action='minting_check', role='n5', planned_at=110,
                          at=108.1, ok=True, minting_active=False)]
        for (phase, role, action), at in zip(self.expected, [110.2, 150.3, 151.4, 152.5]):
            self.rows.append(dict(phase=phase, role=role, action=action, planned_at=110 if phase=='apply' else 150,
                                  at=at, ok=True))

    def inspect(self, rows):
        return inspect_step(self.step, self.controller, rows, self.expected, 100.8)

    def test_separate_operator_clock_and_controller_result(self):
        result = self.inspect(self.rows)
        self.assertTrue(result['recovery_supported'])
        clock = result['operator_clock']['n5']
        self.assertAlmostEqual(clock['seconds_between_apply_and_restore_receipts'], 40.1)
        self.assertAlmostEqual(clock['restore_receipt_seconds_after_schedule'], 0.3)
        self.assertAlmostEqual(clock['verify_receipt_seconds_after_schedule'], 1.4)
        for field, value in (('verdict', 'in_progress'), ('fault_observed', False), ('awaiting', ['payments']),
                             ('failures', ['different tips'])):
            with self.subTest(field=field):
                before = self.controller[field]
                self.controller[field] = value
                result = self.inspect(self.rows)
                self.assertEqual(result['operator_receipts'], 'complete')
                self.assertFalse(result['recovery_supported'])
                self.controller[field] = before

    def test_partial_duplicate_failed_and_out_of_order_sequences(self):
        missing = self.rows[:2] + self.rows[3:]
        duplicated = self.rows + [self.rows[-1]]
        reordered = self.rows[:2] + [self.rows[3], self.rows[2], self.rows[4]]
        for rows in (missing, duplicated, reordered, self.rows[1:]):
            with self.subTest(rows=rows):
                self.assertFalse(self.inspect(rows)['recovery_supported'])
        for index, change in ((0, {'minting_active': True}), (0, {'at':111}), (1, {'role':'n4'}),
                              (1, {'ok':False}), (1, {'at':150}), (2, {'planned_at':149}),
                              (2, {'at':149}), (3, {'at':150.2}), (4, {'at':180})):
            with self.subTest(index=index, change=change):
                rows = copy.deepcopy(self.rows)
                rows[index].update(change)
                result = self.inspect(rows)
                self.assertFalse(result['recovery_supported'])
                self.assertEqual(result['operator_clock'], {})

    def test_preflight_alone_is_unobserved_and_target_mismatch_is_invalid(self):
        self.assertEqual(self.inspect(self.rows[:1])['operator_receipts'], 'unobserved')
        self.controller['targets'] = ['other']
        with self.assertRaisesRegex(ValueError, 'frozen plan'):
            self.inspect(self.rows)

    def test_repeated_operation_labels_need_distinct_receipts(self):
        self.expected.insert(1, self.expected[0])
        rows = [*self.rows[:2], dict(self.rows[1]), *self.rows[2:]]
        self.assertFalse(self.inspect(rows)['recovery_supported'])
        rows[2]['at'] += 0.1
        self.assertTrue(self.inspect(rows)['recovery_supported'])

    def test_fleet_report_selection_and_plan_provenance(self):
        plan = json.loads(Path(__file__).with_name('x-series-plan.json').read_bytes())
        summary, _ = expand(plan)
        run = 'q20261101-x1-endurance'
        inventory = [dict(role=role, hostname=f'{role}.{run}.internal', private_ip=f'10.0.0.{i}')
                     for i, role in enumerate(['controller','n1','n2','n3','n4','n5'], start=1)]
        rows = [dict(step=s['id'], targets=s['target_hosts'], action=s['action'], expect=s['expect'],
                     verdict='not_started', fault_observed=None) for s in summary['degrade_steps']]
        report = dict(status='incomplete', start=100.7, run_id=run+'-a', plan_sha256=summary['plan_sha256'], degrade=rows)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name, data in [('infrastructure-plan.json', {'run_id':run, 'created_at':90, 'host_end':300000}), ('inventory.json', inventory),
                               ('campaign-plan.json', plan), ('campaign-start.json', report), ('result.json', {})]:
                (root/name).write_text(json.dumps(data))
            (root/'faults.jsonl').write_bytes(b'')
            final = root/('final-report-'+run+'-a.json')
            final.write_text(json.dumps(report))
            self.assertTrue(all(s['operator_receipts'] == 'unobserved' for s in inspect_fleet(root)['steps']))
            second = root/('final-report-'+run+'-b.json')
            second.write_text(json.dumps(dict(report, run_id=run+'-b')))
            with self.assertRaisesRegex(ValueError, 'Several final reports'):
                inspect_fleet(root)
            with self.assertRaisesRegex(ValueError, 'activation disagree'):
                inspect_fleet(root, second)
            self.assertEqual(inspect_fleet(root, final)['attempt'], run+'-a')
            final.write_text(json.dumps(dict(report, plan_sha256='0'*64)))
            with self.assertRaisesRegex(ValueError, 'different plan'):
                inspect_fleet(root, final)


if __name__ == '__main__':
    unittest.main()
