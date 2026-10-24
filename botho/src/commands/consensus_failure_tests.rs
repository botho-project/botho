//! Exercise the real node loop, block application, RPC connections and runtime
//! shutdown. Only delivery of an already-externalized decision is injected;
//! these tests do not assert that SCP would externalize the invalid fixture.

use super::*;
use crate::consensus::ConsensusValue;
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
    /// Slept inside the off-loop apply job, before the commit (#1565).
    pub(super) apply_delay: Option<Duration>,
    /// Incremented on every `consensus_tick` arm (#1565).
    ticks: Arc<AtomicU64>,
    /// One sample per consensus tick: the SCP slot and whether an
    /// externalized apply was in flight at that tick (#1565).
    samples: Arc<Mutex<Vec<TickSample>>>,
}

#[derive(Debug, Clone, Copy)]
struct TickSample {
    slot: u64,
    in_flight: bool,
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
    worker: std::thread::JoinHandle<()>,
}

fn spawn_harness(apply_delay: Option<Duration>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("node");
    fs::create_dir(&data_dir).unwrap();
    let config_path = data_dir.join("config.toml");
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
    let worker_apply_delay = apply_delay;
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
        // ledger rejects: genesis is height zero, so height two cannot apply.
        Some(MintingTx::new(
            2,
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
