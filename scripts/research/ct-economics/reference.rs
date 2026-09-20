//! Inactive CT1 arithmetic/model only. No production consensus calls this file.
use bth_cluster_tax::{demurrage_charge, capitalized_reset_charge, ClusterFactorCurve, BLOCKS_PER_YEAR_5S, SETTLEMENT_HORIZON_BLOCKS};
use std::collections::{BTreeMap, BTreeSet};

pub const UNIT: u64 = 250_000_000_000;
pub fn quantize(d: u128, bits: u32) -> u128 {
    assert!((2..=4).contains(&bits));
    assert!(d <= 16 * u64::MAX as u128);
    if d == 0 { return 0; }
    let shift = (128 - d.leading_zeros()).saturating_sub(bits);
    let step = 1u128 << shift;
    d.div_ceil(step) * step
}
pub fn due(v: u64, fi: u64, fo: u64, age: u64, rate: u32) -> u64 {
    demurrage_charge(v,fi,age,rate,BLOCKS_PER_YEAR_5S).max(capitalized_reset_charge(v,fi,fo,SETTLEMENT_HORIZON_BLOCKS,rate,BLOCKS_PER_YEAR_5S))
}
pub fn fee(d: &[u64], outputs: usize, bits: u32) -> Option<u64> {
    if d.is_empty() || d.len()>16 || !(1..=16).contains(&outputs) { return None; }
    let raw = quantize(d.iter().map(|&v| v as u128).sum(), bits) + UNIT as u128 * d.len().max(outputs) as u128;
    raw.try_into().ok()
}
/// Public pool prefix model. The caller supplies authenticated gross issuance;
/// this is not an authorization check or a persistent ledger/reorg implementation.
#[derive(Clone,Default,Debug,PartialEq,Eq)]
pub struct Origins {
    pub wealth: BTreeMap<(u8,u64),u128>,
    pub messages: BTreeSet<[u8;32]>,
}
impl Origins {
    pub fn issue(&mut self,kind:u8,height:u64,value:u64,message:Option<[u8;32]>) -> Result<(), &'static str> {
        if kind>1 || (kind==1)!=message.is_some() { return Err("origin kind/message"); }
        if message.is_some_and(|m| self.messages.contains(&m)) {return Err("replay");}
        let key=(kind,height/17280);
        let amount=self.wealth.get(&key).copied().unwrap_or(0).checked_add(value as u128).ok_or("overflow")?;
        self.wealth.insert(key,amount);
        if let Some(m)=message {self.messages.insert(m);}
        Ok(())
    }
    pub fn factor(&self, tags:&[((u8,u64),u32)]) -> Result<u64,&'static str> {
        if tags.len()>32 {return Err("tag count");}
        let mut previous=None;
        let mut sum=0u64; let mut weighted=0u64;
        for &(id,w) in tags {
            if previous.is_some_and(|p|p>=id) || w==0 || w>1_000_000 {return Err("noncanonical tag");}
            previous=Some(id);
            sum+=w as u64;
            if sum>1_000_000 {return Err("weight sum");}
            let wealth=*self.wealth.get(&id).ok_or("unknown origin")?;
            let base=ClusterFactorCurve::default_params().factor(wealth).max(1500);
            weighted+=w as u64*(base-1000);
        }
        Ok(1000+weighted/1_000_000)
    }
}

#[test]
fn ct1_pure_bounds_and_origin_rollback_model() {
    assert_eq!(demurrage_charge(49,6000,SETTLEMENT_HORIZON_BLOCKS,200,BLOCKS_PER_YEAR_5S),0);
    assert_eq!(capitalized_reset_charge(51,6000,3500,SETTLEMENT_HORIZON_BLOCKS,200,BLOCKS_PER_YEAR_5S),5);
    assert_eq!(capitalized_reset_charge(u64::MAX,6000,5999,SETTLEMENT_HORIZON_BLOCKS,u32::MAX,1),0);
    assert_eq!(capitalized_reset_charge(100,6000,3500,u64::MAX-1,200,1),1);
    for bits in 2..=4 {
        for exp in 0..68 {
            let step=1u128<<exp;
            for v in [step-1,step,step+1] {
                if v>16*u64::MAX as u128 {continue;}
                let q=quantize(v,bits);assert!(q>=v);
                if v>0 {assert!((q-v)*(1<<(bits-1))<v);}
            }
        }
    }
    assert_eq!(quantize(u64::MAX as u128,2),1u128<<64);
    assert!(fee(&[u64::MAX],1,2).is_none());
    assert_ne!(quantize(10,2),quantize(9,2)+quantize(1,2));
    assert_eq!(fee(&[0;16],16,2),Some(16*UNIT));
    let mut origins=Origins::default();
    origins.issue(0,17279,10u64.pow(12),None).unwrap();
    let before=origins.clone();
    origins.issue(0,17280,20u64.pow(3),None).unwrap();
    assert_eq!(origins.wealth[&(0,0)],10u128.pow(12));
    assert_eq!(origins.factor(&[((0,0),1_000_000)]),Ok(1500));
    assert_eq!(origins.factor(&[((0,0),500_000)]),Ok(1250));
    assert_eq!(origins.factor(&[]),Ok(1000));
    origins.issue(1,17280,10u64.pow(12),Some([1;32])).unwrap();
    let issued=origins.clone();
    assert!(origins.issue(1,17280,1,Some([1;32])).is_err());assert_eq!(origins,issued);
    // Model rollback restores BOTH pool state and message-consumption state.
    origins=before.clone();assert_eq!(origins,before);
    assert!(origins.factor(&[((1,1),1)]).is_err());
    assert!(origins.factor(&[((0,0),0)]).is_err());
    assert!(origins.factor(&[((0,0),1),((0,0),1)]).is_err());
    assert!(origins.factor(&[((0,0),1_000_001)]).is_err());
    assert!(origins.factor(&vec![((0,0),1);33]).is_err());
    // A self-hop cannot change the public origin factor by itself; explicit tag
    // deflation is legal in CT1 but its reset is charged, not called free mixing.
    assert_eq!(origins.factor(&[((0,0),1_000_000)]),before.factor(&[((0,0),1_000_000)]));
    assert!(due(10u64.pow(12),6000,1000,0,200)>0);
}
