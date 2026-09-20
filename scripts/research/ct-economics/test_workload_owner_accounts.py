import copy
import unittest
from workload_owner_accounts import validate_owner_accounts


def fixture():
    values = [(2, 1, 10, 0, 88, 1, 89), (0, 2, 0, 10, 110, 2, 112),
              (5, 1, 0, 0, 95, 1, 96)]
    fields = ('fees', 'capture', 'payments_sent', 'payments_received', 'spendable', 'locked', 'accounted')
    owners = [dict(owner=i, **dict(zip(fields, map(str, value)))) for i, value in enumerate(values)]
    return dict(owners=owners, awarded='4', attacker_capture='1', honest_fees='2',
                attacker_fees='5', honest_spendable='198', attacker_spendable='95',
                honest_locked_payouts='3', attacker_locked_payouts='1',
                honest_accounted_value='201', attacker_accounted_value='96')


class OwnerAccounts(unittest.TestCase):
    def test_valid_transfers_fees_and_locked_awards(self):
        validate_owner_accounts(fixture(), 100, 2)

    def test_exact_owner_identity_and_count(self):
        for change in ('missing', 'duplicate', 'reordered', 'boolean'):
            row = fixture()
            if change == 'missing':
                row['owners'].pop()
            elif change == 'duplicate':
                row['owners'][1]['owner'] = 0
            elif change == 'reordered':
                row['owners'].reverse()
            else:
                row['owners'][0]['owner'] = False
            with self.subTest(change=change), self.assertRaises(AssertionError):
                validate_owner_accounts(row, 100, 2)

    def test_owner_and_aggregate_inconsistency(self):
        changes = [lambda r: r['owners'][0].update(fees='3'),
                   lambda r: r['owners'][1].update(locked='1'),
                   lambda r: r.update(honest_fees='3'),
                   lambda r: r.update(awarded='5'),
                   lambda r: r['owners'][0].update(payments_received='1')]
        for change in changes:
            row = fixture()
            change(row)
            with self.assertRaises(AssertionError):
                validate_owner_accounts(row, 100, 2)

    def test_reassignment_cannot_hide_behind_equal_group_aggregate(self):
        row = fixture()
        # Aggregate honest fees still equal2, but each owner's balance equation
        # identifies who actually paid. A group-only check would miss this.
        row['owners'][0]['fees'] = '1'
        row['owners'][1]['fees'] = '1'
        with self.assertRaises(AssertionError):
            validate_owner_accounts(row, 100, 2)

    def test_money_is_canonical_decimal_string(self):
        for value in ('-1', '01', True, 1, 1.0, '1e3', '١', str(2**128)):
            row = copy.deepcopy(fixture())
            row['owners'][0]['fees'] = value
            with self.subTest(value=value), self.assertRaises(AssertionError):
                validate_owner_accounts(row, 100, 2)


if __name__ == '__main__':
    unittest.main()
