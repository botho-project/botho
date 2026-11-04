//! Bounded actual-commit evidence for #1889. One fresh-genesis chain under
//! an explicitly altered difficulty profile, reused across all scenarios.
//! No snapshot seeding, maturity override, fake verifier or DRT double.
use crate::supply::{self, fixture_source_hash, Saved, PROFILE};
use botho::{
    block::{calculate_block_reward, Block},
    ledger::{
        formal_drt_supply::{
            self as oracle, Context, Ghost, KeyHistory, Observation, Outcome, OutputKind, RealInput,
        },
        Ledger, LedgerError,
    },
    transaction::{Transaction, TxOutput, Utxo, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

// Public fixture identities, never funded production wallets.
const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const RECEIVER: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const MATURITY: u64 = 720;

fn template(ledger: &Ledger, owner: &Wallet, txs: Vec<Transaction>) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let mut block = Block::new_template_with_txs(
        &ledger.get_tip().unwrap(),
        &owner.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        txs,
    );
    block.header.timestamp = state.fast_chain_timestamp();
    block.minting_tx.timestamp = block.header.timestamp;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    assert_eq!(state.difficulty, u64::MAX, "explicit altered test profile");
    assert!(block.header.is_valid_pow(), "actual PoW is never bypassed");
    block
}

fn signed(
    ledger: &Ledger,
    owner: &Wallet,
    to: &Wallet,
    input: &Utxo,
    fee: u64,
) -> (Transaction, RealInput, u128) {
    let start = Instant::now();
    let tx = owner
        .create_private_transaction(
            std::slice::from_ref(input),
            vec![TxOutput::new_hybrid_to_address(
                input.output.amount.checked_sub(fee).unwrap(),
                &to.default_address(),
                0,
                None,
                Default::default(),
            )
            .unwrap()],
            fee,
            ledger.get_chain_state().unwrap().height,
            ledger,
        )
        .expect("real production wallet signing must succeed");
    (
        tx,
        RealInput {
            outpoint: input.id,
            amount: input.output.amount,
        },
        start.elapsed().as_micros(),
    )
}

struct Fixture {
    dir: tempfile::TempDir,
    ledger: Option<Ledger>,
    ghost: Ghost,
    blocks: Vec<Block>,
    watched: Vec<[u8; 32]>,
    evidence: PathBuf,
    cases: Vec<Value>,
    began: Instant,
    setup_us: u128,
    replay_cases: Vec<Value>,
    frame_controls: Vec<Value>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).expect("local setup failures fail the test");
        assert_eq!(ledger.get_chain_state().unwrap().height, 0);
        assert_eq!(ledger.get_chain_state().unwrap().total_mined, 0);
        ledger.set_difficulty(u64::MAX).unwrap();
        let evidence = std::env::var_os("SUPPLY_ORACLE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("botho-supply-oracle-{}", std::process::id()))
            });
        std::fs::create_dir_all(&evidence).unwrap();
        Self {
            dir,
            ledger: Some(ledger),
            ghost: Ghost::default(),
            blocks: vec![],
            watched: vec![],
            evidence,
            cases: vec![],
            began: Instant::now(),
            setup_us: 0,
            replay_cases: vec![],
            frame_controls: vec![],
        }
    }

    fn ledger(&self) -> &Ledger {
        self.ledger.as_ref().unwrap()
    }

    fn history(&self) -> KeyHistory {
        let mut keys: BTreeMap<_, _> = self.watched.iter().map(|k| (*k, None)).collect();
        for block in &self.blocks {
            for key in block.transactions.iter().flat_map(Transaction::key_images) {
                assert!(keys.insert(key, Some(block.height())).flatten().is_none());
            }
        }
        keys.into_iter().collect()
    }

    fn save_case(
        &mut self,
        name: &str,
        candidate: &Block,
        inputs: &[RealInput],
        rejection: Option<(bool, &str)>,
    ) -> (PathBuf, String, Saved, u128) {
        let start = Instant::now();
        let pre = oracle::snapshot(self.ledger(), &self.watched).unwrap();
        let network = format!("{:?}", self.ledger().network());
        let directory = self.evidence.join(format!("replay-{name}"));
        std::fs::create_dir(&directory).unwrap();
        drop(self.ledger.take()); // All handles closed BEFORE copying.
        let (data_sha256, data_bytes) =
            supply::copy_closed(self.dir.path(), &directory.join("store")).unwrap();
        let saved = Saved {
            schema: 1,
            profile: PROFILE.into(),
            profile_hash: supply::profile_hash(),
            source_hash: oracle::source_hash(),
            fixture_source_hash: fixture_source_hash(),
            network,
            data_sha256,
            data_bytes,
            pre,
            ghost: self.ghost.sources(),
            history: self.history(),
            candidate: candidate.clone(),
            inputs: inputs.to_vec(),
            rejection: rejection.map(|(stage, error)| (stage, error.into())),
        };
        let sha = saved.write(&directory).unwrap();
        self.ledger = Some(Ledger::open(self.dir.path()).unwrap());
        assert_eq!(
            oracle::snapshot(self.ledger(), &self.watched)
                .unwrap()
                .fingerprint()
                .unwrap(),
            saved.pre.fingerprint().unwrap()
        );
        (directory, sha, saved, start.elapsed().as_micros())
    }

    fn bound(&self) {
        assert!(
            self.began.elapsed() < Duration::from_secs(360),
            "bounded oracle profile exceeded 360s"
        );
    }

    fn seal(&self, owner: &Wallet, txs: Vec<Transaction>) -> (Block, u128) {
        let block = template(self.ledger(), owner, txs);
        let start = Instant::now();
        (
            self.ledger()
                .seal_block(block)
                .expect("production V2 sealing"),
            start.elapsed().as_micros(),
        )
    }

    /// Cold chain construction uses the SAME production seal/add path. Read
    /// each new persisted row once; do not take 720 full-table snapshots.
    fn advance(&mut self, owner: &Wallet, height: u64) {
        let start = Instant::now();
        while self.ledger().get_chain_state().unwrap().height < height {
            self.bound();
            let (block, _) = self.seal(owner, vec![]);
            self.ledger()
                .add_block(&block)
                .expect("setup block must actually commit");
            if block.height() <= MATURITY {
                assert!(block.lottery_outputs.is_empty());
            }
            for index in 0..=block.lottery_outputs.len() {
                let row = self
                    .ledger()
                    .get_utxo(&UtxoId::new(block.hash(), index as u32))
                    .unwrap()
                    .unwrap();
                self.ghost.add_output(&row).unwrap();
            }
            let state = self.ledger().get_chain_state().unwrap();
            assert_eq!(
                self.ghost.value().unwrap()
                    + self.ledger().get_lottery_pool().unwrap()
                    + state.total_fees_burned,
                state.total_mined
            );
            self.blocks.push(block);
        }
        self.setup_us += start.elapsed().as_micros();
    }

    fn coinbase(&self, height: u64) -> Utxo {
        self.ledger()
            .get_utxo(&UtxoId::new(self.blocks[height as usize - 1].hash(), 0))
            .unwrap()
            .unwrap()
    }

    fn reopen(&mut self, expected: &str) {
        drop(self.ledger.take());
        self.ledger = Some(Ledger::open(self.dir.path()).expect("reopen infrastructure failure"));
        assert_eq!(
            oracle::snapshot(self.ledger(), &self.watched)
                .unwrap()
                .fingerprint()
                .unwrap(),
            expected
        );
    }

    fn run(
        &mut self,
        name: &str,
        block: Block,
        inputs: Vec<RealInput>,
        signing_us: u128,
        seal_us: u128,
        rejection: Option<(bool, &str)>,
    ) -> Observation {
        self.bound();
        self.watched
            .extend(block.transactions.iter().flat_map(Transaction::key_images));
        let history = self.history();
        let replay = if matches!(
            name,
            "signed_transfer_0"
                | "late_invalid_signature_balance"
                | "real_maturity_payout"
                | "real_payout_signed_spend"
        ) {
            Some(self.save_case(name, &block, &inputs, rejection))
        } else {
            None
        };
        // Save replay data before any assertion, including unexpected failures.
        std::fs::write(
            self.evidence.join("chain.json"),
            serde_json::to_vec(&json!({
            "profile":PROFILE,"difficulty":u64::MAX,"maturity":MATURITY,
            "genesis":Block::genesis(),"blocks":self.blocks}))
            .unwrap(),
        )
        .unwrap();
        let file = format!("{name}.json");
        std::fs::write(
            self.evidence.join(&file),
            serde_json::to_vec(&json!({
            "profile":PROFILE,"source_hash":oracle::source_hash(),
            "fixture_source_hash":fixture_source_hash(),"real_inputs":inputs,"candidate":block}))
            .unwrap(),
        )
        .unwrap();
        let observation = oracle::attempt(self.ledger(), block, &self.watched).expect(
            "infrastructure/observation errors must fail, not increment rejection coverage",
        );
        let bytes = serde_json::to_vec(&json!({"profile":PROFILE,"source_hash":oracle::source_hash(),
            "fixture_source_hash":fixture_source_hash(),"real_inputs":inputs,"observation":observation})).unwrap();
        std::fs::write(self.evidence.join(&file), &bytes).unwrap();
        if let Some((structure_accepted, message)) = rejection {
            assert_eq!(
                observation.structure_accepted, structure_accepted,
                "{name}: wrong rejection stage"
            );
            match &observation.outcome {
                Outcome::Rejected { error } => assert!(error.contains(message), "{name}: {error}"),
                Outcome::Accepted => panic!("{name}: negative block committed"),
            }
            assert_eq!(
                observation.before.fingerprint().unwrap(),
                observation.after.fingerprint().unwrap(),
                "{name}: rollback"
            );
            assert!(observation.new_outputs.is_empty());
            assert!(matches!(
                self.ledger().get_block(observation.block.height()),
                Err(LedgerError::BlockNotFound(_))
            ));
        } else {
            assert_eq!(observation.outcome, Outcome::Accepted, "{name}");
            assert!(observation.structure_accepted);
            // Bounded negative control: changing an OBSERVED field must be
            // caught by the independent comparison, then restoration passes.
            if name == "signed_transfer_0" {
                let mut corrupted = observation.clone();
                corrupted.after.pool += 1;
                assert!(self
                    .ghost
                    .check(&corrupted, &inputs, &history)
                    .unwrap_err()
                    .contains("cross-addition"));
                corrupted.after.pool -= 1;
                self.ghost.check(&corrupted, &inputs, &history).unwrap();
                corrupted.after.pool = u128::MAX;
                assert!(self
                    .ghost
                    .check(&corrupted, &inputs, &history)
                    .unwrap_err()
                    .contains("arithmetic overflow"));
            }
            if name == "signed_transfer_1" || name == "real_maturity_payout" {
                self.frame_controls.extend(frame_controls(
                    &self.ghost,
                    &observation,
                    &inputs,
                    &history,
                ));
            }
            self.ghost
                .apply(&observation, &inputs, &history)
                .expect("actual persisted value flow");
            assert_eq!(
                self.ledger()
                    .get_block(observation.block.height())
                    .unwrap()
                    .hash(),
                observation.block.hash()
            );
            assert_eq!(
                observation.after.state.height,
                observation.before.state.height + 1
            );
            assert_eq!(observation.after.state.tip_hash, observation.block.hash());
            for tx in &observation.block.transactions {
                for key in tx.key_images() {
                    assert_eq!(
                        self.ledger().is_key_image_spent(&key).unwrap(),
                        Some(observation.block.height())
                    );
                }
            }
            self.blocks.push(observation.block.clone());
        }
        self.reopen(&observation.after.fingerprint().unwrap());
        let entry = json!({"scenario":name,"accepted":rejection.is_none(),"infrastructure":0,
            "structure_accepted":observation.structure_accepted,"height":observation.block.height(),
            "block_hash":hex::encode(observation.block.hash()),"prestate_hash":observation.before.fingerprint().unwrap(),
            "signing_us":signing_us,"seal_us":seal_us,"precheck_us":observation.precheck_us,
            "add_block_us":observation.add_block_us,"observation_us":observation.observation_us,
            "artifact":file,"sha256":hex::encode(Sha256::digest(&bytes))});
        eprintln!("SUPPLY_ORACLE_CASE {entry}");
        self.cases.push(entry);
        if let Some((directory, sha, saved, capture_us)) = replay {
            self.replay_cases
                .push(measure_replay(name, &directory, &sha, &saved, capture_us));
        }
        observation
    }
}

fn frame_controls(
    ghost: &Ghost,
    observation: &Observation,
    inputs: &[RealInput],
    history: &KeyHistory,
) -> Vec<Value> {
    let mut results = vec![];
    let mut sources = ghost.sources();
    sources[0].amount += 1;
    let mut wrong = Ghost::from_sources(&sources).unwrap();
    let unchanged = serde_json::to_vec(&wrong.sources()).unwrap();
    assert!(wrong
        .apply(observation, inputs, history)
        .unwrap_err()
        .contains("known-source map"));
    assert_eq!(serde_json::to_vec(&wrong.sources()).unwrap(), unchanged);
    ghost.check(observation, inputs, history).unwrap();
    results.push(json!({"control":"wrong ghost amount","restored":true,"ghost_unchanged":true}));
    macro_rules! bad {
        ($name:expr, $change:expr, $reason:expr) => {{
            let mut changed = observation.clone();
            $change(&mut changed);
            let mut state = ghost.clone();
            let before = serde_json::to_vec(&state.sources()).unwrap();
            let error = state.apply(&changed, inputs, history).unwrap_err();
            assert!(error.contains($reason), "{}: {error}", $name);
            assert_eq!(serde_json::to_vec(&state.sources()).unwrap(), before, "failed comparison changed ghost");
            state.check(observation, inputs, history).unwrap();
            results.push(json!({"control":$name,"error":error,"restored":true,"ghost_unchanged":true}));
        }};
    }
    let old_id = observation.before.outputs[0].row.id;
    bad!(
        "old amount",
        |o: &mut Observation| o
            .after
            .outputs
            .iter_mut()
            .find(|r| r.row.id == old_id)
            .unwrap()
            .row
            .output
            .amount += 1,
        "old persisted"
    );
    bad!(
        "old context",
        |o: &mut Observation| {
            match &mut o
                .after
                .outputs
                .iter_mut()
                .find(|r| r.row.id == old_id)
                .unwrap()
                .context
            {
                Context::Direct { height, .. } | Context::Lottery { height, .. } => *height += 1,
            }
        },
        "old persisted"
    );
    bad!(
        "old removal",
        |o: &mut Observation| {
            o.after.outputs.retain(|r| r.row.id != old_id);
        },
        "old persisted"
    );
    let new_id = observation.new_outputs[0].output.row.id;
    bad!(
        "missing new persisted row",
        |o: &mut Observation| {
            o.after.outputs.retain(|r| r.row.id != new_id);
        },
        "created-ID"
    );
    bad!(
        "extra persisted row",
        |o: &mut Observation| {
            let mut row = o.after.outputs[0].clone();
            row.row.id.tx_hash = [0xab; 32];
            o.after.outputs.push(row);
        },
        "created-ID"
    );
    bad!(
        "missing redundant new_outputs",
        |o: &mut Observation| {
            o.new_outputs.pop();
        },
        "created-ID"
    );
    bad!(
        "wrong redundant new_outputs",
        |o: &mut Observation| {
            o.new_outputs[0].output.row.output.amount += 1;
        },
        "correspondence"
    );
    bad!(
        "duplicate before ID",
        |o: &mut Observation| o.before.outputs.push(o.before.outputs[0].clone()),
        "duplicate"
    );
    bad!(
        "duplicate after ID",
        |o: &mut Observation| o.after.outputs.push(o.after.outputs[0].clone()),
        "duplicate"
    );
    bad!(
        "duplicate reported ID",
        |o: &mut Observation| o.new_outputs.push(o.new_outputs[0].clone()),
        "duplicate"
    );
    let old_key = history
        .iter()
        .position(|(_, height)| height.is_some())
        .unwrap();
    bad!(
        "wrong existing key marker",
        |o: &mut Observation| o.after.key_images[old_key].1 = None,
        "posthistory"
    );
    bad!(
        "wrong known prehistory",
        |o: &mut Observation| o.before.key_images[old_key].1 = None,
        "prehistory"
    );
    bad!(
        "missing known key observation",
        |o: &mut Observation| {
            o.after.key_images.pop();
        },
        "posthistory"
    );
    bad!(
        "duplicate known key observation",
        |o: &mut Observation| o.after.key_images.push(o.after.key_images[0]),
        "duplicate"
    );
    let key = observation.block.transactions[0].key_images()[0];
    bad!(
        "wrong new key height",
        |o: &mut Observation| {
            o.after
                .key_images
                .iter_mut()
                .find(|(k, _)| *k == key)
                .unwrap()
                .1 = Some(o.block.height() + 1);
        },
        "posthistory"
    );
    if let Some(payout) = observation
        .new_outputs
        .iter()
        .find(|r| matches!(r.kind, OutputKind::Lottery { .. }))
    {
        let id = payout.output.row.id;
        bad!(
            "wrong payout cumulative tweak",
            |o: &mut Observation| {
                for row in o.after.outputs.iter_mut().filter(|r| r.row.id == id) {
                    if let Context::Lottery { tweak, .. } = &mut row.context {
                        tweak[0] ^= 1;
                    }
                }
                for row in &mut o.new_outputs {
                    if row.output.row.id == id {
                        row.output = o
                            .after
                            .outputs
                            .iter()
                            .find(|r| r.row.id == id)
                            .unwrap()
                            .clone();
                    }
                }
            },
            "correspondence"
        );
    }
    results
}

fn measure_replay(
    name: &str,
    directory: &std::path::Path,
    sha: &str,
    saved: &Saved,
    capture_us: u128,
) -> Value {
    const REPEATS: usize = 3;
    let began = Instant::now();
    let owner = Wallet::from_mnemonic(WORDS).unwrap();
    let receiver = Wallet::from_mnemonic(RECEIVER).unwrap();
    let wallet_setup_us = began.elapsed().as_micros();
    // Exercise the actual loader, not seed-only regeneration or cached verdicts.
    assert!(Saved::load(directory, "bad hash")
        .unwrap_err()
        .contains("hash"));
    assert!(Saved::load(&directory.join("missing"), sha).is_err());
    let artifact_controls = if name == "signed_transfer_0" {
        let scratch = tempfile::tempdir().unwrap();
        supply::copy_closed(&directory.join("store"), &scratch.path().join("store")).unwrap();
        for field in ["schema", "profile", "source"] {
            let mut bad = saved.clone();
            match field {
                "schema" => bad.schema += 1,
                "profile" => bad.profile_hash = "wrong".into(),
                _ => bad.source_hash = "wrong".into(),
            }
            let bad_sha = bad.write(scratch.path()).unwrap();
            assert!(Saved::load(scratch.path(), &bad_sha)
                .unwrap_err()
                .contains("schema/source/profile"));
            let restored = saved.write(scratch.path()).unwrap();
            Saved::load(scratch.path(), &restored).unwrap();
        }
        let image = scratch.path().join("store/data.mdb");
        let bytes = std::fs::read(&image).unwrap();
        let mut corrupt = bytes.clone();
        corrupt[0] ^= 1;
        std::fs::write(&image, corrupt).unwrap();
        assert!(Saved::load(scratch.path(), sha)
            .unwrap_err()
            .contains("closed-store hash"));
        std::fs::write(&image, bytes).unwrap();
        Saved::load(scratch.path(), sha).unwrap();
        let manifest = scratch.path().join("manifest.json");
        std::fs::remove_file(&manifest).unwrap();
        std::fs::create_dir(&manifest).unwrap();
        assert!(Saved::load(scratch.path(), sha).is_err());
        std::fs::remove_dir(&manifest).unwrap();
        saved.write(scratch.path()).unwrap();
        Saved::load(scratch.path(), sha).unwrap();
        json!([
            "wrong manifest hash",
            "missing artifact",
            "wrong schema",
            "wrong profile",
            "wrong source",
            "corrupt store",
            "unreadable manifest",
            "restoration passes"
        ])
    } else {
        json!(["wrong manifest hash", "missing artifact"])
    };
    let mut samples = vec![];
    let identity = saved.identity().unwrap();
    for repetition in 0..REPEATS {
        // The fresh candidate between saved replays is deliberately different.
        for fresh in [false, true] {
            let start = Instant::now();
            let loaded = Saved::load(directory, sha).unwrap();
            let load_us = start.elapsed().as_micros();
            assert_eq!(loaded.identity().unwrap(), identity);
            let mut reset = loaded.reset(directory).unwrap();
            let reset_us = reset.reset_us;
            let usage = reset.ledger.storage_usage();
            let (candidate, signing_us, seal_us) = if fresh {
                let source = loaded
                    .pre
                    .outputs
                    .iter()
                    .find(|r| r.row.id == loaded.inputs[0].outpoint)
                    .unwrap();
                let to = if name == "real_maturity_payout" {
                    &owner
                } else {
                    &receiver
                };
                let (mut tx, input, signing_us) = signed(
                    &reset.ledger,
                    &owner,
                    to,
                    &source.row,
                    loaded.candidate.transactions[0].fee,
                );
                assert_eq!(input.outpoint, loaded.inputs[0].outpoint);
                if loaded.rejection.is_some() {
                    tx.outputs[0].amount += 1;
                }
                let seal_start = Instant::now();
                let block = reset
                    .ledger
                    .seal_block(template(&reset.ledger, &owner, vec![tx]))
                    .unwrap();
                assert_ne!(block.hash(), loaded.candidate.hash());
                (block, signing_us, seal_start.elapsed().as_micros())
            } else {
                (loaded.candidate.clone(), 0, 0)
            };
            let (observation, frame_us) = supply::execute(&loaded, &mut reset, candidate).unwrap();
            let serialization = Instant::now();
            let bytes = serde_json::to_vec(&json!({"saved_manifest_sha256":sha,"identity":identity,
                "signed_bytes_reused":!fresh,"real_inputs":loaded.inputs,"observation":observation})).unwrap();
            let artifact = format!(
                "sample-{repetition}-{}.json",
                if fresh { "fresh" } else { "saved" }
            );
            std::fs::write(directory.join(&artifact), &bytes).unwrap();
            let serialization_us = serialization.elapsed().as_micros();
            let cleanup = Instant::now();
            drop(reset);
            let cleanup_us = cleanup.elapsed().as_micros();
            let elapsed_us = start.elapsed().as_micros();
            let measured = load_us
                + reset_us
                + signing_us
                + seal_us
                + observation.precheck_us
                + observation.add_block_us
                + observation.observation_us
                + frame_us
                + serialization_us
                + cleanup_us;
            let sample = json!({"repetition":repetition,"fresh_signatures":usize::from(fresh),"signed_bytes_reused":!fresh,
                "identity":identity,"load_us":load_us,"reset_us":reset_us,"signing_us":signing_us,"template_seal_us":seal_us,
                "precheck_us":observation.precheck_us,"commit_us":observation.add_block_us,"observation_us":observation.observation_us,
                "frame_us":frame_us,"serialization_write_us":serialization_us,"cleanup_us":cleanup_us,
                "other_us":elapsed_us.checked_sub(measured).unwrap(),"elapsed_us":elapsed_us,
                "storage_used_bytes":usage.used_bytes,"storage_map_bytes":usage.map_size_bytes,
                "outcome":observation.outcome,"artifact":artifact,"artifact_bytes":bytes.len(),"sha256":supply::hash(&bytes)});
            samples.push(sample);
        }
    }
    assert_eq!(
        supply::hash(&std::fs::read(directory.join("store/data.mdb")).unwrap()),
        saved.data_sha256,
        "replay wrote the canonical saved image"
    );
    let distribution: Vec<_> = [false, true].into_iter().map(|fresh| {
        let mut times: Vec<_> = samples.iter().filter(|s| s["signed_bytes_reused"] == !fresh).map(|s| s["elapsed_us"].as_u64().unwrap()).collect();
        times.sort();
        json!({"fresh":fresh,"count":times.len(),"min_us":times[0],"median_us":times[times.len()/2],
            "max_us":times[times.len()-1],"sum_us":times.iter().sum::<u64>()})
    }).collect();
    json!({"scenario":name,"directory":directory.file_name().unwrap().to_str().unwrap(),"manifest_sha256":sha,
        "prestate_identity":identity,"closed_data_sha256":saved.data_sha256,"closed_data_bytes":saved.data_bytes,
        "pre_height":saved.pre.state.height,"pre_rows":saved.pre.outputs.len(),"known_keys":saved.history.len(),
        "capture_us":capture_us,"wallet_setup_us":wallet_setup_us,"measurement_elapsed_us":began.elapsed().as_micros(),
        "fresh_signed_samples":REPEATS,"saved_signed_replays":REPEATS,"artifact_controls":artifact_controls,
        "samples":samples,"distribution":distribution})
}

#[test]
fn supply_oracle_real_commits_rollback_and_reopen() {
    assert_eq!(oracle::lottery_maturity(), MATURITY);
    if let Some(path) = std::env::var_os("SUPPLY_ORACLE_REPLAY_DIR") {
        let sha = std::env::var("SUPPLY_ORACLE_REPLAY_SHA256")
            .expect("pin the manifest hash from the receipt");
        eprintln!(
            "SUPPLY_ORACLE_SAVED_REPLAY {}",
            supply::replay(&PathBuf::from(path), &sha)
                .expect("saved artifact replay must execute or fail")
        );
        return;
    }
    let mut f = Fixture::new();
    let start = Instant::now();
    let owner = Wallet::from_mnemonic(WORDS).unwrap();
    let receiver = Wallet::from_mnemonic(RECEIVER).unwrap();
    let wallet_setup_us = start.elapsed().as_micros();
    let (block, seal_us) = f.seal(&owner, vec![]);
    let empty = f.run("empty_no_candidates", block, vec![], 0, seal_us, None);
    assert_eq!(empty.block.lottery_summary.total_fees, 0);
    assert!(empty.block.lottery_outputs.is_empty());
    assert_eq!(empty.after.pool, empty.before.pool);
    f.advance(&owner, 64);

    let mut first_signed = None;
    for i in 0..3 {
        let (tx, input, signing_us) = signed(
            f.ledger(),
            &owner,
            &receiver,
            &f.coinbase(i + 1),
            PICOCREDITS_PER_CREDIT,
        );
        if i == 0 {
            first_signed = Some((tx.clone(), input.clone()));
        }
        let (block, seal_us) = f.seal(&owner, vec![tx]);
        let obs = f.run(
            &format!("signed_transfer_{i}"),
            block,
            vec![input],
            signing_us,
            seal_us,
            None,
        );
        assert!(obs.block.lottery_summary.total_fees > 0);
        assert!(obs.after.state.total_fees_burned > obs.before.state.total_fees_burned);
        assert!(obs
            .new_outputs
            .iter()
            .any(|o| matches!(o.kind, OutputKind::Ordinary { .. })));
        assert!(obs.block.lottery_outputs.is_empty());
    }
    let (block, seal_us) = f.seal(&owner, vec![]);
    let carry = f.run("empty_carryover", block, vec![], 0, seal_us, None);
    assert!(carry.before.pool > 0);
    assert!(carry.block.lottery_outputs.is_empty());
    assert_eq!(carry.block.lottery_summary.total_fees, 0);
    assert_eq!(carry.after.pool, carry.before.pool);
    let (replay, real) = first_signed.unwrap();
    let (block, seal_us) = f.seal(&owner, vec![replay]);
    f.run(
        "spent_key_image_replay",
        block,
        vec![real],
        0,
        seal_us,
        Some((true, "spent")),
    );

    let source = f.coinbase(10);
    let (tx, input, signing_us) = signed(
        f.ledger(),
        &owner,
        &receiver,
        &source,
        PICOCREDITS_PER_CREDIT,
    );
    f.ledger().verify_transaction(&tx).unwrap();
    let mut invalid = tx;
    invalid.outputs[0].amount += 1;
    assert!(invalid.verify_ring_signatures().is_err());
    // Re-seal the changed transaction so the V2 body/root really is valid.
    let (block, seal_us) = f.seal(&owner, vec![invalid]);
    f.run(
        "late_invalid_signature_balance",
        block,
        vec![input],
        signing_us,
        seal_us,
        Some((true, "Invalid ring signature")),
    );

    let (a, input, sign_a) = signed(
        f.ledger(),
        &owner,
        &receiver,
        &source,
        PICOCREDITS_PER_CREDIT,
    );
    let (b, _, sign_b) = signed(
        f.ledger(),
        &owner,
        &receiver,
        &source,
        2 * PICOCREDITS_PER_CREDIT,
    );
    assert_ne!(a.hash(), b.hash());
    assert_eq!(a.key_images(), b.key_images());
    f.ledger().verify_transaction(&a).unwrap();
    f.ledger().verify_transaction(&b).unwrap();
    let (block, seal_us) = f.seal(&owner, vec![a, b]);
    f.run(
        "late_same_block_duplicate",
        block,
        vec![input.clone(), input],
        sign_a + sign_b,
        seal_us,
        Some((true, "Key image already spent")),
    );

    f.advance(&owner, MATURITY);
    let source = f.coinbase(MATURITY - 20);
    let (fund, input, signing_us) = signed(
        f.ledger(),
        &owner,
        &owner,
        &source,
        10 * PICOCREDITS_PER_CREDIT,
    );
    let (paying, seal_us) = f.seal(&owner, vec![fund]);
    assert!(
        !paying.lottery_outputs.is_empty(),
        "first real-maturity draw must pay"
    );
    let mut forged = paying.clone();
    forged.lottery_outputs[0].payout += 1;
    f.run(
        "payout_binding_rejection",
        forged,
        vec![input.clone()],
        0,
        0,
        Some((false, "V2 payouts")),
    );
    let paid = f.run(
        "real_maturity_payout",
        paying,
        vec![input],
        signing_us,
        seal_us,
        None,
    );
    let payouts: Vec<_> = paid
        .new_outputs
        .iter()
        .filter(|o| matches!(o.kind, OutputKind::Lottery { .. }))
        .collect();
    assert_eq!(payouts.len(), paid.block.lottery_outputs.len());
    assert_eq!(
        payouts
            .iter()
            .map(|o| o.output.row.output.amount as u128)
            .sum::<u128>(),
        paid.block.lottery_summary.pool_distributed as u128
    );
    for output in &payouts {
        let OutputKind::Lottery { ordinal } = output.kind else {
            unreachable!()
        };
        let wire = &paid.block.lottery_outputs[ordinal as usize];
        assert_eq!(output.output.row.output.amount, wire.payout);
        assert_eq!(output.output.row.output.target_key, wire.target_key);
        let source = paid
            .before
            .outputs
            .iter()
            .find(|o| o.row.id.to_bytes() == wire.winner_utxo_id())
            .unwrap();
        assert_ne!(
            source.row.output.target_key,
            output.output.row.output.target_key
        );
        assert_eq!(
            source.row.output.public_key,
            output.output.row.output.public_key
        );
        assert_eq!(
            source.row.output.kem_ciphertext,
            output.output.row.output.kem_ciphertext
        );
        assert!(
            matches!(output.output.context, Context::Lottery { height:721, ordinal:i, base_index:0, .. } if i==ordinal)
        );
    }
    let award = payouts[0].output.row.clone();
    f.advance(&owner, MATURITY + 22);
    let (tx, input, signing_us) = signed(
        f.ledger(),
        &owner,
        &receiver,
        &award,
        award.output.amount / 2,
    );
    let (block, seal_us) = f.seal(&owner, vec![tx]);
    f.run(
        "real_payout_signed_spend",
        block,
        vec![input],
        signing_us,
        seal_us,
        None,
    );

    let chain = serde_json::to_vec(&json!({"profile":PROFILE,"difficulty":u64::MAX,
        "maturity":MATURITY,"genesis":Block::genesis(),"blocks":f.blocks}))
    .unwrap();
    std::fs::write(f.evidence.join("chain.json"), &chain).unwrap();
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(revision.status.success());
    let dirty = std::process::Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .unwrap();
    assert!(matches!(dirty.code(), Some(0 | 1)));
    let receipt = json!({"schema":1,"profile":PROFILE,"canonical_schedule_reachable":false,
        "source_revision":String::from_utf8(revision.stdout).unwrap().trim(),
        "source_worktree_dirty":!dirty.success(),
        "source_hash":oracle::source_hash(),"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "fixture_source_hash":fixture_source_hash(),"selected_tests":2,
        "wallet_setup_us":wallet_setup_us,"cold_chain_setup_us":f.setup_us,
        "total_ms":f.began.elapsed().as_millis(),"committed_chain_blocks":f.blocks.len(),
        "accepted_scenarios":f.cases.iter().filter(|c| c["accepted"]==true).count(),
        "rejected_scenarios":f.cases.iter().filter(|c| c["accepted"]==false).count(),
        "infrastructure_failures":0,"negative_control":"pool_after+1 rejected; restored observation accepted",
        "chain_sha256":hex::encode(Sha256::digest(&chain)),"cases":f.cases});
    let extension = json!({"schema":1,"profile":PROFILE,"profile_hash":supply::profile_hash(),
        "source_hash":oracle::source_hash(),"fixture_source_hash":fixture_source_hash(),
        "source_revision":receipt["source_revision"],"source_worktree_dirty":receipt["source_worktree_dirty"],
        "platform":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "frame_controls":f.frame_controls,"replay_cases":f.replay_cases,"total_ms":f.began.elapsed().as_millis(),
        "nightly_allowance_us":3600,"lean_codec_model_us":"unmeasured; positive additional reserve required",
        "million_case_feasible":false,"registration":"blocked pending faithful complete-budget evidence"});
    std::fs::write(
        f.evidence.join("frame-replay-receipt.json"),
        serde_json::to_vec_pretty(&extension).unwrap(),
    )
    .unwrap();
    eprintln!("SUPPLY_ORACLE_FRAME_REPLAY_RECEIPT {extension}");
    std::fs::write(
        f.evidence.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    eprintln!(
        "SUPPLY_ORACLE_RECEIPT {}",
        serde_json::to_string(&receipt).unwrap()
    );
    eprintln!("SUPPLY_ORACLE_ARTIFACTS {}", f.evidence.display());
    assert_eq!(receipt["accepted_scenarios"], 7);
    assert_eq!(receipt["rejected_scenarios"], 4);
    f.bound();
}

#[test]
fn supply_oracle_infrastructure_failure_is_not_rejection() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let error = match Ledger::open(file.path()) {
        Ok(_) => panic!("a file cannot serve as a ledger directory"),
        Err(error) => error,
    };
    assert!(
        !error.is_block_fault(),
        "setup failure must not be consensus coverage"
    );
    eprintln!(
        "SUPPLY_ORACLE_INFRA_CONTROL expected_local_failure=1 accepted=0 rejected=0 error={error}"
    );
}
