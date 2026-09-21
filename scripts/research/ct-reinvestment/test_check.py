"""Small accounting controls, not a numerical history capture."""
import copy
import unittest
from check import INITIAL, amount, validate_row


def fixture():
    owners = []
    for owner in range(101):
        owners.append(dict(owner=owner, ordinary=str(INITIAL), award_unspent='0', award_consumed='0',
                           award_immature='0', award_deferred='0', award_ready='0', capture='0', fees='0',
                           payments_sent='0', payments_received='0', attempts=0, success=0, failures={}))
    return dict(seed=1306, mode='candidate_locked', owners=owners, fees='0', capture='0', burn='0', reserve='0',
                payment_attempts=113, payment_success=0, payment_failures={'unaffordable':113},
                consolidation_opportunities=11413, consolidation_attempts=0, consolidation_success=0,
                receipts=[], recycled_outputs=0, public_outputs=101, max_eligible=100)


class OwnerAccounts(unittest.TestCase):
    def test_locked_control_and_canonical_amounts(self):
        validate_row(fixture())
        for v in ['01', '-1', 1, True, '1.0', '١', str(2**128)]:
            with self.assertRaises(AssertionError): amount(v)

    def test_funded_recycling_keeps_consumed_awards_in_capture_only(self):
        row = fixture(); row['mode'] = 'candidate_age720'
        for o in row['owners']:
            o.update(attempts=113, failures={'no_private_input':113})
        row.update(consolidation_attempts=11413, consolidation_success=1, recycled_outputs=1,
                   payment_success=1, payment_failures={'unaffordable':112},
                   fees='1000000000000', capture='800000000000', burn='200000000000')
        row['owners'][1].update(ordinary=str(INITIAL-500_000_000_000), fees='500000000000')
        row['owners'][0].update(ordinary=str(INITIAL+300_000_000_000), capture='800000000000',
            award_consumed='800000000000', fees='500000000000', success=1, failures={'no_private_input':112})
        row['receipts']=[dict(owner=0,height=20800,inputs=['01'*36,'02'*36],output='03'*36,
                             consumed='800000000000',fee='500000000000',value='300000000000')]
        validate_row(row)
        bad=copy.deepcopy(row);bad['receipts'][0].update(fee='250000000000',value='550000000000')
        with self.assertRaises(AssertionError):validate_row(bad)
        bad=copy.deepcopy(row);bad['owners'][0]['award_unspent']='600000000000'
        with self.assertRaises(AssertionError):validate_row(bad)
        bad=copy.deepcopy(row);bad['receipts'][0]['inputs']=['01'*36,'01'*36]
        with self.assertRaises(AssertionError):validate_row(bad)

    def test_missing_duplicate_owner_and_failed_denominators(self):
        for mutate in [lambda r:r['owners'].pop(),lambda r:r['owners'][1].update(owner=0),
                       lambda r:r.update(payment_success=1),lambda r:r['owners'][0].update(ordinary='0')]:
            row=fixture();mutate(row)
            with self.assertRaises(AssertionError):validate_row(row)


if __name__=='__main__':unittest.main()
