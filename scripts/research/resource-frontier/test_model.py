import copy
import json
import unittest
from decimal import Decimal as D
from pathlib import Path
import model as m


class FrontierTests(unittest.TestCase):
    def setUp(self):
        self.config=json.loads((m.HERE/'scenarios.json').read_text())
        self.p=copy.deepcopy(self.config['presets'][0])
        self.c=copy.deepcopy(self.config['common'])

    def run_model(self,price='1',util='0.1',retention='720'):
        return m.evaluate(self.p,self.c,price,util,retention)

    def test_source_transcriptions(self):
        self.assertEqual(m.emission(0,0),dict(reward_pico=50*m.PICO,miner_pico=50*m.PICO,lottery_pico=0))
        e=m.emission(m.BLOCKS_YEAR,0)
        self.assertEqual(e['reward_pico'],25*m.PICO)
        self.assertEqual(e['lottery_pico'],D('2.5')*m.PICO)
        e=m.emission(5*m.BLOCKS_YEAR,611_010_000*m.PICO)
        expected=(611_010_000*m.PICO*200//10000+611_010_000*m.PICO*50//10000)//m.BLOCKS_YEAR
        self.assertEqual(e['reward_pico'],expected)
        self.assertEqual(e['miner_pico']+e['lottery_pico'],expected)
        for fee in [0,1,4,5,999,2**64-1]:
            split=m.fee_split(fee)
            self.assertEqual(split['pool_pico']+split['burn_pico'],fee)
            self.assertEqual(split['pool_pico'],fee*800//1000)

    def test_conservation_and_role_funding(self):
        r=self.run_model()
        self.assertEqual(sum(r['components_currency'].values()),r['total_resource_currency'])
        self.assertEqual(r['consensus_resource_currency']+r['verifier_resource_currency'],r['total_resource_currency'])
        self.assertEqual(r['remaining_role_separated_gap_currency'],max(D(0),r['consensus_resource_currency']-r['assigned_consensus_issuance_bth'])+r['verifier_resource_currency'])
        for capture,row in r['funding'].items():
            self.assertEqual(sum(row['hypothetical_routing'].values()),1)
            funded=row['fee_only']
            if funded['fee_bth'] is not None:
                self.assertAlmostEqual(funded['fee_bth']*r['accepted_transactions']*D(capture),r['total_resource_currency'],places=24)
        self.p['assigned_consensus_issuance_bth_hour']='100'
        r=self.run_model()
        self.assertEqual(r['remaining_role_separated_gap_currency'],r['verifier_resource_currency'])
        self.assertGreater(r['unused_consensus_support_currency'],0)
        self.assertEqual(r['assigned_consensus_issuance_bth']+r['unassigned_miner_issuance_bth'],r['available_miner_issuance_bth'])
        self.p['assigned_consensus_issuance_bth_hour']='100000000'
        with self.assertRaises(ValueError): self.run_model()

    def test_price_and_load_sensitivity(self):
        a=self.run_model();b=self.run_model(price='100')
        self.assertEqual(a['total_resource_currency'],b['total_resource_currency'])
        self.assertAlmostEqual(a['resource_equivalent_fee_bth'],b['resource_equivalent_fee_bth']*100,places=24)
        low=self.run_model(util='0.01');high=self.run_model(util='0.8')
        self.assertGreater(low['resource_equivalent_fee_bth'],high['resource_equivalent_fee_bth'])
        for name in ['verifier_incremental_electricity','verifier_cpu_amortization','bandwidth','storage']:
            self.assertEqual(low['components_currency'][name]/low['accepted_transactions'],high['components_currency'][name]/high['accepted_transactions'])

    def test_storage_units_and_replication(self):
        a=self.run_model()
        expected=D('10')*D('0.1')*3600*720*10000*10/D(10**9)
        self.assertEqual(a['stationary_storage_gb'],expected)
        self.assertEqual(a['components_currency']['storage'],expected*24*D('0.00003'))
        self.c['period_hours']='48'
        b=self.run_model()
        self.assertEqual(a['stationary_storage_gb'],b['stationary_storage_gb'])
        self.assertEqual(a['total_resource_currency']*2,b['total_resource_currency'])
        self.p['replicas']='20'
        c=self.run_model()
        self.assertEqual(b['components_currency']['storage']*2,c['components_currency']['storage'])
        self.assertEqual(b['components_currency']['verifier_cpu_amortization']*2,c['components_currency']['verifier_cpu_amortization'])

    def test_zero_overload_and_nonstationary(self):
        r=self.run_model(util='0')
        self.assertIsNone(r['resource_equivalent_fee_bth'])
        self.assertGreater(r['total_resource_currency'],0)
        r=self.run_model()
        self.assertIsNone(r['funding']['0']['fee_only']['fee_bth'])
        self.assertIn('no_indirect_revenue',r['funding']['0']['fee_only']['status'])
        self.c['verify_seconds']='100'
        r=self.run_model()
        self.assertEqual(r['status'],'overloaded')
        self.assertIsNone(r['resource_equivalent_fee_bth'])
        self.assertIsNone(r['funding']['1']['fee_only']['fee_bth'])
        self.assertEqual(self.run_model(retention=None)['status'],'nonstationary_append_only_storage')
        self.assertEqual(m.funding('1','0','1','0','1')['fee_bth'],0)

    def test_affordability_and_security_separation(self):
        a=m.affordability('0.25','25','0.01')
        self.assertTrue(a['within_supplied_fraction'])
        self.assertEqual(a['minimum_payment_at_supplied_fraction_bth'],25)
        self.assertFalse(m.affordability('0.25','24','0.01')['within_supplied_fraction'])
        r=self.run_model()
        self.assertAlmostEqual((r['resource_plus_additional_security_fee_bth']-r['resource_equivalent_fee_bth'])*r['accepted_transactions'],r['additional_security_currency'],places=24)

    def test_invalid_exact_inputs(self):
        for bad in [True,0.1,'NaN','Infinity','-1','not-a-number']:
            with self.assertRaises(ValueError): m.dec(bad)
        for bad in [True,-1,0.1]:
            with self.assertRaises(ValueError): m.emission(bad,0)
        for price,util in [('0','0.1'),('1','1.1')]:
            with self.assertRaises(ValueError):self.run_model(price=price,util=util)
        with self.assertRaises(ValueError):m.affordability('1','0','0.1')
        with self.assertRaises(ValueError):m.funding('1','1','1','1.1','0')
        with self.assertRaises(ValueError):m.emission(2**64,0)
        with self.assertRaises(ValueError):m.emission(0,2**128)
        with self.assertRaises(ValueError):m.fee_split(2**64)
        self.p['cores_per_replica']='0'
        with self.assertRaises(ValueError):self.run_model()

    def test_reproducible_report_matrix(self):
        a=m.reports(self.config);b=m.reports(self.config)
        self.assertEqual(a,b)
        changed=copy.deepcopy(self.config);changed['common']['electricity_currency_kwh']='0.2'
        changed_report=json.loads(m.reports(changed)['report.json'])
        report=json.loads(a['report.json'])
        self.assertNotEqual(report['canonical_config_sha256'],changed_report['canonical_config_sha256'])
        self.assertEqual(changed_report['actual_config'],changed)
        self.assertEqual(len(report['rows']),72)
        self.assertEqual(sum(r['accepted_transactions']=='0' for r in report['rows']),18)
        self.assertEqual(len(a['affordability.csv'].splitlines())-1,486)
        for name,content in a.items():
            self.assertEqual((m.HERE/name).read_text(),content)


if __name__=='__main__':unittest.main()
