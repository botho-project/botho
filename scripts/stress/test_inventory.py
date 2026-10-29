import copy
import sqlite3
import unittest
from types import SimpleNamespace

from controller import Controller
from selection import choose
from signer_history import prepare_blocks


class CanonicalDecoyInventoryTests(unittest.TestCase):
    def setUp(self):
        self.c = Controller.__new__(Controller)
        db = sqlite3.connect(':memory:')
        db.execute('CREATE TABLE reservations (input TEXT)')
        self.addCleanup(db.close)
        self.c.j = SimpleNamespace(db=db)
        self.owned = {
            'id': bytes([7] * 32).hex() + ':0',
            'key_image': 'image',
            'utxo': {
                'target_key': [1] * 32,
                'amount': 10**12,
                'created_at': 100,
                'tx_hash': [7] * 32,
                'output_index': 0,
            },
        }
        self.c.inventory = [[self.owned]]
        self.c.spent = [{'keyImage':'image','spent':False,'pending':False}]
        # At height 170, the selected age window is heights 93..107.
        self.c.blocks = [{'height':100,'outputs':[
            {'targetKey':bytes([i]*32).hex()} for i in range(2,21)]}]
        self.c.blocks[0]['outputs'].insert(0, {'targetKey':bytes([1]*32).hex(),
            'txHash':bytes([7]*32).hex(),'outputIndex':0})

    def test_recent_lottery_alias_does_not_make_inventory_ready(self):
        # Eighteen canonical decoys plus a recent payout falsely met the
        # nineteen-decoy gate. The payout's original is outside the age band.
        alias = self.c.blocks[0]['outputs'][-1]
        alias['lottery'] = True
        self.c.blocks.insert(0,{'height':1,'outputs':[
            {'targetKey':alias['targetKey']} ]})
        before = copy.deepcopy(self.c.inventory)
        self.assertEqual(self.c.spendable(0,170),[])
        self.assertEqual(self.c.inventory,before)  # accounting history retained
        self.c.blocks[-1]['outputs'].append({'targetKey':bytes([21]*32).hex()})
        self.assertEqual(self.c.spendable(0,170),[self.owned])

    def test_duplicate_target_outside_window_cannot_count_again(self):
        alias = self.c.blocks[0]['outputs'][-1]
        self.c.blocks.insert(0, {'height': 1, 'outputs': [dict(alias)]})
        self.assertEqual(self.c.spendable(0, 170), [])

    def test_owned_unflagged_alias_never_replaces_canonical_input(self):
        alias = copy.deepcopy(self.owned)
        alias['id'] = bytes([8] * 32).hex() + ':0'
        alias['utxo']['tx_hash'] = [8] * 32
        alias['utxo']['created_at'] = 101
        alias['utxo']['amount'] *= 2
        self.c.inventory[0] = [alias, self.owned, copy.deepcopy(self.owned)]
        self.c.blocks.append({'height':101,'outputs':[{
            'targetKey':bytes([1]*32).hex(),'txHash':bytes([8]*32).hex(),'outputIndex':0}]})
        self.assertEqual(self.c.canonical_inventory(0), [self.owned])
        self.assertEqual(self.c.spendable(0,170), [self.owned])
        self.assertEqual(len(self.c.inventory[0]), 3)

    def test_alias_with_equal_height_and_amount_requires_canonical_outpoint(self):
        alias = copy.deepcopy(self.owned)
        alias['id'] = bytes([8] * 32).hex() + ':0'
        alias['utxo']['tx_hash'] = [8] * 32
        self.c.inventory[0] = [alias]
        self.assertEqual(self.c.canonical_inventory(0), [])

    def test_legacy_outputs_without_lottery_flag_remain_eligible(self):
        self.assertEqual(self.c.spendable(0,170),[self.owned])

    def test_owned_lottery_value_is_retained_but_not_ready_to_spend(self):
        self.owned['utxo']['lottery'] = True
        self.assertEqual(self.c.spendable(0,170),[])
        self.assertEqual(self.c.inventory[0],[self.owned])


class V2LotteryInputTests(unittest.TestCase):
    """A V2 award is a campaign input; it is never a decoy (#1643)."""

    PAYOUT = bytes([7] * 32).hex() + ':1'

    def setUp(self):
        self.c = Controller.__new__(Controller)
        db = sqlite3.connect(':memory:')
        db.execute('CREATE TABLE reservations (input TEXT)')
        self.addCleanup(db.close)
        self.c.j = SimpleNamespace(db=db, limits={'max_single_signed_fee_picocredits': 10},
                                   set=lambda *a: None)
        self.payout = {
            'id': self.PAYOUT, 'key_image': 'award',
            'utxo': {'target_key': [1] * 32, 'amount': 10**12, 'created_at': 100,
                     'tx_hash': [7] * 32, 'output_index': 1, 'lottery': True,
                     'lottery_context': None},
        }
        self.c.inventory = [[self.payout]]
        self.c.spent = [{'keyImage': 'award', 'spent': False, 'pending': False}]
        # At height 170 the payout's age window is heights 93..107.
        self.row = {'targetKey': bytes([1] * 32).hex(), 'txHash': bytes([7] * 32).hex(),
                    'outputIndex': 1, 'lottery': True, 'cryptoOutputIndex': 0,
                    'amountCommitment': (10**12).to_bytes(8, 'little').hex(),
                    'lotteryContext': {'version': 2, 'baseIndex': 0, 'tweak': '03' + '00' * 31,
                                       'ordinal': 0, 'height': 100},
                    'winnerOutpoint': 'cd' * 32 + ':0'}
        self.decoys = [{'targetKey': bytes([i] * 32).hex(), 'txHash': bytes([i] * 32).hex(),
                        'outputIndex': 0} for i in range(2, 21)]
        self.c.blocks = [{'height': 100, 'outputs': [self.row, *self.decoys]}]
        self.payout['utxo']['lottery_context'] = dict(self.row['lotteryContext'])

    def test_v2_award_is_selected_as_a_campaign_input(self):
        self.assertEqual(self.c.canonical_inventory(0), [self.payout])
        self.assertEqual(self.c.spendable(0, 170), [self.payout])
        self.assertEqual(self.c.spendable(0, 170, 10**11, 1), [self.payout])
        selected, evidence = choose(self.c, 0, 170, 10**11, 1)
        self.assertEqual(selected, [self.payout])
        self.assertEqual(evidence['eligible'], 1)
        # The bounded signer history carries the selected row with its context.
        sent = [o for b in prepare_blocks(self.c.blocks, 170, [self.PAYOUT]) for o in b['outputs']]
        self.assertIn(self.row, sent)
        # Verbatim: the signer needs ``lottery: true`` and the full context.
        payout = next(o for o in sent if o['txHash'] == self.row['txHash'])
        self.assertIs(payout['lottery'], True)
        self.assertEqual(payout['lotteryContext'], self.row['lotteryContext'])
        self.assertEqual(payout['cryptoOutputIndex'], 0)
        self.assertEqual(payout['winnerOutpoint'], self.row['winnerOutpoint'])

    def test_prepare_blocks_refuses_a_selected_legacy_payout(self):
        from runtime import Gate

        del self.row['lotteryContext']
        with self.assertRaisesRegex(Gate, 'selected legacy lottery payout'):
            prepare_blocks(self.c.blocks, 170, [self.PAYOUT])
        self.row['lotteryContext'] = {'version': 2}
        with self.assertRaisesRegex(Gate, 'malformed V2 lottery row'):
            prepare_blocks(self.c.blocks, 170, [self.PAYOUT])

    def test_v2_payouts_never_count_as_decoys(self):
        # Replace one ordinary decoy with another V2 payout: 18 decoys remain.
        other = dict(self.row, targetKey=bytes([30] * 32).hex(), txHash=bytes([30] * 32).hex())
        self.c.blocks[0]['outputs'][-1] = other
        self.assertEqual(self.c.spendable(0, 170), [])
        self.assertEqual(choose(self.c, 0, 170)[0], [])
        sent = [o for b in prepare_blocks(self.c.blocks, 170, [self.PAYOUT]) for o in b['outputs']]
        self.assertNotIn(other, sent)
        self.c.blocks[0]['outputs'].append({'targetKey': bytes([21] * 32).hex(),
                                            'txHash': bytes([21] * 32).hex(), 'outputIndex': 0})
        self.assertEqual(self.c.spendable(0, 170), [self.payout])
        self.assertEqual(choose(self.c, 0, 170)[0], [self.payout])

    def test_legacy_payout_stays_unspendable(self):
        del self.row['lotteryContext']
        self.assertEqual(self.c.canonical_inventory(0), [])
        self.assertEqual(self.c.spendable(0, 170), [])
        self.assertEqual(self.c.inventory[0], [self.payout])  # retained as evidence


if __name__ == '__main__':
    unittest.main()
