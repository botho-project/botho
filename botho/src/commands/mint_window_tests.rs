//! Bounded, serial resource regression for the real node loop (#1958).
//! The fixture's funded LMDB rows are synthetic, not an accepted-chain witness.
//! Clock/scan inputs are injected only in the second attempt; minting, stop,
//! bookkeeping, status publication, wallet scanning and shutdown stay real.

use super::*;
use crate::{
    ledger::{ChainState, Ledger, UtxoSnapshot},
    rpc::{
        metrics::{MINTING_ACTIVE, NODE_LOOP_DELAYED},
        websocket::WsEvent,
    },
    transaction::{TxOutput, Utxo, UtxoId},
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio::sync::{broadcast, mpsc as async_mpsc};

const HIGH_BALANCE: u64 = 20_000_000_000_000_000;
const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ATTEMPT_LIMIT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(super) struct Step {
    pub clock: Option<(Option<u64>, Instant)>,
    pub fail_scan: bool,
    pub balance: Option<u64>,
    pub pending: Option<usize>,
    /// Slept inside the off-loop balance scan before it reads the ledger.
    pub scan_delay: Option<Duration>,
    synced: Option<bool>,
    quorum: Option<bool>,
}

pub(super) struct Hooks {
    control: Option<async_mpsc::UnboundedReceiver<Step>>,
    events: mpsc::Sender<Event>,
    websocket: Option<broadcast::Receiver<WsEvent>>,
    started: Instant,
}

#[derive(Serialize)]
struct Event {
    stage: &'static str,
    wall: Option<u64>,
    elapsed_ms: u128,
    enabled: bool,
    paused: bool,
    rpc: bool,
    metric: i64,
    worker_active: bool,
    hashes: u64,
    completed_hashes: u64,
    #[serde(skip)]
    health: crate::node::minter::MinterHealth,
    websocket: Vec<bool>,
    scan: Option<(u64, usize)>,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap())
    }
}

impl Hooks {
    pub(super) fn controlled(&self) -> bool {
        self.control.is_some()
    }

    pub(super) fn attach(&mut self, broadcaster: &WsBroadcaster) {
        self.websocket = Some(broadcaster.subscribe());
    }

    pub(super) async fn step(&mut self, config: &mut Config, synced: &mut bool) -> Result<Step> {
        let Some(control) = &mut self.control else {
            return Ok(Step::default());
        };
        // Waiting here deliberately delays the actual balance arm. The trace
        // distinguishes tick entry from its later expiry/scan execution.
        let step = tokio::time::timeout(ATTEMPT_LIMIT, control.recv())
            .await?
            .context("mint-window test controller disconnected")?;
        if let Some(value) = step.synced {
            *synced = value;
        }
        if let Some(reachable) = step.quorum {
            config.network.quorum.mode = QuorumMode::Recommended;
            config.network.quorum.min_peers = if reachable { 0 } else { 1 };
        }
        println!(
            "MINT_WINDOW_STEP {}",
            serde_json::json!({
                "elapsed_ms": self.started.elapsed().as_millis(),
                "wall_override": step.clock.map(|(wall, _)| wall),
                "monotonic_override_ms": step.clock.map(|(_, clock)| clock.saturating_duration_since(self.started).as_millis()),
                "fail_scan": step.fail_scan, "balance_override": step.balance,
                "pending_override": step.pending, "synced": step.synced, "quorum": step.quorum,
            })
        );
        Ok(step)
    }

    pub(super) fn observe(
        &mut self,
        stage: &'static str,
        enabled: bool,
        paused: bool,
        state: &RpcState,
        scan: Option<(u64, usize)>,
    ) {
        let health = state.minter_health.as_ref().unwrap().read().unwrap();
        let health = health.as_ref().unwrap().clone();
        let snapshot = health.snapshot();
        let mut websocket = Vec::new();
        while let Ok(event) = self.websocket.as_mut().unwrap().try_recv() {
            if let WsEvent::MintingStatus { active, .. } = event {
                websocket.push(active);
            }
        }
        let event = Event {
            stage,
            wall: unix_seconds(),
            elapsed_ms: self.started.elapsed().as_millis(),
            enabled,
            paused,
            rpc: *state.minting_active.read().unwrap(),
            metric: MINTING_ACTIVE.get(),
            worker_active: snapshot.active,
            hashes: snapshot.total_hashes,
            completed_hashes: health.completed_hashes.load(Ordering::Relaxed),
            health,
            websocket,
            scan,
        };
        println!(
            "MINT_WINDOW_EVENT {}",
            serde_json::to_string(&event).unwrap()
        );
        let _ = self.events.send(event);
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    control: async_mpsc::UnboundedSender<Step>,
    events: mpsc::Receiver<Event>,
    finished: mpsc::Receiver<std::result::Result<(), String>>,
    worker: Option<std::thread::JoinHandle<()>>,
    started: Instant,
    wall: u64,
    until: u64,
    expires: Instant,
}

impl Harness {
    fn spawn(controlled: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let wallet = Wallet::from_mnemonic(MNEMONIC).unwrap();
        let ledger =
            Ledger::open(&crate::config::ledger_db_path_from_config(&config_path)).unwrap();
        let output = Utxo {
            id: UtxoId::new([0x58; 32], 0),
            output: TxOutput::new(HIGH_BALANCE, &wallet.default_address()),
            created_at: 0,
        };
        // Near-impossible PoW prevents an incidental block from changing this
        // fixture's balance. The real worker must nevertheless produce hashes.
        let state = ChainState {
            difficulty: 1,
            ..ledger.get_chain_state().unwrap()
        };
        // Preserve the actual genesis hash: a zero tip makes Ledger::open
        // initialize genesis again and reset the fixture's hard difficulty.
        let snapshot =
            UtxoSnapshot::new(0, state.tip_hash, state, vec![output], vec![], vec![]).unwrap();
        ledger.seed_fixture_snapshot_unvalidated(&snapshot).unwrap();
        assert_eq!(
            ledger
                .unspent_wallet_outputs(&wallet)
                .unwrap()
                .iter()
                .map(|o| o.utxo.output.amount)
                .sum::<u64>(),
            HIGH_BALANCE
        );
        drop(ledger);
        // Complete schema/reopen validation during fixture setup, before the
        // two runtime attempts begin. A synthetic non-genesis V1 snapshot
        // lacks the persisted MTP window and cannot be reopened by a node.
        let reopened =
            Ledger::open(&crate::config::ledger_db_path_from_config(&config_path)).unwrap();
        assert_eq!(reopened.get_chain_state().unwrap().difficulty, 1);
        drop(reopened);
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = reservation.local_addr().unwrap();
        let mut config = Config::new(MNEMONIC.into(), Network::Testnet);
        config.minting.threads = 1;
        config.network.listen_address = Some("127.0.0.1".parse().unwrap());
        config.network.gossip_port = Some(0);
        config.network.rpc_port = Some(addr.port());
        config.network.metrics_port = Some(0);
        config.network.dns_seeds.enabled = false;
        config.network.fallback_seeds = false;
        config.network.bootstrap_peers.clear();
        config.network.quorum.mode = QuorumMode::Explicit;
        config.network.quorum.threshold = 1;
        config.network.quorum.members.clear();
        config.save(&config_path).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown.clone();
        let (control, controls) = async_mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        let started = Instant::now();
        let wall = unix_seconds().unwrap();
        let until = wall + if controlled { 90 } else { 60 };
        let window = TestnetMintWindow::new(Network::Testnet, Some(until), wall, started).unwrap();
        let expires = window.deadline.unwrap().1;
        println!(
            "MINT_WINDOW_ATTEMPT {}",
            serde_json::json!({
                "controlled": controlled, "pid": std::process::id(), "restarts": 0,
                "wall_start": wall, "deadline": until, "limit_seconds": ATTEMPT_LIMIT.as_secs(),
                "entrypoint": "run_async_with_shutdown", "mint": true, "threads": 1,
                "rpc": addr.to_string(), "balance": HIGH_BALANCE, "mempool": 0,
            })
        );
        drop(reservation);
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let result = runtime.block_on(async {
                tokio::time::timeout(
                    ATTEMPT_LIMIT,
                    run_async_with_shutdown(
                        config,
                        &config_path,
                        true,
                        window,
                        worker_shutdown,
                        None,
                        Some(Hooks {
                            control: controlled.then_some(controls),
                            events: event_tx,
                            websocket: None,
                            started,
                        }),
                    ),
                )
                .await
                .context("mint-window attempt exceeded 120 seconds")?
            });
            // Dropping Node inside the real loop joins all minting workers;
            // runtime drop also finishes detached RPC/metrics tasks.
            drop(runtime);
            let _ = finished_tx.send(result.map_err(|e| e.to_string()));
        });
        Self {
            _dir: dir,
            addr,
            shutdown,
            control,
            events,
            finished,
            worker: Some(worker),
            started,
            wall,
            until,
            expires,
        }
    }

    fn next(&self, stage: &str) -> Event {
        loop {
            let event = self
                .events
                .recv_timeout(self.remaining())
                .expect("missing runtime observation");
            if event.stage == stage {
                return event;
            }
        }
    }

    fn remaining(&self) -> Duration {
        ATTEMPT_LIMIT
            .checked_sub(self.started.elapsed())
            .expect("attempt exceeded 120 seconds")
    }

    fn step(&self, step: Step, stage: &str) -> Event {
        self.control.send(step).unwrap();
        self.next(stage)
    }

    fn rpc(&self, method: &str) -> serde_json::Value {
        let connect_started = Instant::now();
        let mut socket = loop {
            match TcpStream::connect_timeout(&self.addr, Duration::from_millis(100)) {
                Ok(socket) => break socket,
                Err(error) => {
                    assert!(
                        connect_started.elapsed() < Duration::from_secs(5),
                        "RPC startup: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let body =
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":{}}).to_string();
        write!(socket, "POST /rpc HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        let response: serde_json::Value = serde_json::from_str(body).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn wait_for_hashing(&self, previous: &Event) {
        loop {
            let completed = previous.health.completed_hashes.load(Ordering::Relaxed);
            if completed > previous.completed_hashes {
                let status = self.rpc("minting_getStatus");
                assert_eq!(status["active"], true);
                println!(
                    "MINT_WINDOW_RPC {}",
                    serde_json::json!({"status":status,"completed_hashes_test_only":completed})
                );
                return;
            }
            assert!(
                self.started.elapsed() < Duration::from_secs(105),
                "worker never hashed"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn finish(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.control.send(Step::default());
        self.finished
            .recv_timeout(self.remaining())
            .expect("node/worker join timed out")
            .unwrap();
        self.worker.take().unwrap().join().unwrap();
        println!(
            "MINT_WINDOW_JOIN {}",
            serde_json::json!({"elapsed_ms":self.started.elapsed().as_millis(),"joined":true})
        );
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.control.send(Step::default());
        if let Some(worker) = self.worker.take() {
            // Also clean up on assertion failure, within this attempt's cap.
            let remaining = ATTEMPT_LIMIT.saturating_sub(self.started.elapsed());
            match self.finished.recv_timeout(remaining) {
                Ok(result) => {
                    eprintln!("MINT_WINDOW_CLEANUP {result:?}");
                    let _ = worker.join();
                }
                Err(error) => eprintln!("MINT_WINDOW_CLEANUP_INCOMPLETE {error}; outer process deadline remains authoritative"),
            }
        }
    }
}

fn assert_stopped(event: &Event) {
    assert!(
        !event.enabled && event.paused && !event.rpc && !event.worker_active,
        "{event:?}"
    );
    assert_eq!(event.metric, 0, "{event:?}");
}

fn assert_started(event: &Event) {
    assert!(
        event.enabled && !event.paused && event.rpc && event.worker_active,
        "{event:?}"
    );
    assert_eq!(event.metric, 1, "{event:?}");
}

/// This process-global metrics/RandomX resource test must run alone. CI invokes
/// this exact ignored selector after the ordinary library suite, in a fresh
/// process using the same compiled binary, with an outer 300-second deadline.
#[test]
#[ignore = "serial RandomX resource regression; run explicitly with --ignored --exact --test-threads=1"]
fn mint_window_runtime_regression() {
    baseline_attempt();
    controlled_attempt();
}

/// Narrow local probe; the combined selector above is the CI gate.
#[test]
#[ignore = "serial RandomX controlled probe; covered by the combined CI selector"]
fn mint_window_controlled_probe() {
    controlled_attempt();
}

fn baseline_attempt() {
    // Attempt 1: unmodified 10-second interval and real wall/monotonic clocks.
    let baseline = Harness::spawn(false);
    let first = baseline.next("tick");
    assert_started(&first);
    assert!(first.websocket.contains(&true));
    assert_started(&baseline.next("after_expiry"));
    baseline.wait_for_hashing(&first);
    assert_eq!(baseline.rpc("wallet_getBalance")["confirmed"], HIGH_BALANCE);
    let stopped = loop {
        let event = baseline.next("after_expiry");
        if !event.enabled {
            break event;
        }
        assert!(!event.paused);
    };
    assert_stopped(&stopped);
    assert!(stopped.websocket.contains(&false));
    assert!(stopped.completed_hashes > 0);
    assert!(stopped.wall.unwrap() >= baseline.until);
    let scan = baseline.next("scanned");
    assert_eq!(scan.scan, Some((HIGH_BALANCE, 0)));
    assert_stopped(&scan);
    assert_eq!(baseline.rpc("minting_getStatus")["active"], false);
    let stable = baseline.next("settled");
    assert_stopped(&stable);
    // Allow an in-flight hash to drain, then require stability across a full
    // additional ten-second interval. The final join is separate evidence.
    let drained = baseline.next("settled");
    assert_stopped(&drained);
    let stable = baseline.next("settled");
    assert_stopped(&stable);
    assert_eq!(drained.completed_hashes, stable.completed_hashes);
    baseline.finish();
}

fn controlled_attempt() {
    // Attempt 2: drive clock rollback plus monotonic expiry and scan failure,
    // then use the same production policy/resume path with explicit inputs.
    let controlled = Harness::spawn(true);
    let first_tick = controlled.next("tick");
    assert_started(&first_tick);
    controlled.wait_for_hashing(&first_tick);
    std::thread::sleep(Duration::from_millis(250));
    let running = controlled.step(Step::default(), "after_expiry");
    let delay_ms = running.elapsed_ms - first_tick.elapsed_ms;
    assert!(delay_ms >= 250);
    println!(
        "MINT_WINDOW_DELAY {}",
        serde_json::json!({"delay_ms":delay_ms,"timely_expiry_claim":false})
    );
    assert_started(&running);
    assert!(running.completed_hashes > 0);
    // Missing a scan here proves the continuous override still skips it.
    assert_eq!(
        controlled
            .events
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .stage,
        "tick"
    );
    let stopped = controlled.step(
        Step {
            clock: Some((Some(controlled.wall - 1), controlled.expires)),
            fail_scan: true,
            ..Step::default()
        },
        "after_expiry",
    );
    assert_stopped(&stopped);
    assert!(stopped.websocket.contains(&false));
    assert_stopped(&controlled.next("scan_failed"));
    assert_eq!(controlled.rpc("minting_getStatus")["active"], false);
    // #1984: a balance scan slower than the loop-delay threshold runs off
    // the loop. Before, the loop sat inside it: no swarm polling, so a
    // producer that had lost quorum could never rejoin.
    let delayed_before = NODE_LOOP_DELAYED.get();
    let scan_started = Instant::now();
    controlled
        .control
        .send(Step {
            scan_delay: Some(Duration::from_secs(6)),
            ..Step::default()
        })
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let rpc_started = Instant::now();
    assert_eq!(controlled.rpc("minting_getStatus")["active"], false);
    assert!(rpc_started.elapsed() < Duration::from_secs(1));
    let slow = controlled.next("settled");
    assert!(scan_started.elapsed() >= Duration::from_secs(6));
    assert_stopped(&slow);
    assert_eq!(
        NODE_LOOP_DELAYED.get(),
        delayed_before,
        "balance scan blocked the node loop"
    );
    assert_stopped(&controlled.step(Step::default(), "settled"));
    assert_stopped(&controlled.step(Step::default(), "settled"));
    // Both gates remain effective despite pending work or low balance.
    assert_stopped(&controlled.step(
        Step {
            balance: Some(0),
            synced: Some(false),
            ..Step::default()
        },
        "settled",
    ));
    assert_stopped(&controlled.step(
        Step {
            pending: Some(1),
            synced: Some(true),
            quorum: Some(false),
            ..Step::default()
        },
        "settled",
    ));
    // A stop is asynchronous. A legitimate resume may need a later balance
    // tick until the previous worker's in-flight hash and join finish.
    let resume_deadline = Instant::now() + Duration::from_secs(5);
    let resumed = loop {
        let event = controlled.step(
            Step {
                pending: Some(1),
                quorum: Some(true),
                ..Step::default()
            },
            "settled",
        );
        if event.enabled {
            break event;
        }
        assert_stopped(&event);
        assert!(
            Instant::now() < resume_deadline,
            "worker join prevented resume"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_started(&resumed);
    assert!(resumed.websocket.contains(&true));
    controlled.wait_for_hashing(&resumed);
    assert_stopped(&controlled.step(Step::default(), "settled"));
    controlled.finish();
    // One restart keeps this attempt below its 120-second resource budget.
    // Successful low-balance resume and peer/sync event reentry still have
    // predicate coverage only; this test does not claim those runtime paths.
}
