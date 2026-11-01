//! Exercise the real node loop, block application, RPC connections and runtime
//! shutdown. Only delivery of an already-externalized decision is injected;
//! these tests do not assert that SCP would externalize the invalid fixture.

use super::*;
use crate::{
    block::{calculate_block_reward, difficulty::EmissionController, Block},
    consensus::ConsensusValue,
    ledger::Ledger,
};
use bth_transaction_types::constants::Network;
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{atomic::AtomicU64, mpsc, Mutex},
    time::Instant,
};

pub(super) struct FailureTestHooks {
    decisions: mpsc::Receiver<Vec<Option<MintingTx>>>,
    queued: std::collections::VecDeque<Option<MintingTx>>,
    stopped: mpsc::Sender<Stopped>,
    initial_slot: Option<u64>,
    delivered: usize,
    cached_hash: Option<[u8; 32]>,
    /// Slept inside every off-loop apply job, before the commit (#1565).
    pub(super) apply_delay: Option<Duration>,
    /// Incremented on every `consensus_tick` arm (#1565).
    ticks: Arc<AtomicU64>,
    /// One sample per consensus tick: the SCP slot and whether an apply job
    /// was queued or in flight at that tick (#1565, #1566).
    samples: Arc<Mutex<Vec<TickSample>>>,
    /// Network blocks the tick arm feeds to the production enqueue
    /// functions (#1566).
    network_blocks: mpsc::Receiver<InjectedBlocks>,
    /// One sample per non-consensus apply completion (#1566).
    completions: Arc<Mutex<Vec<CompletionSample>>>,
    /// Incremented on every parked-decision `error!` (#1583).
    parked_warnings: Arc<AtomicU64>,
    /// Overrides the consensus config the loop builds, so a test can drive
    /// the real solo propose path with no slot wait (#1550).
    pub(super) consensus_config: Option<crate::consensus::ConsensusConfig>,
    /// Local mint candidates the tick arm feeds to `submit_minting_tx`, as
    /// the miner's work channel does (#1550).
    candidates: mpsc::Receiver<MintingTx>,
    /// One sample per submitted candidate (#1550).
    submissions: Arc<Mutex<Vec<SubmissionSample>>>,
}

/// A local mint candidate submitted by the tick arm (#1550).
#[derive(Debug, Clone, Copy)]
struct SubmissionSample {
    /// An apply job was queued or in flight at submission.
    busy: bool,
    /// The SCP slot at submission.
    slot: u64,
    /// The candidate was accepted into the consensus tx cache.
    cached: bool,
}

/// Blocks injected as if they had arrived from the network (#1566).
pub(super) enum InjectedBlocks {
    /// A `NetworkEvent::NewBlock`.
    Gossip(Block),
    /// A `SyncResponse::Blocks` batch the sync manager handed back.
    Sync(libp2p::PeerId, Vec<Block>),
}

#[derive(Debug, Clone, Copy)]
struct TickSample {
    slot: u64,
    /// An apply job was queued or in flight.
    in_flight: bool,
    /// Values queued in the consensus service for the next proposal.
    pending: usize,
    at: Instant,
}

/// Which completion hooks a non-consensus apply job ran (#1566).
#[derive(Debug, Clone, Copy)]
struct CompletionSample {
    source: &'static str,
    committed: usize,
    failed: bool,
    /// Sync only: `on_blocks_added` (+ chain state, #419 reconcile) ran.
    blocks_added: bool,
    /// Sync only: #644 `on_zero_progress` ran.
    zero_progress: bool,
    at: Instant,
}

#[derive(Debug)]
struct Stopped {
    initial_slot: u64,
    final_slot: u64,
    ledger_height: u64,
    delivered: usize,
    queued_remaining: usize,
    cache_retained: bool,
}

impl FailureTestHooks {
    pub(super) fn next_event(
        &mut self,
        consensus: &mut ConsensusService,
    ) -> Option<ConsensusEvent> {
        if self.queued.is_empty() {
            self.queued.extend(self.decisions.try_recv().ok()?);
        }
        let minting = self.queued.pop_front()?;
        let slot = consensus.current_slot();
        self.initial_slot.get_or_insert(slot);
        self.delivered += 1;
        let values = match minting {
            Some(minting) => {
                let hash = minting.hash();
                consensus.register_minting_tx(hash, bincode::serialize(&minting).unwrap());
                self.cached_hash = Some(hash);
                vec![ConsensusValue::from_minting_tx(hash, 0)]
            }
            None => vec![], // Real BlockBuilder rejects a decision without a coinbase.
        };
        Some(ConsensusEvent::SlotExternalized {
            slot_index: slot,
            values,
        })
    }

    pub(super) fn on_tick(&mut self, consensus: &ConsensusService, in_flight: bool) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        self.samples.lock().unwrap().push(TickSample {
            slot: consensus.current_slot(),
            in_flight,
            pending: consensus.pending_count(),
            at: Instant::now(),
        });
    }

    pub(super) fn submit_candidates(&mut self, consensus: &mut ConsensusService, busy: bool) {
        for minting in self.candidates.try_iter() {
            let hash = minting.hash();
            consensus.submit_minting_tx(
                hash,
                minting.pow_priority(),
                bincode::serialize(&minting).unwrap(),
            );
            self.submissions.lock().unwrap().push(SubmissionSample {
                busy,
                slot: consensus.current_slot(),
                cached: consensus.get_tx_entry(&hash).is_some(),
            });
        }
    }

    pub(super) fn on_parked_warning(&mut self) {
        self.parked_warnings.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn take_network_blocks(&mut self) -> Vec<InjectedBlocks> {
        self.network_blocks.try_iter().collect()
    }

    pub(super) fn on_apply_completion(
        &mut self,
        source: &'static str,
        committed: usize,
        failed: bool,
        blocks_added: bool,
        zero_progress: bool,
    ) {
        self.completions.lock().unwrap().push(CompletionSample {
            source,
            committed,
            failed,
            blocks_added,
            zero_progress,
            at: Instant::now(),
        });
    }

    pub(super) fn observe_stop(self, consensus: &ConsensusService, node: &Node) {
        let _ = self.stopped.send(Stopped {
            initial_slot: self.initial_slot.unwrap_or_default(),
            final_slot: consensus.current_slot(),
            ledger_height: node
                .shared_ledger()
                .read()
                .unwrap()
                .get_chain_state()
                .unwrap()
                .height,
            delivered: self.delivered,
            queued_remaining: self.queued.len(),
            cache_retained: self
                .cached_hash
                .is_none_or(|hash| consensus.get_tx_entry(&hash).is_some()),
        });
    }
}

struct StopOnDrop(Arc<AtomicBool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn connect_rpc(addr: SocketAddr) -> TcpStream {
    let start = Instant::now();
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
            Ok(socket) => {
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                return socket;
            }
            Err(error) => {
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "RPC did not start: {error}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Read one response without closing the connection or accidentally consuming
/// bytes from a later response. A successful RPC proves the detached handler
/// exists before the injected failure.
fn establish_rpc_keep_alive(socket: &mut TcpStream) {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"node_getStatus","params":{}}"#;
    write!(socket, "POST /rpc HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnection: keep-alive\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        headers.push(byte[0]);
        assert!(headers.len() < 8192);
    }
    let headers = String::from_utf8(headers).unwrap();
    assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
    assert!(!headers.to_ascii_lowercase().contains("connection: close"));
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .expect("RPC response must include its body length");
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert!(response.get("result").is_some(), "{response}");
}

fn assert_rpc_closed(socket: &mut TcpStream) {
    let mut byte = [0];
    match socket.read(&mut byte) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("existing RPC connection remained open after failure: {result:?}"),
    }
}

/// A running node-loop fixture: the real `run_async_with_shutdown` on its own
/// runtime thread, with only decision delivery injected.
struct Harness {
    dir: tempfile::TempDir,
    data_dir: std::path::PathBuf,
    config_path: std::path::PathBuf,
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    _stop_on_panic: StopOnDrop,
    decisions: mpsc::Sender<Vec<Option<MintingTx>>>,
    stopped: mpsc::Receiver<Stopped>,
    finished: mpsc::Receiver<std::result::Result<(), String>>,
    before_persist: mpsc::Receiver<()>,
    permit_persist: mpsc::Sender<()>,
    ticks: Arc<AtomicU64>,
    samples: Arc<Mutex<Vec<TickSample>>>,
    network_blocks: mpsc::Sender<InjectedBlocks>,
    completions: Arc<Mutex<Vec<CompletionSample>>>,
    parked_warnings: Arc<AtomicU64>,
    candidates: mpsc::Sender<MintingTx>,
    submissions: Arc<Mutex<Vec<SubmissionSample>>>,
    worker: std::thread::JoinHandle<()>,
}

fn spawn_harness(apply_delay: Option<Duration>) -> Harness {
    spawn_harness_with(apply_delay, false)
}

/// Seed a testnet ledger at `path` whose chain difficulty is trivial, so a
/// handful of RandomX hashes finds a valid block (as `map_full_tests` does).
fn seed_trivial_pow_ledger(path: &std::path::Path) -> Ledger {
    let ledger = Ledger::open(path).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    ledger
}

/// A chain of `count` valid blocks on top of a trivial-PoW genesis, built
/// by the production commit function on a scratch ledger, so emission and
/// difficulty evolve exactly as they will on the node under test.
fn valid_chain(scratch: &std::path::Path, count: u64) -> Vec<Block> {
    valid_chain_minted_by(scratch, count, "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about")
}

/// [`valid_chain`] minted by `mnemonic`'s wallet: a different mnemonic gives
/// a competing fork on the same genesis (#1583).
fn valid_chain_minted_by(scratch: &std::path::Path, count: u64, mnemonic: &str) -> Vec<Block> {
    let ledger = seed_trivial_pow_ledger(scratch);
    let state = ledger.get_chain_state().unwrap();
    let mut controller = EmissionController::from_chain_state(
        state.difficulty,
        state.total_mined,
        state.total_fees_burned,
        state.total_tx,
        state.epoch_tx,
        state.epoch_emission,
        state.epoch_burns,
        state.current_reward,
    );
    let ledger: SharedLedger = Arc::new(RwLock::new(ledger));
    let wallet = Wallet::from_mnemonic(mnemonic).unwrap();
    let minter = wallet.default_address();
    let mut blocks = Vec::new();
    for _ in 0..count {
        let state = ledger.read().unwrap().get_chain_state().unwrap();
        let mut block = Block::new_template_with_txs(
            &Block::genesis(),
            &minter,
            state.difficulty,
            calculate_block_reward(state.height + 1, state.total_mined),
            vec![],
        );
        block.header.height = state.height + 1;
        block.header.prev_block_hash = state.tip_hash;
        block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
        block.minting_tx.block_height = block.header.height;
        block.minting_tx.prev_block_hash = state.tip_hash;
        block.minting_tx.timestamp = block.header.timestamp;
        while !block.header.is_valid_pow() {
            block.header.nonce += 1;
        }
        block.minting_tx.nonce = block.header.nonce;
        // Protocol 7.0.0 (#1640): seal the V2 body root like the producer.
        let block = ledger.read().unwrap().seal_block(block).unwrap();
        commit_block_locked(&ledger, &mut controller, &block).unwrap();
        blocks.push(block);
    }
    blocks
}

fn spawn_harness_with(apply_delay: Option<Duration>, trivial_pow: bool) -> Harness {
    spawn_harness_full(apply_delay, trivial_pow, None)
}

fn spawn_harness_full(
    apply_delay: Option<Duration>,
    trivial_pow: bool,
    consensus_config: Option<crate::consensus::ConsensusConfig>,
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("node");
    fs::create_dir(&data_dir).unwrap();
    let config_path = data_dir.join("config.toml");
    if trivial_pow {
        drop(seed_trivial_pow_ledger(
            &crate::config::ledger_db_path_from_config(&config_path),
        ));
    }
    // Reserve an ephemeral RPC port; gossip also uses an ephemeral port. The
    // only configured bootstrap destination is loopback port zero, so neither
    // DNS nor public fallback seeds are contacted by this private fixture.
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = reservation.local_addr().unwrap();
    let mut config = Config::new_relay(Network::Testnet);
    config.network.gossip_port = Some(0);
    config.network.rpc_port = Some(addr.port());
    config.network.metrics_port = Some(0);
    config.network.dns_seeds.enabled = false;
    config.network.bootstrap_peers = vec!["/ip4/127.0.0.1/tcp/0".into()];
    config.network.quorum.mode = QuorumMode::Explicit;
    config.network.quorum.threshold = 1;
    config.network.quorum.members.clear();
    config.save(&config_path).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let _stop_on_panic = StopOnDrop(shutdown.clone());
    let worker_shutdown = shutdown.clone();
    let worker_config_path = config_path.clone();
    let (decisions, decision_rx) = mpsc::channel();
    let (stopped_tx, stopped) = mpsc::channel();
    let (finished_tx, finished) = mpsc::channel();
    let (before_persist_tx, before_persist) = mpsc::channel();
    let (permit_persist, permit_persist_rx) = mpsc::channel();
    let ticks = Arc::new(AtomicU64::new(0));
    let samples = Arc::new(Mutex::new(Vec::new()));
    let (worker_ticks, worker_samples) = (ticks.clone(), samples.clone());
    let (network_blocks, network_blocks_rx) = mpsc::channel();
    let completions = Arc::new(Mutex::new(Vec::new()));
    let worker_completions = completions.clone();
    let parked_warnings = Arc::new(AtomicU64::new(0));
    let worker_parked_warnings = parked_warnings.clone();
    let worker_apply_delay = apply_delay;
    let (candidates, candidates_rx) = mpsc::channel();
    let submissions = Arc::new(Mutex::new(Vec::new()));
    let worker_submissions = submissions.clone();
    drop(reservation);
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(30),
                run_async_with_shutdown(
                    config,
                    &worker_config_path,
                    false,
                    TestnetMintWindow::new(Network::Testnet, None, 0, Instant::now()).unwrap(),
                    worker_shutdown,
                    Some(FailureTestHooks {
                        decisions: decision_rx,
                        queued: Default::default(),
                        stopped: stopped_tx,
                        initial_slot: None,
                        delivered: 0,
                        cached_hash: None,
                        apply_delay: worker_apply_delay,
                        ticks: worker_ticks,
                        samples: worker_samples,
                        network_blocks: network_blocks_rx,
                        completions: worker_completions,
                        parked_warnings: worker_parked_warnings,
                        consensus_config,
                        candidates: candidates_rx,
                        submissions: worker_submissions,
                    }),
                ),
            )
            .await
            .context("injected node failure exceeded deadline")?
        });
        // This is the exact function run() uses, including full runtime drop
        // before waiting on an undurable failure. Do not duplicate it here.
        let result = finish_runtime(rt, result, || {
            // Block at the persistence boundary with the existing keep-alive
            // connection still held by the test. It must already be closed.
            before_persist_tx.send(()).unwrap();
            permit_persist_rx
                .recv_timeout(Duration::from_secs(15))
                .unwrap();
        });
        finished_tx
            .send(result.map_err(|error| error.to_string()))
            .unwrap();
    });
    Harness {
        dir,
        data_dir,
        config_path,
        addr,
        shutdown,
        _stop_on_panic,
        decisions,
        stopped,
        finished,
        before_persist,
        permit_persist,
        ticks,
        samples,
        network_blocks,
        completions,
        parked_warnings,
        candidates,
        submissions,
        worker,
    }
}

fn exercise_failure(application_failure: bool, persistence_failure: bool) {
    exercise_failure_with_delay(application_failure, persistence_failure, None);
}

fn exercise_failure_with_delay(
    application_failure: bool,
    persistence_failure: bool,
    apply_delay: Option<Duration>,
) {
    let Harness {
        dir,
        data_dir,
        config_path,
        addr,
        shutdown,
        _stop_on_panic,
        decisions,
        stopped,
        finished,
        before_persist,
        permit_persist,
        worker,
        ..
    } = spawn_harness(apply_delay);
    let mut socket = connect_rpc(addr);
    establish_rpc_keep_alive(&mut socket);
    if persistence_failure {
        // Existing LMDB handles remain valid on Unix, but the original marker
        // parent no longer exists. This forces a real create failure without
        // depending on chmod behavior under root or replacing the writer.
        fs::rename(&data_dir, dir.path().join("moved-node")).unwrap();
    }
    let decision = if application_failure {
        let wallet = Wallet::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();
        // A new-height block that the real builder constructs but the real
        // ledger rejects: genesis is height zero, and this height-one block
        // names a parent that is not genesis (and has no valid PoW). It is
        // at tip + 1, so it runs and fails (#1583: a decision ABOVE tip + 1
        // would be parked, not failed).
        Some(MintingTx::new(
            1,
            0,
            &wallet.default_address(),
            [0; 32],
            u64::MAX,
            1,
        ))
    } else {
        None
    };
    // Deliver both atomically so the second decision is definitely queued
    // before the first is handled, even if the worker runs immediately.
    decisions.send(vec![decision, None]).unwrap();
    let stopped = stopped.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        stopped.delivered, 1,
        "processed a queued event after failure"
    );
    assert_eq!(
        stopped.final_slot, stopped.initial_slot,
        "advanced a rejected decision"
    );
    assert_eq!(stopped.queued_remaining, 1);
    assert_eq!(stopped.ledger_height, 0);
    assert!(
        stopped.cache_retained,
        "cleared the failed decision's cache"
    );
    before_persist
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(!consensus_failure::marker_path(&config_path).exists());
    assert_rpc_closed(&mut socket);
    permit_persist.send(()).unwrap();
    if persistence_failure {
        assert!(!consensus_failure::marker_path(&config_path).exists());
        assert!(
            matches!(
                finished.recv_timeout(Duration::from_millis(150)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "undurable failure exited instead of remaining parked after RPC closed"
        );
        shutdown.store(true, Ordering::SeqCst);
        assert!(finished
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .is_ok());
    } else {
        let error = finished
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap_err();
        assert!(
            error.contains("Consensus stopped after externalized-block failure"),
            "{error}"
        );
        let marker = consensus_failure::marker_path(&config_path);
        let evidence: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
        assert_eq!(evidence["ledger_height"], 0);
        assert_eq!(evidence["externalized_slot"], stopped.initial_slot);
        let reason = evidence["reason"].as_str().unwrap();
        if application_failure {
            assert!(reason.contains("application failed"), "{reason}");
            assert!(!evidence["rejected_block"].is_null());
            assert!(evidence["cached_values"][0]["raw_transaction_hex"]
                .as_str()
                .is_some());
        } else {
            assert!(reason.contains("construction failed"), "{reason}");
            assert!(evidence["rejected_block"].is_null());
        }
        // Exercise production restart refusal, before any socket/signal setup.
        let error = run(&config_path, false, None, None, None).unwrap_err();
        assert!(error.to_string().contains("Consensus failure marker"));
    }
    worker.join().unwrap();
}

#[test]
fn externalized_application_failure_stops_loop_closes_rpc_and_refuses_restart() {
    exercise_failure(true, false);
}

#[test]
fn externalized_build_failure_stops_loop_closes_rpc_and_refuses_restart() {
    exercise_failure(false, false);
}

#[cfg(unix)]
#[test]
fn undurable_externalized_failure_closes_existing_rpc_before_parking() {
    exercise_failure(true, true);
}

/// Long enough for several 500 ms consensus ticks to run during the apply.
const APPLY_DELAY: Duration = Duration::from_secs(3);

/// #1565: the externalized apply runs off the node loop. During a 3 s apply
/// the loop keeps ticking, the slot does not advance, and once the apply
/// completes the slot advances exactly once.
///
/// The decision is a height-0 coinbase: the real builder constructs it, the
/// real ledger rejects it (genesis already fills height 0), and the #421
/// classification `height_already_applied(0, Some(0))` treats it as a benign
/// duplicate. That drives the success-side completion path (`advance_slot`)
/// without a PoW fixture. The harness is solo, so only `advance_slot` moves
/// the slot.
#[test]
fn externalized_apply_runs_off_loop_and_advances_slot_once_after_completion() {
    let harness = spawn_harness(Some(APPLY_DELAY));
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);
    let wallet = Wallet::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();
    let decision = MintingTx::new(0, 0, &wallet.default_address(), [0; 32], u64::MAX, 1);
    harness.decisions.send(vec![Some(decision)]).unwrap();

    // Wait until a tick observes the apply finished (in flight, then not).
    let deadline = Instant::now() + Duration::from_secs(20);
    let samples = loop {
        let samples = harness.samples.lock().unwrap().clone();
        let finished = samples
            .iter()
            .position(|sample| sample.in_flight)
            .is_some_and(|first| samples[first..].iter().any(|sample| !sample.in_flight));
        if finished {
            break samples;
        }
        assert!(
            Instant::now() < deadline,
            "apply never completed: {samples:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    harness.shutdown.store(true, Ordering::SeqCst);
    let stopped = harness
        .stopped
        .recv_timeout(Duration::from_secs(10))
        .unwrap();

    let first = samples.iter().position(|sample| sample.in_flight).unwrap();
    let after = first
        + samples[first..]
            .iter()
            .position(|sample| !sample.in_flight)
            .unwrap();
    let during = &samples[first..after];
    assert!(
        during.len() >= 4,
        "node loop stopped ticking during the apply: {samples:?}"
    );
    // The ticks must be spread across the apply in real time. A loop that
    // blocked on the apply would instead replay its missed ticks in a burst
    // (tokio's default `MissedTickBehavior::Burst`) once the apply returned.
    let span = during.last().unwrap().at - during[0].at;
    assert!(
        span >= APPLY_DELAY / 2,
        "in-flight ticks were not spread over the apply ({span:?}): {samples:?}"
    );
    assert!(
        during
            .iter()
            .all(|sample| sample.slot == stopped.initial_slot),
        "slot advanced before the apply completed: {samples:?}"
    );
    assert!(
        samples[after..]
            .iter()
            .all(|sample| sample.slot == stopped.initial_slot + 1),
        "slot must advance exactly once after completion: {samples:?}"
    );
    assert!(harness.ticks.load(Ordering::SeqCst) >= during.len() as u64);
    assert_eq!(stopped.final_slot, stopped.initial_slot + 1);
    assert_eq!(stopped.delivered, 1);
    assert_eq!(stopped.queued_remaining, 0);
    assert_eq!(stopped.ledger_height, 0);

    harness
        .before_persist
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    harness.permit_persist.send(()).unwrap();
    assert!(harness
        .finished
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .is_ok());
    assert!(!consensus_failure::marker_path(&harness.config_path).exists());
    harness.worker.join().unwrap();
}

/// #1565: a delayed off-loop apply that fails terminally takes the unchanged
/// terminal path: no slot advance, the queued decision is never delivered,
/// evidence is written and restart is refused.
#[test]
fn delayed_externalized_application_failure_stops_loop_unchanged() {
    exercise_failure_with_delay(true, false, Some(APPLY_DELAY));
}

// ---- #1566: every apply path runs off the node loop ----

/// Wait until the tick samples show an apply job become busy and then idle
/// again, and return them.
fn wait_busy_then_idle(harness: &Harness) -> Vec<TickSample> {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let samples = harness.samples.lock().unwrap().clone();
        let finished = samples
            .iter()
            .position(|sample| sample.in_flight)
            .is_some_and(|first| samples[first..].iter().any(|sample| !sample.in_flight));
        if finished {
            return samples;
        }
        assert!(
            Instant::now() < deadline,
            "apply never completed: {samples:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until at least one tick saw an apply job queued or in flight.
fn wait_busy(harness: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !harness
        .samples
        .lock()
        .unwrap()
        .iter()
        .any(|sample| sample.in_flight)
    {
        assert!(Instant::now() < deadline, "apply never started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The ticks sampled while an apply was busy. Asserts the loop kept ticking
/// across the injected delay in real time. A count alone is not enough: a
/// loop blocked on the apply replays its missed ticks in a burst (tokio's
/// default `MissedTickBehavior::Burst`) once the apply returns, so the span
/// is what proves liveness.
fn assert_ticks_spread_over(samples: &[TickSample], delay: Duration) -> (usize, usize) {
    let first = samples.iter().position(|sample| sample.in_flight).unwrap();
    let after = first
        + samples[first..]
            .iter()
            .position(|sample| !sample.in_flight)
            .unwrap();
    let during = &samples[first..after];
    assert!(
        during.len() >= 4,
        "node loop stopped ticking during the apply: {samples:?}"
    );
    let span = during.last().unwrap().at - during[0].at;
    assert!(
        span >= delay / 2,
        "busy ticks were not spread over the apply ({span:?}): {samples:?}"
    );
    (first, after)
}

/// Shut the harness down cleanly and return what the loop saw at stop.
fn stop_cleanly(harness: Harness) -> Stopped {
    harness.shutdown.store(true, Ordering::SeqCst);
    let stopped = harness
        .stopped
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    harness
        .before_persist
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    harness.permit_persist.send(()).unwrap();
    assert!(harness
        .finished
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .is_ok());
    assert!(
        !consensus_failure::marker_path(&harness.config_path).exists(),
        "a non-consensus apply must never write terminal evidence"
    );
    harness.worker.join().unwrap();
    stopped
}

/// #1566 loop liveness, gossip path: a gossiped block is fed through the
/// production `enqueue_network_block` (the `NewBlock` arm's entry point) and
/// applied off the loop. During a 3 s apply the loop keeps ticking, spread
/// over the delay. The block has no valid parent or PoW, so the commit
/// fails, and that failure is not terminal.
#[test]
fn gossip_apply_runs_off_loop_and_failure_is_non_terminal() {
    let harness = spawn_harness(Some(APPLY_DELAY));
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);
    // Contiguous with the genesis tip, so it is queued, not dropped.
    let mut block = Block::genesis();
    block.header.height = 1;
    block.minting_tx.block_height = 1;
    let sent = Instant::now();
    harness
        .network_blocks
        .send(InjectedBlocks::Gossip(block))
        .unwrap();

    let samples = wait_busy_then_idle(&harness);
    assert_ticks_spread_over(&samples, APPLY_DELAY);
    let completions = harness.completions.lock().unwrap().clone();
    assert_eq!(completions.len(), 1, "{completions:?}");
    let completion = completions[0];
    assert_eq!(completion.source, "gossip");
    assert_eq!(completion.committed, 0);
    assert!(completion.failed, "the invalid block must fail to commit");
    assert!(completion.at - sent >= APPLY_DELAY);
    // The loop survived the failure and still serves the existing connection.
    establish_rpc_keep_alive(&mut socket);

    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.ledger_height, 0);
    assert_eq!(stopped.delivered, 0);
}

/// #1566 loop liveness, sync path: one multi-block sync batch is fed through
/// the production `enqueue_sync_batch` (the `SyncResponse::Blocks` arm's
/// entry point) and applied as ONE off-loop job. The loop keeps ticking
/// across the delay; the batch's completion hooks run exactly once, after
/// the job completes, and all blocks commit. Re-serving the same batch is a
/// pure overlap: it commits nothing and drives #644 `on_zero_progress`
/// once, not `on_failure`.
#[test]
fn sync_batch_apply_runs_off_loop_and_hooks_run_once_after_completion() {
    let scratch = tempfile::tempdir().unwrap();
    let blocks = valid_chain(scratch.path(), 3);
    let harness = spawn_harness_with(Some(APPLY_DELAY), true);
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);
    let peer = libp2p::PeerId::random();
    let sent = Instant::now();
    harness
        .network_blocks
        .send(InjectedBlocks::Sync(peer, blocks.clone()))
        .unwrap();

    let samples = wait_busy_then_idle(&harness);
    assert_ticks_spread_over(&samples, APPLY_DELAY);
    let completions = harness.completions.lock().unwrap().clone();
    assert_eq!(
        completions.len(),
        1,
        "batch hooks must run exactly once: {completions:?}"
    );
    let completion = completions[0];
    assert_eq!(completion.source, "sync");
    assert_eq!(completion.committed, 3);
    assert!(!completion.failed);
    assert!(completion.blocks_added, "on_blocks_added must run");
    assert!(!completion.zero_progress);
    assert!(
        completion.at - sent >= APPLY_DELAY,
        "hooks ran before the job completed"
    );

    // Pure overlap (#641/#644).
    let resent = harness.samples.lock().unwrap().len();
    harness
        .network_blocks
        .send(InjectedBlocks::Sync(peer, blocks))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while harness.completions.lock().unwrap().len() < 2 {
        assert!(Instant::now() < deadline, "overlap batch never completed");
        std::thread::sleep(Duration::from_millis(50));
    }
    let completions = harness.completions.lock().unwrap().clone();
    assert_eq!(completions.len(), 2, "{completions:?}");
    let overlap = completions[1];
    assert_eq!(overlap.source, "sync");
    assert_eq!(overlap.committed, 0);
    assert!(
        !overlap.failed,
        "an overlap must not be reported as a failure"
    );
    assert!(!overlap.blocks_added);
    assert!(
        overlap.zero_progress,
        "an overlap must drive on_zero_progress"
    );
    assert!(harness.samples.lock().unwrap().len() > resent);

    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.ledger_height, 3);
}

/// #1566 / #421: our own decision for height 1, queued behind a gossiped
/// block for the same height, is NOT dropped at enqueue. It runs after the
/// gossip commit, is classified a benign duplicate by
/// `height_already_applied`, and `advance_slot` runs exactly once, after
/// both jobs completed in order.
#[test]
fn consensus_job_behind_same_height_gossip_is_benign_duplicate_and_advances_once() {
    let scratch = tempfile::tempdir().unwrap();
    let block = valid_chain(scratch.path(), 1).remove(0);
    let harness = spawn_harness_with(Some(APPLY_DELAY), true);
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);
    harness
        .network_blocks
        .send(InjectedBlocks::Gossip(block))
        .unwrap();
    wait_busy(&harness);
    let wallet = Wallet::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();
    let decision = MintingTx::new(1, 0, &wallet.default_address(), [0; 32], u64::MAX, 1);
    harness.decisions.send(vec![Some(decision)]).unwrap();

    let samples = wait_busy_then_idle(&harness);
    let (first, after) = assert_ticks_spread_over(&samples, APPLY_DELAY * 2);
    let completions = harness.completions.lock().unwrap().clone();
    assert_eq!(completions.len(), 1, "{completions:?}");
    assert_eq!(completions[0].source, "gossip");
    assert_eq!(
        completions[0].committed, 1,
        "the gossiped block commits first"
    );
    let slot_before = samples[first].slot;
    assert!(
        samples[first..after]
            .iter()
            .all(|sample| sample.slot == slot_before),
        "slot advanced before the consensus job completed: {samples:?}"
    );
    assert!(
        samples[after..]
            .iter()
            .all(|sample| sample.slot == slot_before + 1),
        "slot must advance exactly once, after completion: {samples:?}"
    );

    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.delivered, 1);
    assert_eq!(stopped.initial_slot, slot_before);
    assert_eq!(stopped.final_slot, stopped.initial_slot + 1);
    assert_eq!(stopped.ledger_height, 1);
}

// ---- #1583: a decision above tip + 1 is parked, not terminal ----

const MNEMONIC_A: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const MNEMONIC_B: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// Wait until a tick satisfies `pred`, and return the samples seen so far.
fn wait_for_sample(
    harness: &Harness,
    what: &str,
    pred: impl Fn(&TickSample) -> bool,
) -> Vec<TickSample> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let samples = harness.samples.lock().unwrap().clone();
        if samples.iter().any(&pred) {
            return samples;
        }
        assert!(Instant::now() < deadline, "{what}: {samples:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Deliver our own decision for `block` (its minting tx), wait until the
/// loop holds it, and check it stays parked: the loop keeps ticking, the
/// slot does not advance, nothing is applied and nothing stops. Returns the
/// slot the decision was delivered at.
fn deliver_and_assert_parked(harness: &Harness, block: &Block, hold: Duration) -> u64 {
    harness
        .decisions
        .send(vec![Some(block.minting_tx.clone())])
        .unwrap();
    let samples = wait_for_sample(harness, "decision never held", |s| s.in_flight);
    let first = samples.iter().position(|s| s.in_flight).unwrap();
    let slot = samples[first].slot;
    let ticks_before = harness.ticks.load(Ordering::SeqCst);
    std::thread::sleep(hold);
    let samples = harness.samples.lock().unwrap().clone();
    assert!(
        samples[first..]
            .iter()
            .all(|sample| sample.in_flight && sample.slot == slot),
        "a parked decision must hold the slot and stay pending: {samples:?}"
    );
    assert!(
        harness.ticks.load(Ordering::SeqCst) >= ticks_before + 2,
        "the node loop stopped ticking while a decision was parked"
    );
    assert!(
        harness.completions.lock().unwrap().is_empty(),
        "nothing may apply while only the parked decision is pending"
    );
    assert!(
        matches!(harness.finished.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "a parked decision stopped the node"
    );
    assert!(!consensus_failure::marker_path(&harness.config_path).exists());
    slot
}

/// Externalize at tip + 2 with the parent missing: no terminal failure, the
/// decision parks, the gap block (sync batch or gossip) is applied, then
/// the parked decision commits and `advance_slot` runs exactly once.
fn exercise_parked_fill(fill: impl FnOnce(Block) -> InjectedBlocks) {
    let scratch = tempfile::tempdir().unwrap();
    let chain = valid_chain_minted_by(scratch.path(), 2, MNEMONIC_A);
    let harness = spawn_harness_with(None, true);
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);

    let slot = deliver_and_assert_parked(&harness, &chain[1], Duration::from_millis(1500));

    // The gap block is NOT dropped as a duplicate of the parked decision.
    harness.network_blocks.send(fill(chain[0].clone())).unwrap();
    let samples = wait_for_sample(&harness, "slot never advanced after the fill", |s| {
        s.slot != slot
    });
    let completions = harness.completions.lock().unwrap().clone();
    assert_eq!(completions.len(), 1, "{completions:?}");
    assert_eq!(completions[0].committed, 1, "the gap block commits");
    assert!(
        samples
            .iter()
            .filter(|sample| sample.slot != slot)
            .all(|sample| sample.slot == slot + 1 && sample.at >= completions[0].at),
        "the slot must advance exactly once, after the gap fill: {samples:?}"
    );
    // The loop survived and still serves the existing connection.
    establish_rpc_keep_alive(&mut socket);

    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.delivered, 1);
    assert_eq!(stopped.initial_slot, slot);
    assert_eq!(stopped.final_slot, slot + 1, "advance_slot ran once");
    assert_eq!(stopped.ledger_height, 2, "the parked decision committed");
}

#[test]
fn decision_at_tip_plus_two_parks_then_applies_after_sync_fills_the_parent() {
    exercise_parked_fill(|parent| InjectedBlocks::Sync(libp2p::PeerId::random(), vec![parent]));
}

/// The #1593 review case: a gossiped gap block below a parked decision is
/// enqueued, not dropped as a duplicate.
#[test]
fn gap_gossip_block_below_a_parked_decision_is_applied_not_dropped() {
    exercise_parked_fill(InjectedBlocks::Gossip);
}

/// A sync batch that already contains H completes the parked decision as a
/// #421 benign duplicate, and `advance_slot` runs exactly once.
#[test]
fn sync_batch_containing_the_parked_height_completes_it_as_a_benign_duplicate() {
    let scratch = tempfile::tempdir().unwrap();
    let chain = valid_chain_minted_by(scratch.path(), 2, MNEMONIC_A);
    let harness = spawn_harness_with(None, true);
    let slot = deliver_and_assert_parked(&harness, &chain[1], Duration::from_millis(1000));
    harness
        .network_blocks
        .send(InjectedBlocks::Sync(libp2p::PeerId::random(), chain))
        .unwrap();
    wait_for_sample(&harness, "slot never advanced after the batch", |s| {
        s.slot != slot
    });
    let completions = harness.completions.lock().unwrap().clone();
    assert_eq!(completions.len(), 1, "{completions:?}");
    assert_eq!(completions[0].committed, 2);
    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.final_slot, slot + 1);
    assert_eq!(stopped.ledger_height, 2);
}

/// Same setup, but sync delivers a DIFFERENT tip + 1 (a fork) from the
/// decided block's parent. The parked decision dispatches, the V2 producer
/// (`seal_block`, #1640) refuses it with the same "Previous block hash
/// mismatch" wording `add_block` uses (#420), and the failure
/// is terminal with today's evidence and restart refusal.
#[test]
fn parked_decision_with_mismatched_synced_parent_is_terminal() {
    let scratch_a = tempfile::tempdir().unwrap();
    let scratch_b = tempfile::tempdir().unwrap();
    let chain_a = valid_chain_minted_by(scratch_a.path(), 2, MNEMONIC_A);
    let fork_b = valid_chain_minted_by(scratch_b.path(), 1, MNEMONIC_B);
    assert_ne!(chain_a[0].hash(), fork_b[0].hash(), "fixture must fork");
    let harness = spawn_harness_with(None, true);
    let slot = deliver_and_assert_parked(&harness, &chain_a[1], Duration::from_millis(1000));
    harness
        .network_blocks
        .send(InjectedBlocks::Sync(libp2p::PeerId::random(), fork_b))
        .unwrap();

    let stopped = harness
        .stopped
        .recv_timeout(Duration::from_secs(15))
        .unwrap();
    assert_eq!(stopped.final_slot, slot, "advanced a rejected decision");
    assert_eq!(stopped.ledger_height, 1, "the forked parent committed");
    harness
        .before_persist
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    harness.permit_persist.send(()).unwrap();
    let error = harness
        .finished
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert!(
        error.contains("Consensus stopped after externalized-block failure"),
        "{error}"
    );
    let marker = consensus_failure::marker_path(&harness.config_path);
    let evidence: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    assert_eq!(evidence["ledger_height"], 1);
    assert_eq!(evidence["externalized_slot"], slot);
    let reason = evidence["reason"].as_str().unwrap();
    assert!(reason.contains("application failed"), "{reason}");
    assert!(reason.contains("Previous block hash mismatch"), "{reason}");
    assert!(!evidence["rejected_block"].is_null());
    let error = run(&harness.config_path, false, None, None, None).unwrap_err();
    assert!(error.to_string().contains("Consensus failure marker"));
    harness.worker.join().unwrap();
}

/// No sync source (the harness is solo): the decision stays parked past
/// `PARKED_DECISION_WARN_AFTER`, the loop keeps ticking, the slot never
/// advances, the `error!` fires once per interval, and a clean shutdown
/// writes no evidence.
#[test]
fn parked_decision_with_no_sync_source_stays_bounded_and_non_terminal() {
    let harness = spawn_harness(None);
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);
    let wallet = Wallet::from_mnemonic(MNEMONIC_A).unwrap();
    let mut decided = Block::genesis();
    decided.minting_tx = MintingTx::new(2, 0, &wallet.default_address(), [7; 32], u64::MAX, 1);
    let hold = PARKED_DECISION_WARN_AFTER * 3 + PARKED_DECISION_WARN_AFTER / 2;
    let slot = deliver_and_assert_parked(&harness, &decided, hold);
    let warnings = harness.parked_warnings.load(Ordering::SeqCst);
    assert!(
        (3..=4).contains(&warnings),
        "expected one parked warning per {PARKED_DECISION_WARN_AFTER:?} over {hold:?}, got {warnings}"
    );
    establish_rpc_keep_alive(&mut socket);

    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.delivered, 1);
    assert_eq!(stopped.final_slot, slot, "advance_slot ran while parked");
    assert_eq!(stopped.ledger_height, 0);
}

/// #1577: setting the shutdown flag (what the SIGINT/SIGTERM listener does)
/// while an externalized apply is in flight drains that apply before the loop
/// tears down, then exits cleanly.
///
/// Asserts only on observable harness outputs (`Stopped`, `finished`, wall
/// time), not on how the loop tracks the in-flight job, so it survives the
/// #1566 apply-queue rewrite. The tick samples are used only to pick a moment
/// when the apply has demonstrably started.
#[test]
fn shutdown_during_in_flight_apply_drains_before_teardown() {
    let harness = spawn_harness(Some(APPLY_DELAY));
    let mut socket = connect_rpc(harness.addr);
    establish_rpc_keep_alive(&mut socket);
    let wallet = Wallet::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();
    // Height-0 coinbase: a benign duplicate (#421), so the drained apply
    // completes on the non-terminal path (see the #1565 test above).
    let decision = MintingTx::new(0, 0, &wallet.default_address(), [0; 32], u64::MAX, 1);
    // The apply job (and its APPLY_DELAY sleep) cannot start before this.
    let sent_at = Instant::now();
    harness.decisions.send(vec![Some(decision)]).unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !harness
        .samples
        .lock()
        .unwrap()
        .iter()
        .any(|sample| sample.in_flight)
    {
        assert!(Instant::now() < deadline, "apply never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let shutdown_at = Instant::now();
    let apply_done_no_earlier_than = sent_at + APPLY_DELAY;
    assert!(
        shutdown_at + Duration::from_millis(500) < apply_done_no_earlier_than,
        "fixture too slow: shutdown would not land during the apply"
    );
    harness.shutdown.store(true, Ordering::SeqCst);

    let stopped = harness
        .stopped
        .recv_timeout(Duration::from_secs(10))
        .expect("loop did not stop after shutdown");
    let stopped_at = Instant::now();
    // i.e. Stopped came no earlier than the apply delay remaining at shutdown.
    assert!(
        stopped_at >= apply_done_no_earlier_than,
        "loop tore down {:?} before the in-flight apply could finish",
        apply_done_no_earlier_than - stopped_at
    );
    assert_eq!(stopped.delivered, 1);
    assert_eq!(stopped.queued_remaining, 0);
    assert_eq!(stopped.ledger_height, 0);

    harness
        .before_persist
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert_rpc_closed(&mut socket);
    harness.permit_persist.send(()).unwrap();
    assert!(harness
        .finished
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .is_ok());
    assert!(!consensus_failure::marker_path(&harness.config_path).exists());
    harness.worker.join().unwrap();
}

// ---- #1550: no proposal on the stale tip while a real decision applies ----

/// #1550: the other tests inject `SlotExternalized` directly, so the service
/// never holds `externalized` and the propose gate is not exercised through
/// the real loop. Here the decision comes from the real solo propose path
/// (`submit_minting_tx` -> `tick` -> direct externalize), so the loop holds
/// the service's own pending decision while a delayed apply runs off the
/// loop.
///
/// While that apply is in flight the loop keeps ticking, and a competing
/// mint for the old tip (what the local miner produces in that gap) is
/// neither cached nor queued: nothing can be proposed or externalized for
/// the next slot on the stale tip. The slot advances exactly once, after the
/// apply. A positive control then shows the same submission path does
/// propose and apply once the decision is applied.
#[test]
fn real_decision_apply_in_flight_blocks_stale_tip_proposals() {
    let scratch_a = tempfile::tempdir().unwrap();
    let scratch_b = tempfile::tempdir().unwrap();
    let chain = valid_chain_minted_by(scratch_a.path(), 2, MNEMONIC_A);
    let competitor = valid_chain_minted_by(scratch_b.path(), 1, MNEMONIC_B);
    assert_eq!(competitor[0].height(), chain[0].height());
    assert_ne!(competitor[0].minting_tx.hash(), chain[0].minting_tx.hash());
    let harness = spawn_harness_full(
        Some(APPLY_DELAY),
        true,
        Some(crate::consensus::ConsensusConfig::fixed_timing(0)),
    );

    // The first mint is proposed, solo-externalized and applied off the loop.
    harness
        .candidates
        .send(chain[0].minting_tx.clone())
        .unwrap();
    let samples = wait_for_sample(&harness, "the decision never started applying", |s| {
        s.in_flight
    });
    let slot = samples.iter().find(|s| s.in_flight).unwrap().slot;

    // The local miner's work on the old tip arrives during the apply.
    harness
        .candidates
        .send(competitor[0].minting_tx.clone())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let submissions = loop {
        let submissions = harness.submissions.lock().unwrap().clone();
        if submissions.len() == 2 {
            break submissions;
        }
        assert!(Instant::now() < deadline, "candidate never submitted");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(submissions[0].cached, "{submissions:?}");
    assert!(
        submissions[1].busy && submissions[1].slot == slot,
        "precondition: the stale candidate arrived during the apply: {submissions:?}"
    );
    assert!(
        !submissions[1].cached,
        "a stale-tip mint was accepted while the decision awaits application"
    );

    let samples = wait_busy_then_idle(&harness);
    let (first, after) = assert_ticks_spread_over(&samples, APPLY_DELAY);
    assert!(
        samples[first..after]
            .iter()
            .all(|sample| sample.slot == slot && sample.pending == 0),
        "the slot advanced or a value was queued during the apply: {samples:?}"
    );
    assert!(
        samples[after..]
            .iter()
            .all(|sample| sample.slot == slot + 1 && sample.pending == 0),
        "the slot must advance exactly once, after the apply: {samples:?}"
    );

    // Positive control: on the applied tip the same path proposes again.
    harness
        .candidates
        .send(chain[1].minting_tx.clone())
        .unwrap();
    wait_for_sample(&harness, "the next mint was never applied", |s| {
        s.slot == slot + 2
    });
    let stopped = stop_cleanly(harness);
    assert_eq!(stopped.delivered, 0, "no decision was injected");
    assert_eq!(stopped.final_slot, slot + 2);
    // Block 2 extends block 1, so the tip is the first decision's block and
    // never the competitor's.
    assert_eq!(stopped.ledger_height, 2);
}
