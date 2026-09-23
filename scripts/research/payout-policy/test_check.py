import copy
import unittest
from check import THRESHOLD, check_block


def row(policy, available, rho, count, payout):
    # No new inflow: all value comes from the provided previous reserve.
    return dict(policy=policy,fees=0,emission=0,burn=0,available=available,
                cap=rho*250_000_000_000,block_reward=10**15,eligible=rho,
                distribution=payout,reserve=available-payout,
                awards=[dict(id=str(i),value=payout//count+(payout%count if i==count-1 else 0)) for i in range(count)])

class PolicyChecks(unittest.TestCase):
    def test_threshold_neighbors_and_remainder(self):
        for n in [THRESHOLD-1,THRESHOLD,THRESHOLD+1,4*THRESHOLD-1,4*THRESHOLD,4*THRESHOLD+1]:
            k=4 if n>=4*THRESHOLD else 0
            check_block(row('threshold_reserve',n,100,k,n if k else 0),n)
            k=min(4,n//THRESHOLD)
            check_block(row('adaptive_count',n,100,k,n if k else 0),n)
    def test_empty_and_cap_stall(self):
        for p in ['baseline','threshold_reserve','adaptive_count']:
            check_block(row(p,0,100,0,0),0)
            check_block(row(p,10**12,0,0,0),10**12)
        for p in ['threshold_reserve','adaptive_count']:
            check_block(row(p,10**12,1,0,0),10**12)
    def test_corrupt_recurrence_and_types(self):
        good=row('adaptive_count',400_000_000_000,100,1,400_000_000_000)
        for key,value in [('reserve',1),('distribution',1),('fees',True),('eligible',-1),('cap',2**64),('available',1.5)]:
            bad=copy.deepcopy(good);bad[key]=value
            with self.assertRaises(AssertionError):check_block(bad,400_000_000_000)
        bad=copy.deepcopy(good);bad['awards'][0]['value']-=1
        with self.assertRaises(AssertionError):check_block(bad,400_000_000_000)
    def test_fee_split_and_saturation(self):
        r=row('adaptive_count',2**128-1,100,4,25_000_000_000_000)
        r.update(fees=5,burn=1)
        check_block(r,2**128-1)

if __name__=='__main__':unittest.main()

class AttributionChecks(unittest.TestCase):
    def test_fifo_partial_and_expiry(self):
        from check import derive_fifo
        first=row('threshold_reserve',400_000_000_000,4,0,0)
        first.update(height=20000,fees=500_000_000_000,burn=100_000_000_000,reason='cap_stall',cohort_expiry={'20001':4})
        second=row('adaptive_count',800_000_000_000,4,3,800_000_000_000)
        second.update(height=20001,fees=500_000_000_000,burn=100_000_000_000,reason='draw',cohort_expiry={'20002':4})
        for a in second['awards']:a['owner']=0
        result=derive_fifo([first,second])
        self.assertEqual([r['expired_cohort'] for r in result['releases']],[4,0])
        self.assertEqual(result['unreleased'],[])
        self.assertEqual(sum(r['value'] for r in result['releases']),800_000_000_000)
        bad=copy.deepcopy(second);bad['reason']='accumulating'
        with self.assertRaises(AssertionError):derive_fifo([first,bad])
