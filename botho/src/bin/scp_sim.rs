// Copyright (c) 2024 Botho Foundation

//! SCP Consensus Simulation Tool
//!
//! This tool simulates a network of SCP nodes to measure consensus performance.
//!
//! Usage:
//!   cargo run --bin scp_sim -- --nodes 3 --txs 1000
//!   cargo run --bin scp_sim -- --nodes 5 --txs 5000 --rate 10000
//!   cargo run --bin scp_sim -- bench --runs 5 --nodes 3,5,7
//!   cargo run --bin scp_sim -- --output results.json

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use crossbeam_channel::{unbounded, Receiver, Sender};
use dashmap::DashMap;
use bt_common::NodeID;
use bt_consensus_scp::{
    msg::Msg,
    slot::{CombineFn, ValidityFn},
    test_utils::test_node_id,
    Node, QuorumSet, ScpNode, SlotIndex,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(name = "scp_sim")]
#[command(about = "SCP Consensus Simulation Tool with Benchmarking")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Number of nodes (2-10)
    #[arg(short, long, default_value = "3", global = true)]
    nodes: usize,

    /// Number of transactions to process
    #[arg(short, long, default_value = "1000", global = true)]
    txs: usize,

    /// Submission rate (txs per second)
    #[arg(short, long, default_value = "5000", global = true)]
    rate: u64,

    /// Max transactions per slot
    #[arg(long, default_value = "100", global = true)]
    max_per_slot: usize,

    /// SCP timebase in milliseconds
    #[arg(long, default_value = "500", global = true)]
    timebase_ms: u64,

    /// Quorum threshold (k value, defaults to BFT optimal)
    #[arg(short, long, global = true)]
    quorum_k: Option<usize>,

    /// Output file for results (JSON format)
    #[arg(short, long, global = true)]
    output: Option<String>,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Quiet mode (minimal output)
    #[arg(short = 'Q', long, global = true)]
    quiet: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a single simulation (default)
    Run,

    /// Run benchmark with multiple iterations
    Bench {
        /// Number of benchmark runs
        #[arg(long, default_value = "5")]
        runs: usize,

        /// Comma-separated list of node counts to test (e.g., "3,5,7")
        #[arg(long)]
        node_counts: Option<String>,

        /// Warmup runs before measurement
        #[arg(long, default_value = "1")]
        warmup: usize,
    },

    /// Compare different configurations
    Compare {
        /// Comma-separated list of node counts (e.g., "3,5,7")
        #[arg(long, default_value = "3,5")]
        node_counts: String,

        /// Comma-separated list of k thresholds (e.g., "2,3")
        #[arg(long)]
        k_values: Option<String>,
    },
}

// ============================================================================
// Performance Metrics
// ============================================================================

#[derive(Default)]
struct Metrics {
    messages_sent: AtomicU64,
    messages_received: AtomicU64,
    slots_externalized: AtomicU64,
    values_externalized: AtomicU64,
    start_time: Mutex<Option<Instant>>,
    slot_latencies_us: Mutex<Vec<u64>>,
}

impl Metrics {
    fn new() -> Self {
        Self {
            messages_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            slots_externalized: AtomicU64::new(0),
            values_externalized: AtomicU64::new(0),
            start_time: Mutex::new(None),
            slot_latencies_us: Mutex::new(Vec::new()),
        }
    }

    fn start(&self) {
        *self.start_time.lock().unwrap() = Some(Instant::now());
    }

    fn elapsed(&self) -> Duration {
        self.start_time
            .lock()
            .unwrap()
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    fn record_msg_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn record_msg_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    fn record_externalization(&self, values_count: u64, latency: Duration) {
        self.slots_externalized.fetch_add(1, Ordering::Relaxed);
        self.values_externalized
            .fetch_add(values_count, Ordering::Relaxed);
        self.slot_latencies_us
            .lock()
            .unwrap()
            .push(latency.as_micros() as u64);
    }

    fn report(&self) -> MetricsReport {
        let elapsed = self.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        let values = self.values_externalized.load(Ordering::Relaxed);
        let slots = self.slots_externalized.load(Ordering::Relaxed);
        let msgs_sent = self.messages_sent.load(Ordering::Relaxed);
        let msgs_recv = self.messages_received.load(Ordering::Relaxed);

        let latencies = self.slot_latencies_us.lock().unwrap();
        let latency_stats = LatencyStats::from_samples(&latencies);

        MetricsReport {
            elapsed_ms: elapsed.as_millis() as u64,
            values_externalized: values,
            slots_externalized: slots,
            messages_sent: msgs_sent,
            messages_received: msgs_recv,
            throughput_tps: if elapsed_secs > 0.0 {
                values as f64 / elapsed_secs
            } else {
                0.0
            },
            slots_per_sec: if elapsed_secs > 0.0 {
                slots as f64 / elapsed_secs
            } else {
                0.0
            },
            avg_values_per_slot: if slots > 0 {
                values as f64 / slots as f64
            } else {
                0.0
            },
            latency_stats,
        }
    }

}

/// Latency statistics in microseconds
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LatencyStats {
    min_us: u64,
    max_us: u64,
    mean_us: f64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    stddev_us: f64,
}

impl LatencyStats {
    fn from_samples(samples: &[u64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }

        let mut sorted: Vec<u64> = samples.to_vec();
        sorted.sort_unstable();

        let min_us = sorted[0];
        let max_us = sorted[sorted.len() - 1];
        let mean_us = samples.iter().sum::<u64>() as f64 / samples.len() as f64;

        let p50_us = percentile(&sorted, 50);
        let p95_us = percentile(&sorted, 95);
        let p99_us = percentile(&sorted, 99);

        let variance = samples
            .iter()
            .map(|&x| {
                let diff = x as f64 - mean_us;
                diff * diff
            })
            .sum::<f64>()
            / samples.len() as f64;
        let stddev_us = variance.sqrt();

        Self {
            min_us,
            max_us,
            mean_us,
            p50_us,
            p95_us,
            p99_us,
            stddev_us,
        }
    }
}

fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (sorted.len() * p / 100).saturating_sub(1).max(0);
    sorted[idx.min(sorted.len() - 1)]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricsReport {
    elapsed_ms: u64,
    values_externalized: u64,
    slots_externalized: u64,
    messages_sent: u64,
    messages_received: u64,
    throughput_tps: f64,
    slots_per_sec: f64,
    avg_values_per_slot: f64,
    #[serde(flatten)]
    latency_stats: LatencyStats,
}

// Internal struct for display
struct MetricsReportDisplay {
    elapsed: Duration,
    values_externalized: u64,
    slots_externalized: u64,
    messages_sent: u64,
    messages_received: u64,
    throughput_tps: f64,
    slots_per_sec: f64,
    avg_values_per_slot: f64,
    latency_stats: LatencyStats,
}

impl std::fmt::Display for MetricsReportDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "\n============= SCP SIMULATION RESULTS =============")?;
        writeln!(f, "Duration:              {:?}", self.elapsed)?;
        writeln!(f, "Values Externalized:   {}", self.values_externalized)?;
        writeln!(f, "Slots Completed:       {}", self.slots_externalized)?;
        writeln!(f, "Messages Sent:         {}", self.messages_sent)?;
        writeln!(f, "Messages Received:     {}", self.messages_received)?;
        writeln!(f, "--------------------------------------------------")?;
        writeln!(f, "Throughput:            {:.2} tx/s", self.throughput_tps)?;
        writeln!(f, "Slots per Second:      {:.2}", self.slots_per_sec)?;
        writeln!(f, "Avg Values per Slot:   {:.1}", self.avg_values_per_slot)?;
        writeln!(f, "--------------------------------------------------")?;
        writeln!(f, "Slot Latency (p50):    {:.2} ms", self.latency_stats.p50_us as f64 / 1000.0)?;
        writeln!(f, "Slot Latency (p95):    {:.2} ms", self.latency_stats.p95_us as f64 / 1000.0)?;
        writeln!(f, "Slot Latency (p99):    {:.2} ms", self.latency_stats.p99_us as f64 / 1000.0)?;
        writeln!(f, "Slot Latency (mean):   {:.2} ms", self.latency_stats.mean_us / 1000.0)?;
        writeln!(f, "Slot Latency (stddev): {:.2} ms", self.latency_stats.stddev_us / 1000.0)?;
        writeln!(f, "==================================================")?;
        Ok(())
    }
}

/// Benchmark results for multiple runs
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkResult {
    config: BenchmarkConfig,
    runs: Vec<MetricsReport>,
    summary: BenchmarkSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkConfig {
    nodes: usize,
    quorum_k: usize,
    txs: usize,
    rate: u64,
    max_per_slot: usize,
    timebase_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkSummary {
    throughput_mean: f64,
    throughput_stddev: f64,
    throughput_min: f64,
    throughput_max: f64,
    latency_p50_mean_ms: f64,
    latency_p95_mean_ms: f64,
    latency_p99_mean_ms: f64,
}

impl BenchmarkSummary {
    fn from_runs(runs: &[MetricsReport]) -> Self {
        let throughputs: Vec<f64> = runs.iter().map(|r| r.throughput_tps).collect();
        let latency_p50s: Vec<f64> = runs.iter().map(|r| r.latency_stats.p50_us as f64 / 1000.0).collect();
        let latency_p95s: Vec<f64> = runs.iter().map(|r| r.latency_stats.p95_us as f64 / 1000.0).collect();
        let latency_p99s: Vec<f64> = runs.iter().map(|r| r.latency_stats.p99_us as f64 / 1000.0).collect();

        let throughput_mean = mean(&throughputs);
        let throughput_stddev = stddev(&throughputs, throughput_mean);

        Self {
            throughput_mean,
            throughput_stddev,
            throughput_min: throughputs.iter().cloned().fold(f64::INFINITY, f64::min),
            throughput_max: throughputs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            latency_p50_mean_ms: mean(&latency_p50s),
            latency_p95_mean_ms: mean(&latency_p95s),
            latency_p99_mean_ms: mean(&latency_p99s),
        }
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn stddev(values: &[f64], mean: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
    variance.sqrt()
}

// ============================================================================
// Node Message Types
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
struct TxValue(String);

impl bt_crypto_digestible::Digestible for TxValue {
    fn append_to_transcript<DT: bt_crypto_digestible::DigestTranscript>(
        &self,
        context: &'static [u8],
        transcript: &mut DT,
    ) {
        self.0.as_bytes().append_to_transcript(context, transcript);
    }
}

impl std::fmt::Display for TxValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug)]
enum NodeMessage {
    Value(TxValue),
    ScpMsg(Arc<Msg<TxValue>>),
    Stop,
}

// ============================================================================
// Simulation Node
// ============================================================================

struct SimNode {
    sender: Sender<NodeMessage>,
    ledger: Arc<Mutex<Vec<Vec<TxValue>>>>,
}

impl SimNode {
    fn new(
        node_id: NodeID,
        quorum_set: QuorumSet,
        peers: HashSet<NodeID>,
        nodes_map: Arc<DashMap<NodeID, SimNode>>,
        metrics: Arc<Metrics>,
        slot_start_times: Arc<Mutex<HashMap<SlotIndex, Instant>>>,
        max_slot_values: usize,
        scp_timebase: Duration,
        verbose: bool,
    ) -> (Self, thread::JoinHandle<()>) {
        let (sender, receiver) = unbounded();
        let ledger = Arc::new(Mutex::new(Vec::new()));
        let ledger_clone = ledger.clone();

        let handle = thread::Builder::new()
            .name(format!("node-{}", node_id))
            .spawn(move || {
                run_node(
                    node_id,
                    quorum_set,
                    peers,
                    receiver,
                    nodes_map,
                    ledger_clone,
                    metrics,
                    slot_start_times,
                    max_slot_values,
                    scp_timebase,
                    verbose,
                )
            })
            .expect("Failed to spawn node thread");

        (SimNode { sender, ledger }, handle)
    }

    fn send_value(&self, value: TxValue) {
        let _ = self.sender.send(NodeMessage::Value(value));
    }

    fn send_msg(&self, msg: Arc<Msg<TxValue>>) {
        let _ = self.sender.send(NodeMessage::ScpMsg(msg));
    }

    fn send_stop(&self) {
        let _ = self.sender.send(NodeMessage::Stop);
    }

    fn ledger_size(&self) -> usize {
        self.ledger
            .lock()
            .unwrap()
            .iter()
            .map(|block| block.len())
            .sum()
    }
}

fn run_node(
    node_id: NodeID,
    quorum_set: QuorumSet,
    peers: HashSet<NodeID>,
    receiver: Receiver<NodeMessage>,
    nodes_map: Arc<DashMap<NodeID, SimNode>>,
    ledger: Arc<Mutex<Vec<Vec<TxValue>>>>,
    metrics: Arc<Metrics>,
    slot_start_times: Arc<Mutex<HashMap<SlotIndex, Instant>>>,
    max_slot_values: usize,
    scp_timebase: Duration,
    verbose: bool,
) {
    // Create validity and combine functions
    let validity_fn: ValidityFn<TxValue, String> = Arc::new(|_| Ok(()));
    let combine_fn: CombineFn<TxValue, String> = Arc::new(move |values| {
        let mut combined: Vec<TxValue> = values.to_vec();
        combined.sort();
        combined.dedup();
        combined.truncate(max_slot_values);
        Ok(combined)
    });

    // Create SCP node with a stub logger
    let logger = bt_common::logger::create_null_logger();
    let mut scp_node = Node::new(
        node_id.clone(),
        quorum_set,
        validity_fn,
        combine_fn,
        0, // initial slot
        logger,
    );
    scp_node.scp_timebase = scp_timebase;

    let mut pending_values: Vec<TxValue> = Vec::new();
    let mut current_slot: SlotIndex = 0;
    let mut slot_started = false;

    loop {
        // Drain all available messages in a batch (more efficient than one-by-one)
        let mut processed = 0;
        loop {
            match receiver.try_recv() {
                Ok(NodeMessage::Value(v)) => {
                    pending_values.push(v);
                    // Record slot start time when first value arrives for this slot
                    if !slot_started {
                        slot_start_times
                            .lock()
                            .unwrap()
                            .entry(current_slot)
                            .or_insert_with(Instant::now);
                        slot_started = true;
                    }
                    processed += 1;
                }
                Ok(NodeMessage::ScpMsg(msg)) => {
                    metrics.record_msg_received();
                    if let Ok(Some(out_msg)) = scp_node.handle_message(&msg) {
                        broadcast_msg(&nodes_map, &peers, out_msg, &metrics);
                    }
                    processed += 1;
                }
                Ok(NodeMessage::Stop) => return,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return,
            }
        }

        // Propose pending values
        if !pending_values.is_empty() {
            let to_propose: BTreeSet<TxValue> = pending_values
                .iter()
                .take(max_slot_values)
                .cloned()
                .collect();

            if let Ok(Some(out_msg)) = scp_node.propose_values(to_propose) {
                broadcast_msg(&nodes_map, &peers, out_msg, &metrics);
            }
        }

        // Process timeouts
        for out_msg in scp_node.process_timeouts() {
            broadcast_msg(&nodes_map, &peers, out_msg, &metrics);
        }

        // Check for externalization
        if let Some(block) = scp_node.get_externalized_values(current_slot) {
            let externalized: HashSet<TxValue> = block.iter().cloned().collect();
            pending_values.retain(|v| !externalized.contains(v));

            // Calculate slot latency
            let latency = {
                let times = slot_start_times.lock().unwrap();
                times
                    .get(&current_slot)
                    .map(|start| start.elapsed())
                    .unwrap_or_default()
            };

            if verbose {
                eprintln!(
                    "[Node {}] Slot {} externalized {} values in {:?}",
                    node_id,
                    current_slot,
                    block.len(),
                    latency
                );
            }

            metrics.record_externalization(block.len() as u64, latency);
            ledger.lock().unwrap().push(block);
            current_slot += 1;
            slot_started = false;
        }

        // Yield if no messages were processed (reduces CPU spinning)
        if processed == 0 {
            thread::yield_now();
        }
    }
}

fn broadcast_msg(
    nodes_map: &Arc<DashMap<NodeID, SimNode>>,
    peers: &HashSet<NodeID>,
    msg: Msg<TxValue>,
    metrics: &Arc<Metrics>,
) {
    let msg = Arc::new(msg);
    // DashMap allows concurrent reads - no global lock needed!
    for peer_id in peers {
        if let Some(peer) = nodes_map.get(peer_id) {
            peer.send_msg(msg.clone());
            metrics.record_msg_sent();
        }
    }
}

// ============================================================================
// Network Setup
// ============================================================================

fn build_mesh_network(
    n: usize,
    k: usize,
    metrics: Arc<Metrics>,
    slot_start_times: Arc<Mutex<HashMap<SlotIndex, Instant>>>,
    max_slot_values: usize,
    scp_timebase: Duration,
    verbose: bool,
) -> (
    Arc<DashMap<NodeID, SimNode>>,
    Vec<NodeID>,
    Vec<thread::JoinHandle<()>>,
) {
    let nodes_map: Arc<DashMap<NodeID, SimNode>> = Arc::new(DashMap::new());
    let mut handles = Vec::new();
    let mut node_ids = Vec::new();

    // Create node IDs first
    for i in 0..n {
        node_ids.push(test_node_id(i as u32));
    }

    // Create nodes
    for i in 0..n {
        let node_id = node_ids[i].clone();
        let peers: HashSet<NodeID> = node_ids
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, id)| id.clone())
            .collect();
        let peer_vec: Vec<NodeID> = peers.iter().cloned().collect();
        let quorum_set = QuorumSet::new_with_node_ids(k as u32, peer_vec);

        let (sim_node, handle) = SimNode::new(
            node_id.clone(),
            quorum_set,
            peers,
            nodes_map.clone(),
            metrics.clone(),
            slot_start_times.clone(),
            max_slot_values,
            scp_timebase,
            verbose,
        );

        nodes_map.insert(node_id, sim_node);
        handles.push(handle);
    }

    (nodes_map, node_ids, handles)
}

// ============================================================================
// Simulation Runner
// ============================================================================

struct SimConfig {
    nodes: usize,
    k: usize,
    txs: usize,
    rate: u64,
    max_per_slot: usize,
    timebase_ms: u64,
    verbose: bool,
    quiet: bool,
    seed: u64,
}

fn calculate_k(nodes: usize, quorum_k: Option<usize>) -> Result<usize, String> {
    let k = quorum_k.unwrap_or_else(|| {
        let peers = nodes - 1;
        (2 * peers + 2) / 3
    });

    if k > nodes - 1 {
        return Err(format!(
            "quorum threshold k={} exceeds number of peers {} (n-1)",
            k,
            nodes - 1
        ));
    }
    Ok(k)
}

fn run_simulation(config: &SimConfig) -> Result<MetricsReport, String> {
    let metrics = Arc::new(Metrics::new());
    let slot_start_times: Arc<Mutex<HashMap<SlotIndex, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let scp_timebase = Duration::from_millis(config.timebase_ms);

    if !config.quiet {
        println!("Building {}-node mesh network...", config.nodes);
    }

    let (nodes_map, node_ids, handles) = build_mesh_network(
        config.nodes,
        config.k,
        metrics.clone(),
        slot_start_times.clone(),
        config.max_per_slot,
        scp_timebase,
        config.verbose,
    );

    // Generate random values
    if !config.quiet {
        println!("Generating {} transactions...", config.txs);
    }
    let mut rng = StdRng::seed_from_u64(config.seed);
    let values: Vec<TxValue> = (0..config.txs)
        .map(|_| {
            let s: String = (0..10).map(|_| rng.gen_range('a'..='z')).collect();
            TxValue(s)
        })
        .collect();

    // Start metrics timing
    metrics.start();
    if !config.quiet {
        println!("Starting simulation...\n");
    }

    // Submit values
    let submission_delay = Duration::from_micros(1_000_000 / config.rate);
    let mut last_progress = Instant::now();

    for (i, value) in values.iter().enumerate() {
        // DashMap allows concurrent access - no lock needed
        for node_id in &node_ids {
            if let Some(node) = nodes_map.get(node_id) {
                node.send_value(value.clone());
            }
        }

        thread::sleep(submission_delay);

        if !config.quiet && last_progress.elapsed() > Duration::from_secs(1) {
            let report = metrics.report();
            println!(
                "Progress: submitted {}/{} | externalized {} | {:.0} tx/s",
                i + 1,
                config.txs,
                report.values_externalized,
                report.throughput_tps
            );
            last_progress = Instant::now();
        }
    }

    if !config.quiet {
        println!("\nAll transactions submitted, waiting for externalization...");
    }

    // Wait for externalization
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let min_ledger_size = node_ids
            .iter()
            .filter_map(|id| nodes_map.get(id).map(|n| n.ledger_size()))
            .min()
            .unwrap_or(0);

        if min_ledger_size >= config.txs {
            break;
        }

        if Instant::now() > deadline {
            return Err("Timeout waiting for externalization".to_string());
        }

        if !config.quiet && last_progress.elapsed() > Duration::from_secs(2) {
            let report = metrics.report();
            println!(
                "Waiting: externalized {}/{} | {:.0} tx/s",
                report.values_externalized, config.txs, report.throughput_tps
            );
            last_progress = Instant::now();
        }

        thread::sleep(Duration::from_millis(10));
    }

    // Stop all nodes
    for node_id in &node_ids {
        if let Some(node) = nodes_map.get(node_id) {
            node.send_stop();
        }
    }

    for handle in handles {
        let _ = handle.join();
    }

    // Verify ledger consistency
    if !config.quiet {
        println!("Verifying ledger consistency...");
    }
    let first_ledger: Vec<Vec<TxValue>> = nodes_map
        .get(&node_ids[0])
        .map(|n| n.ledger.lock().unwrap().clone())
        .unwrap_or_default();

    for (i, node_id) in node_ids.iter().enumerate().skip(1) {
        if let Some(node) = nodes_map.get(node_id) {
            let ledger = node.ledger.lock().unwrap();
            if ledger.len() != first_ledger.len() {
                return Err(format!(
                    "Node {} has {} blocks, Node 0 has {}",
                    i,
                    ledger.len(),
                    first_ledger.len()
                ));
            }
            for (j, block) in ledger.iter().enumerate() {
                if block != &first_ledger[j] {
                    return Err(format!("Block {} differs between Node 0 and Node {}", j, i));
                }
            }
        }
    }

    Ok(metrics.report())
}

fn run_benchmark(
    base_config: &SimConfig,
    runs: usize,
    warmup: usize,
    node_counts: Option<Vec<usize>>,
) -> Vec<BenchmarkResult> {
    let node_counts = node_counts.unwrap_or_else(|| vec![base_config.nodes]);
    let mut results = Vec::new();

    for &nodes in &node_counts {
        let k = match calculate_k(nodes, None) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("Skipping {} nodes: {}", nodes, e);
                continue;
            }
        };

        println!("\n========== Benchmarking {} nodes (k={}) ==========", nodes, k);

        let config = SimConfig {
            nodes,
            k,
            ..*base_config
        };

        // Warmup runs
        for w in 0..warmup {
            println!("Warmup run {}/{}...", w + 1, warmup);
            let warmup_config = SimConfig {
                quiet: true,
                ..config
            };
            let _ = run_simulation(&warmup_config);
        }

        // Benchmark runs
        let mut run_results = Vec::new();
        for r in 0..runs {
            println!("Run {}/{}...", r + 1, runs);
            let run_config = SimConfig {
                quiet: true,
                seed: 42 + r as u64,
                ..config
            };
            match run_simulation(&run_config) {
                Ok(report) => {
                    println!(
                        "  -> {:.0} tx/s, p50={:.2}ms, p99={:.2}ms",
                        report.throughput_tps,
                        report.latency_stats.p50_us as f64 / 1000.0,
                        report.latency_stats.p99_us as f64 / 1000.0
                    );
                    run_results.push(report);
                }
                Err(e) => {
                    eprintln!("  -> ERROR: {}", e);
                }
            }
        }

        if !run_results.is_empty() {
            let summary = BenchmarkSummary::from_runs(&run_results);
            println!(
                "\nSummary: {:.0} ± {:.0} tx/s (min={:.0}, max={:.0})",
                summary.throughput_mean,
                summary.throughput_stddev,
                summary.throughput_min,
                summary.throughput_max
            );

            results.push(BenchmarkResult {
                config: BenchmarkConfig {
                    nodes,
                    quorum_k: k,
                    txs: config.txs,
                    rate: config.rate,
                    max_per_slot: config.max_per_slot,
                    timebase_ms: config.timebase_ms,
                },
                runs: run_results,
                summary,
            });
        }
    }

    results
}

fn run_comparison(
    base_config: &SimConfig,
    node_counts: Vec<usize>,
    k_values: Option<Vec<usize>>,
) -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    println!("\n============= CONFIGURATION COMPARISON =============\n");
    println!(
        "{:>6} {:>4} {:>12} {:>10} {:>10} {:>10}",
        "Nodes", "k", "Throughput", "p50 (ms)", "p95 (ms)", "p99 (ms)"
    );
    println!("{}", "-".repeat(60));

    for &nodes in &node_counts {
        let k_vals = k_values.clone().unwrap_or_else(|| {
            vec![calculate_k(nodes, None).unwrap_or(nodes - 1)]
        });

        for &k in &k_vals {
            if k > nodes - 1 {
                continue;
            }

            let config = SimConfig {
                nodes,
                k,
                quiet: true,
                ..*base_config
            };

            match run_simulation(&config) {
                Ok(report) => {
                    println!(
                        "{:>6} {:>4} {:>10.0} tx/s {:>10.2} {:>10.2} {:>10.2}",
                        nodes,
                        k,
                        report.throughput_tps,
                        report.latency_stats.p50_us as f64 / 1000.0,
                        report.latency_stats.p95_us as f64 / 1000.0,
                        report.latency_stats.p99_us as f64 / 1000.0
                    );

                    let summary = BenchmarkSummary::from_runs(&[report.clone()]);
                    results.push(BenchmarkResult {
                        config: BenchmarkConfig {
                            nodes,
                            quorum_k: k,
                            txs: config.txs,
                            rate: config.rate,
                            max_per_slot: config.max_per_slot,
                            timebase_ms: config.timebase_ms,
                        },
                        runs: vec![report],
                        summary,
                    });
                }
                Err(e) => {
                    println!("{:>6} {:>4} ERROR: {}", nodes, k, e);
                }
            }
        }
    }

    println!("{}", "-".repeat(60));
    results
}

fn parse_csv_usize(s: &str) -> Vec<usize> {
    s.split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect()
}

fn save_results(results: &[BenchmarkResult], path: &str) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(results)?;
    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;
    println!("\nResults saved to {}", path);
    Ok(())
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    let args = Args::parse();

    // Validate node count
    if args.nodes < 2 || args.nodes > 10 {
        eprintln!("Error: nodes must be between 2 and 10");
        std::process::exit(1);
    }

    let k = match calculate_k(args.nodes, args.quorum_k) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let base_config = SimConfig {
        nodes: args.nodes,
        k,
        txs: args.txs,
        rate: args.rate,
        max_per_slot: args.max_per_slot,
        timebase_ms: args.timebase_ms,
        verbose: args.verbose,
        quiet: args.quiet,
        seed: 42,
    };

    match args.command {
        Some(Command::Bench {
            runs,
            node_counts,
            warmup,
        }) => {
            let node_list = node_counts.map(|s| parse_csv_usize(&s));
            let results = run_benchmark(&base_config, runs, warmup, node_list);

            if let Some(output) = args.output {
                if let Err(e) = save_results(&results, &output) {
                    eprintln!("Failed to save results: {}", e);
                }
            }
        }

        Some(Command::Compare {
            node_counts,
            k_values,
        }) => {
            let node_list = parse_csv_usize(&node_counts);
            let k_list = k_values.map(|s| parse_csv_usize(&s));
            let results = run_comparison(&base_config, node_list, k_list);

            if let Some(output) = args.output {
                if let Err(e) = save_results(&results, &output) {
                    eprintln!("Failed to save results: {}", e);
                }
            }
        }

        Some(Command::Run) | None => {
            // Single run mode
            if !args.quiet {
                println!("============= SCP SIMULATION CONFIG ==============");
                println!("Nodes:                 {}", args.nodes);
                println!("Quorum threshold (k):  {}", k);
                println!("Transactions:          {}", args.txs);
                println!("Submission rate:       {} tx/s", args.rate);
                println!("Max per slot:          {}", args.max_per_slot);
                println!("SCP timebase:          {} ms", args.timebase_ms);
                println!("==================================================\n");
            }

            match run_simulation(&base_config) {
                Ok(report) => {
                    // Display the report
                    let display = MetricsReportDisplay {
                        elapsed: Duration::from_millis(report.elapsed_ms),
                        values_externalized: report.values_externalized,
                        slots_externalized: report.slots_externalized,
                        messages_sent: report.messages_sent,
                        messages_received: report.messages_received,
                        throughput_tps: report.throughput_tps,
                        slots_per_sec: report.slots_per_sec,
                        avg_values_per_slot: report.avg_values_per_slot,
                        latency_stats: report.latency_stats.clone(),
                    };
                    println!("{}", display);
                    println!("All {} nodes have consistent ledgers!", args.nodes);

                    if let Some(output) = args.output {
                        let results = vec![BenchmarkResult {
                            config: BenchmarkConfig {
                                nodes: args.nodes,
                                quorum_k: k,
                                txs: args.txs,
                                rate: args.rate,
                                max_per_slot: args.max_per_slot,
                                timebase_ms: args.timebase_ms,
                            },
                            runs: vec![report.clone()],
                            summary: BenchmarkSummary::from_runs(&[report]),
                        }];
                        if let Err(e) = save_results(&results, &output) {
                            eprintln!("Failed to save results: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Simulation failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
