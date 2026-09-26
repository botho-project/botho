import copy
import sqlite3
import unittest
from types import SimpleNamespace

from controller import Controller


class CanonicalDecoyInventoryTests(unittest.TestCase):
    def setUp(self):
        self.c = Controller.__new__(Controller)
        db = sqlite3.connect(':memory:')
        db.execute('CREATE TABLE reservations (input TEXT)')
        self.addCleanup(db.close)
        self.c.j = SimpleNamespace(db=db)
        self.owned = {'id':'grant:0','key_image':'image',
                      'utxo':{'target_key':[1]*32,'amount':10**12,'created_at':100,'tx_hash':[7]*32,'output_index':0}}
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
        alias['id'] = 'alias:0'
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
        alias['utxo']['tx_hash'] = [8]*32
        self.c.inventory[0] = [alias]
        self.assertEqual(self.c.canonical_inventory(0), [])

    def test_legacy_outputs_without_lottery_flag_remain_eligible(self):
        self.assertEqual(self.c.spendable(0,170),[self.owned])

    def test_owned_lottery_value_is_retained_but_not_ready_to_spend(self):
        self.owned['utxo']['lottery'] = True
        self.assertEqual(self.c.spendable(0,170),[])
        self.assertEqual(self.c.inventory[0],[self.owned])


if __name__ == '__main__':
    unittest.main()
