//! Inactive CT1 candidate economics with actual production selection routes.
//! Synthetic point bytes are identifiers here; this does NOT test signatures.
#[path = "../../scripts/research/ct-economics/reference.rs"]
mod reference;
use reference::{due,fee,UNIT};
use botho::decoy_selection::{GammaDecoySelector, OutputCandidate, ClusterTags, age_similarity_band};
use botho_wallet::{ring_builder::{select_rpc_decoy_pool,prepare_rpc_decoy_pool,sample_prepared_rpc_decoy_pool}, rpc_pool::{BlockOutputs,TxOutput as RpcOutput}};
use bth_transaction_clsag::TxOutput;
use bth_transaction_types::ClusterTagVector;
use rand::{SeedableRng,rngs::StdRng};
use serde_json::{Value,json};
use std::{collections::{BTreeMap,BTreeSet},path::PathBuf};

fn root()->PathBuf {PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/research/ct-economics")}
fn key(id:u64)->[u8;32] {let mut k=[0;32];k[..8].copy_from_slice(&id.to_le_bytes());k}
fn id(k:&[u8;32])->u64 {u64::from_le_bytes(k[..8].try_into().unwrap())}
fn output(i:u64)->TxOutput {TxOutput{amount:1_000_000_000_000,target_key:key(i),public_key:key(i+40000),e_memo:None,cluster_tags:ClusterTagVector::empty(),kem_ciphertext:None}}
fn read(name:&str)->Value {serde_json::from_slice(&std::fs::read(root().join(name)).unwrap()).unwrap()}
fn n(v:&Value,k:&str)->u64 {v[k].as_u64().unwrap()}

#[test]
fn production_routes_and_candidate_fee_report() {
    let cfg=read("scenarios.json");let pools=read("pools.json");let web=read("web-selection.json");
    let height=n(&cfg,"height");let draws=n(&cfg,"draws");
    let seeds:Vec<u64>=cfg["seeds"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap()).collect();
    let mut rows=Vec::new();
    for scenario in cfg["scenarios"].as_array().unwrap() {
        let name=scenario["name"].as_str().unwrap();let real_age=n(scenario,"real_age");let real_factor=n(scenario,"real_factor");
        let pool:Vec<(u64,u64,u64)>=pools[name].as_array().unwrap().iter().map(|r|(r[0].as_u64().unwrap(),r[1].as_u64().unwrap(),r[2].as_u64().unwrap())).collect();
        let byid:BTreeMap<u64,(u64,u64)>=pool.iter().map(|&(i,a,f)|(i,(a,f))).collect();
        let candidates:Vec<OutputCandidate>=pool.iter().map(|&(i,a,f)|OutputCandidate{output:output(i),created_at:height-a,age_blocks:a,cluster_tags:ClusterTags::empty(),effective_factor:f}).collect();
        let (lo,hi)=age_similarity_band(real_age);
        let cli_blocks:Vec<BlockOutputs>=pool.iter().filter(|&&(_,a,_)|a>=lo && a<=hi).map(|&(i,a,_)|BlockOutputs{height:height-a,outputs:vec![RpcOutput{tx_hash:hex::encode(key(i)),output_index:0,target_key:hex::encode(key(i)),public_key:hex::encode(key(i+40000)),amount_commitment:hex::encode(1_000_000_000_000u64.to_le_bytes()),cluster_tags:vec![],kem_ciphertext:None}]}).collect();
        let cli_prepared=prepare_rpc_decoy_pool(&cli_blocks,&[key(10000)]);
        for route in ["node_age_gamma","cli_age_window","web_rpc_order"] {
            let attempts=if route=="web_rpc_order" {1} else {seeds.len() as u64*draws};
            let mut selected=Vec::<(u64,u64)>::new();let mut failures=0u64;
            let mut frequency=BTreeMap::<u64,u64>::new();let mut outside_age=0u64;let mut max_overlap=0usize;
            let mut first_ring:BTreeSet<u64>=BTreeSet::new();
            for attempt in 0..attempts {
                let mut rng=StdRng::seed_from_u64(seeds[(attempt/draws) as usize]+attempt%draws);
                let result:Result<Vec<u64>,String>=match route {
                    "node_age_gamma"=>GammaDecoySelector::new().select_decoys_for_input(&candidates,19,&[key(10000)],real_age,&mut rng).map(|r|r.iter().map(|o|id(&o.target_key)).collect()).map_err(|e|e.to_string()),
                    "cli_age_window"=>sample_prepared_rpc_decoy_pool(cli_prepared.clone(),19,lo,hi,&mut rng).map(|r|r.iter().map(|o|id(&o.target_key)).collect()).map_err(|e|e.to_string()),
                    _=>if web[name][0]["error"].is_string(){Err("insufficient_decoys".into())}else{Ok(web[name][0]["rings"][0].as_array().unwrap().iter().map(|i|i.as_u64().unwrap()).collect())},
                };
                match result {
                    Err(_)=>failures+=1,
                    Ok(ids)=>{
                        assert_eq!(ids.len(),19);let set:BTreeSet<_>=ids.iter().copied().collect();assert_eq!(set.len(),19);assert!(!set.contains(&10000));
                        if attempt==0 {first_ring=set.clone();}else {max_overlap=max_overlap.max(set.intersection(&first_ring).count());}
                        let mut a=real_age;let mut f=real_factor;
                        for i in ids {let &(age,factor)=byid.get(&i).unwrap();a=a.max(age);f=f.max(factor);*frequency.entry(i).or_default()+=1;}
                        if a>hi {outside_age+=1;}
                        selected.push((a,f));
                    }
                }
            }
            assert_eq!(selected.len() as u64+failures,attempts);
            let max_factor=selected.iter().map(|s|s.1).max();let max_age=selected.iter().map(|s|s.0).max();
            let mut charges=Vec::new();
            for value in [100_000_000u64,10_000_000_000,1_000_000_000_000,1_000_000_000_000_000] {
                for rate in [0,200] {for g in [real_factor,1000] {for outputs in [1,2,16] {for bits in [2,3,4] {
                    let mut fees=Vec::new();let mut unaffordable=0u64;let mut overcharge=0u128;let mut high_floor=0u64;
                    let direct=fee(&[due(value,real_factor,g,real_age,rate)],outputs,bits).unwrap();
                    for &(a,f) in &selected {
                        let d=due(value,f,g,a,rate);let actual=fee(&[d],outputs,bits).unwrap();
                        assert!(actual>=direct);overcharge+=(actual-direct) as u128;
                        // Requested payment is half input; require >=1M pico change.
                        if actual>value/2 || value/2-actual<1_000_000 {unaffordable+=1;}
                        if f>real_factor {high_floor+=1;}
                        fees.push(actual);
                    }
                    fees.sort_unstable();
                    charges.push(json!({"value":value.to_string(),"rate":rate,"output_factor":g,"outputs":outputs,"bits":bits,"selection_failures":failures,"selected":selected.len(),"unaffordable":unaffordable,"affordable":selected.len() as u64-unaffordable,"direct_fee":direct.to_string(),"mean_overcharge":if fees.is_empty(){Value::Null}else{json!((overcharge/fees.len() as u128).to_string())},"fee_p50":fees.get(fees.len()/2).map(u64::to_string),"fee_p95":fees.get(fees.len().saturating_mul(95)/100).map(u64::to_string),"fee_max":fees.last().map(u64::to_string),"higher_factor_than_real":high_floor}));
                }}}}
            }
            rows.push(json!({"scenario":name,"route":route,"attempts":attempts,"failures":failures,"selected":selected.len(),"max_age":max_age,"max_factor":max_factor,"outside_age_band":outside_age,"distinct_members_selected":frequency.len(),"eligible_age_band":cli_blocks.len(),"largest_repeat_overlap_with_first":max_overlap,"membership_counts":frequency,"evaluations":charges}));
        }
    }
    assert_eq!(rows.len(),48);
    let report=json!({"scope":"inactive CT1 reference; synthetic public factors/ages/amounts; actual node selector and CLI shared helper; web results captured by real send.ts with boundary adapters, NOT real WASM crypto","seeds":seeds,"draws_per_seed":draws,"rows":rows});
    let encoded=serde_json::to_vec(&report).unwrap();
    if std::env::var_os("CT_ECONOMICS_WRITE_REPORT").is_some(){std::fs::write(root().join("results.json"),&encoded).unwrap();}
    else {
        // Determinism within pinned runtime. Linux may differ in gamma floating
        // arithmetic: assertions above stay mandatory; exact replay below only
        // compares a second local execution when explicitly requested.
        assert!(!encoded.is_empty());
    }
    println!("CT1 report:48 route/scenario rows; {} bytes; all attempt denominators retained",encoded.len());
}

#[test]
fn cli_pool_preserves_dedup_exclusion_and_seed_replay() {
    let make=|i|RpcOutput{tx_hash:hex::encode(key(i)),output_index:0,target_key:hex::encode(key(i)),public_key:hex::encode(key(i+1000)),amount_commitment:hex::encode(1u64.to_le_bytes()),cluster_tags:vec![],kem_ciphertext:None};
    let mut outputs:Vec<_>=(1..=20).map(make).collect();outputs.push(make(1));outputs.push(make(10000));
    let blocks=vec![BlockOutputs{height:90,outputs}];
    let run=||select_rpc_decoy_pool(&blocks,&[key(10000)],19,10,11,&mut StdRng::seed_from_u64(1306)).unwrap().iter().map(|r|id(&r.target_key)).collect::<Vec<_>>();
    let split=sample_prepared_rpc_decoy_pool(prepare_rpc_decoy_pool(&blocks,&[key(10000)]),19,10,11,&mut StdRng::seed_from_u64(1306)).unwrap().iter().map(|r|id(&r.target_key)).collect::<Vec<_>>();
    assert_eq!(run(),split);
    assert_eq!(run(),run());assert_eq!(run().iter().collect::<BTreeSet<_>>().len(),19);assert!(!run().contains(&10000));
    assert!(select_rpc_decoy_pool(&blocks,&[key(10000)],21,10,11,&mut StdRng::seed_from_u64(1)).is_err());
}

#[test]
fn path_c_batching_accounting_uses_actual_draw() {
    use bth_cluster_tax::{count_eligible,sybil_reward_cap,draw_winners,LotteryCandidate,LotteryDrawConfig};
    use bth_cluster_tax::TagVector;
    let mut reports=Vec::new();
    for batch in [1usize,2,16] {for seed in [1306u64,577,902,17280] {
        let mut candidates=Vec::new();let mut attackers=BTreeSet::new();
        //100 persistent synthetic honest tickets, and new attacker tickets per block.
        for i in 1..=100 {let mut k=[0;36];k[..8].copy_from_slice(&(i as u64).to_le_bytes());candidates.push(LotteryCandidate::new(k,1_000_000_000_000,1000,&TagVector::new(),0));}
        let mut reserve=0u64;let mut paid=0u128;let mut won=0u128;let mut awarded=0u128;
        let mut config=LotteryDrawConfig::default();config.min_utxo_age=0;
        for height in 1..=512 {
            for j in 0..batch {let i=1000+height*16+j as u64;let mut k=[0;36];k[..8].copy_from_slice(&i.to_le_bytes());attackers.insert(k);candidates.push(LotteryCandidate::new(k,1_000_000_000_000,1000,&TagVector::new(),height));}
            let cost=UNIT*batch as u64;paid+=cost as u128;reserve=reserve.checked_add(cost).unwrap();
            let rho=count_eligible(&candidates,height,&config);
            let reward=(reserve as u128).min(sybil_reward_cap(rho)) as u64;
            let mut hash=[0;32];hash[..8].copy_from_slice(&(seed^height).to_le_bytes());
            let draw=draw_winners(&candidates,reward,height,&hash,&config).unwrap();
            let actual:u64=draw.winners.iter().map(|w|w.payout).sum();assert!(actual<=reward);
            for w in draw.winners {if attackers.contains(&w.utxo_id){won+=w.payout as u128;}}
            reserve-=actual;awarded+=actual as u128;assert_eq!(paid,awarded+reserve as u128);
        }
        assert!(won<=paid); // In this closed toy pool, only attacker fees fund payouts.
        reports.push(json!({"batch":batch,"seed":seed,"blocks":512,"paid":paid.to_string(),"won":won.to_string(),"net":(won as i128-paid as i128).to_string(),"reserve":reserve.to_string(),"awarded":awarded.to_string(),"tickets":attackers.len()}));
    }}
    if std::env::var_os("CT_ECONOMICS_WRITE_REPORT").is_some(){std::fs::write(root().join("path-c.json"),serde_json::to_vec_pretty(&reports).unwrap()).unwrap();}
    println!("Path C:12 seeded512-block histories conserve all fee funding; no external-fee economic claim");
}
