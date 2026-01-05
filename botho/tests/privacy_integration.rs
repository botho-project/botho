// Copyright (c) 2024 Botho Foundation

//! Integration tests for onion gossip privacy features.
//!
//! This module provides comprehensive integration tests to validate onion
//! gossip functionality in realistic network scenarios.
//!
//! # Test Categories
//!
//! ## Circuit Tests
//! - Basic circuit construction with diverse hops
//! - Circuit pool maintenance and rotation
//! - Subnet diversity requirements
//!
//! ## Relay Tests
//! - Relay performance under load
//! - Peer churn resilience
//! - Rate limiting effectiveness
//!
//! ## Adversarial Tests
//! - Deanonymization resistance
//! - Sybil attack resistance
//! - Timing correlation resistance

use botho::network::privacy::{
    encrypt_exit_layer, wrap_onion, CircuitHopKey, CircuitId, CircuitPool, CircuitPoolConfig,
    CircuitSelector, OutboundCircuit, RelayAction, RelayHandler, RelayPeerInfo, RelayState,
    RelayStateConfig, SelectionConfig, SymmetricKey, CIRCUIT_HOPS,
};
use bth_gossip::{InnerMessage, NatType, OnionRelayMessage, RelayCapacity};
use libp2p::PeerId;
use rand::Rng;
use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

// ============================================================================
// Test Utilities
// ============================================================================

/// A simulated peer for privacy testing.
#[derive(Debug, Clone)]
struct SimulatedPeer {
    peer_id: PeerId,
    ip_addr: Ipv4Addr,
    relay_capacity: RelayCapacity,
    is_adversary: bool,
    observed_txs: Arc<RwLock<HashSet<[u8; 32]>>>,
    messages_relayed: Arc<AtomicU64>,
}

/// Create a default relay capacity that meets the minimum selection threshold.
fn default_test_relay_capacity() -> RelayCapacity {
    RelayCapacity {
        bandwidth_bps: 5_000_000, // 5 Mbps → 0.2 bandwidth score
        uptime_ratio: 0.8,        // 0.24 uptime score
        nat_type: NatType::Open,  // 0.2 NAT bonus
        current_load: 0.1,        // Low load
    }
    // Total: ~0.58 score (well above 0.2 threshold)
}

impl SimulatedPeer {
    fn new(ip_addr: Ipv4Addr) -> Self {
        Self {
            peer_id: PeerId::random(),
            ip_addr,
            relay_capacity: default_test_relay_capacity(),
            is_adversary: false,
            observed_txs: Arc::new(RwLock::new(HashSet::new())),
            messages_relayed: Arc::new(AtomicU64::new(0)),
        }
    }

    fn with_capacity(ip_addr: Ipv4Addr, capacity: RelayCapacity) -> Self {
        Self {
            peer_id: PeerId::random(),
            ip_addr,
            relay_capacity: capacity,
            is_adversary: false,
            observed_txs: Arc::new(RwLock::new(HashSet::new())),
            messages_relayed: Arc::new(AtomicU64::new(0)),
        }
    }

    fn to_relay_info(&self) -> RelayPeerInfo {
        RelayPeerInfo::new(
            self.peer_id,
            Some(self.ip_addr),
            self.relay_capacity.clone(),
        )
    }

    fn observe_tx(&self, tx_hash: [u8; 32]) {
        self.observed_txs.write().unwrap().insert(tx_hash);
    }

    fn has_observed(&self, tx_hash: &[u8; 32]) -> bool {
        self.observed_txs.read().unwrap().contains(tx_hash)
    }

    fn record_relay(&self) {
        self.messages_relayed.fetch_add(1, Ordering::Relaxed);
    }

    fn relay_count(&self) -> u64 {
        self.messages_relayed.load(Ordering::Relaxed)
    }
}

/// A simulated privacy network for testing.
struct PrivacyTestNetwork {
    peers: Vec<SimulatedPeer>,
    peer_index: HashMap<PeerId, usize>,
    selector: CircuitSelector,
    circuits: Vec<TestCircuit>,
}

struct TestCircuit {
    circuit: OutboundCircuit,
    #[allow(dead_code)]
    origin_idx: usize,
    tx_hashes: Vec<[u8; 32]>,
}

impl PrivacyTestNetwork {
    fn new(size: usize) -> Self {
        let mut peers = Vec::with_capacity(size);
        let mut peer_index = HashMap::new();

        for i in 0..size {
            // Each peer gets a different /16 subnet (10.i.x.1)
            // This ensures diversity requirements can be met
            let second_octet = (i % 256) as u8;
            let ip = Ipv4Addr::new(10, second_octet, 0, 1);

            let peer = SimulatedPeer::new(ip);
            peer_index.insert(peer.peer_id, i);
            peers.push(peer);
        }

        Self {
            peers,
            peer_index,
            selector: CircuitSelector::new(SelectionConfig::default()),
            circuits: Vec::new(),
        }
    }

    fn with_subnets(subnets: Vec<((u8, u8), usize)>) -> Self {
        let mut peers = Vec::new();
        let mut peer_index = HashMap::new();

        for ((a, b), count) in subnets {
            for i in 0..count {
                let ip = Ipv4Addr::new(a, b, (i % 256) as u8, ((i / 256) % 256 + 1) as u8);
                let peer = SimulatedPeer::new(ip);
                peer_index.insert(peer.peer_id, peers.len());
                peers.push(peer);
            }
        }

        Self {
            peers,
            peer_index,
            selector: CircuitSelector::new(SelectionConfig::default()),
            circuits: Vec::new(),
        }
    }

    fn with_adversaries(total: usize, adversary_count: usize) -> Self {
        let mut network = Self::new(total);

        for i in 0..adversary_count.min(total) {
            network.peers[i].is_adversary = true;
        }

        network
    }

    fn adversary_count(&self) -> usize {
        self.peers.iter().filter(|p| p.is_adversary).count()
    }

    fn honest_peer_count(&self) -> usize {
        self.peers.iter().filter(|p| !p.is_adversary).count()
    }

    fn relay_peers(&self) -> Vec<RelayPeerInfo> {
        self.peers.iter().map(|p| p.to_relay_info()).collect()
    }

    fn build_circuit(&mut self, origin_idx: usize) -> Result<usize, String> {
        let available_peers: Vec<RelayPeerInfo> = self
            .peers
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != origin_idx)
            .map(|(_, p)| p.to_relay_info())
            .collect();

        let hops = self
            .selector
            .select_diverse_hops(&available_peers, 3)
            .map_err(|e| e.to_string())?;

        let mut rng = rand::thread_rng();
        let circuit = OutboundCircuit::new(
            CircuitId::random(&mut rng),
            [hops[0], hops[1], hops[2]],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(600),
        );

        let circuit_idx = self.circuits.len();
        self.circuits.push(TestCircuit {
            circuit,
            origin_idx,
            tx_hashes: Vec::new(),
        });

        Ok(circuit_idx)
    }

    fn send_transaction(&mut self, circuit_idx: usize, tx_hash: [u8; 32]) -> PeerId {
        let circuit = &mut self.circuits[circuit_idx];
        circuit.tx_hashes.push(tx_hash);

        for hop in circuit.circuit.hops() {
            if let Some(&idx) = self.peer_index.get(hop) {
                self.peers[idx].record_relay();
                if self.peers[idx].is_adversary {
                    self.peers[idx].observe_tx(tx_hash);
                }
            }
        }

        *circuit.circuit.exit_hop()
    }

    fn verify_circuit_diversity(&self, circuit_idx: usize) -> bool {
        let circuit = &self.circuits[circuit_idx].circuit;
        let mut subnets = HashSet::new();

        for hop in circuit.hops() {
            if let Some(&idx) = self.peer_index.get(hop) {
                let ip = self.peers[idx].ip_addr;
                let subnet = ((ip.octets()[0] as u16) << 8) | (ip.octets()[1] as u16);
                if !subnets.insert(subnet) {
                    return false;
                }
            }
        }

        subnets.len() == 3
    }

    fn kill_peer(&mut self, idx: usize) {
        let peer_id = self.peers[idx].peer_id;
        self.peer_index.remove(&peer_id);
    }

    fn alive_peer_indices(&self) -> Vec<usize> {
        self.peers
            .iter()
            .enumerate()
            .filter(|(_, p)| self.peer_index.contains_key(&p.peer_id))
            .map(|(i, _)| i)
            .collect()
    }
}

fn make_test_circuit(lifetime: Duration) -> OutboundCircuit {
    let mut rng = rand::thread_rng();
    OutboundCircuit::new(
        CircuitId::random(&mut rng),
        [PeerId::random(), PeerId::random(), PeerId::random()],
        [
            SymmetricKey::random(&mut rng),
            SymmetricKey::random(&mut rng),
            SymmetricKey::random(&mut rng),
        ],
        lifetime,
    )
}

fn high_capacity_relay() -> RelayCapacity {
    RelayCapacity {
        bandwidth_bps: 100_000_000,
        uptime_ratio: 0.99,
        nat_type: NatType::Open,
        current_load: 0.1,
    }
}

fn low_capacity_relay() -> RelayCapacity {
    RelayCapacity {
        bandwidth_bps: 2_000_000, // Just above minimum threshold
        uptime_ratio: 0.6,
        nat_type: NatType::Restricted, // Gives 0.1 bonus
        current_load: 0.3,
    }
    // Score: ~0.23 (just above 0.2 threshold)
}

fn random_tx_hash() -> [u8; 32] {
    let mut hash = [0u8; 32];
    rand::thread_rng().fill(&mut hash);
    hash
}

fn to_gossip_circuit_id(id: &CircuitId) -> bth_gossip::CircuitId {
    bth_gossip::CircuitId(*id.as_bytes())
}

// ============================================================================
// Circuit Tests
// ============================================================================

#[test]
fn test_circuit_construction() {
    let mut network = PrivacyTestNetwork::new(10);
    let circuit_idx = network.build_circuit(0).expect("should build circuit");

    let circuit = &network.circuits[circuit_idx].circuit;
    assert_eq!(circuit.hops().len(), CIRCUIT_HOPS);

    let unique_hops: HashSet<_> = circuit.hops().iter().collect();
    assert_eq!(unique_hops.len(), CIRCUIT_HOPS);

    for hop in circuit.hops() {
        assert!(
            network.peer_index.contains_key(hop),
            "Hop should exist in network"
        );
    }

    let origin_peer_id = network.peers[0].peer_id;
    assert!(
        !unique_hops.contains(&origin_peer_id),
        "Origin should not be in circuit hops"
    );
}

#[test]
fn test_circuit_subnet_diversity() {
    let network =
        PrivacyTestNetwork::with_subnets(vec![((192, 168), 5), ((10, 0), 5), ((172, 16), 5)]);

    let selector = CircuitSelector::new(SelectionConfig::default());
    let relay_peers = network.relay_peers();

    for _ in 0..10 {
        let result = selector.select_diverse_hops(&relay_peers, 3);
        assert!(result.is_ok(), "Should be able to select diverse hops");

        let hops = result.unwrap();
        let mut subnets = HashSet::new();

        for hop in &hops {
            for peer in &network.peers {
                if peer.peer_id == *hop {
                    let octets = peer.ip_addr.octets();
                    let subnet = ((octets[0] as u16) << 8) | (octets[1] as u16);
                    subnets.insert(subnet);
                    break;
                }
            }
        }

        assert_eq!(
            subnets.len(),
            3,
            "All 3 hops should be in different subnets"
        );
    }
}

#[test]
fn test_diversity_enforcement() {
    let network = PrivacyTestNetwork::with_subnets(vec![((10, 0), 10)]);

    let selector = CircuitSelector::new(SelectionConfig {
        strict_diversity: true,
        ..Default::default()
    });
    let relay_peers = network.relay_peers();

    let result = selector.select_diverse_hops(&relay_peers, 3);
    assert!(result.is_err(), "Should fail with insufficient diversity");
}

#[test]
fn test_circuit_pool_minimum() {
    let min_circuits = 3;
    let mut pool = CircuitPool::new(CircuitPoolConfig {
        min_circuits,
        ..Default::default()
    });

    assert_eq!(pool.active_count(), 0);
    assert!(pool.needs_more_circuits());

    for _ in 0..min_circuits {
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
    }

    assert_eq!(pool.active_count(), min_circuits);
    assert!(!pool.needs_more_circuits());
}

#[test]
fn test_circuit_pool_expiration() {
    let mut pool = CircuitPool::new(CircuitPoolConfig::default());

    for _ in 0..5 {
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
    }

    assert_eq!(pool.total_count(), 5);

    let removed = pool.remove_expired();
    assert_eq!(removed, 0, "Fresh circuits should not be expired");
    assert_eq!(pool.total_count(), 5);
}

#[test]
fn test_circuit_selection_randomness() {
    let mut pool = CircuitPool::new(CircuitPoolConfig::default());

    for _ in 0..10 {
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
    }

    let mut selected_ids = HashSet::new();
    for _ in 0..50 {
        if let Some(circuit) = pool.get_circuit() {
            selected_ids.insert(*circuit.id().as_bytes());
        }
    }

    assert!(
        selected_ids.len() > 1,
        "Random selection should hit multiple circuits, got {} unique",
        selected_ids.len()
    );
}

#[test]
fn test_weighted_relay_selection() {
    let high_cap_peers: Vec<_> = (0..3)
        .map(|i| {
            let ip = Ipv4Addr::new(10, i, 0, 1);
            SimulatedPeer::with_capacity(ip, high_capacity_relay())
        })
        .collect();

    let low_cap_peers: Vec<_> = (0..3)
        .map(|i| {
            let ip = Ipv4Addr::new(172, i, 0, 1);
            SimulatedPeer::with_capacity(ip, low_capacity_relay())
        })
        .collect();

    let all_peers: Vec<RelayPeerInfo> = high_cap_peers
        .iter()
        .chain(low_cap_peers.iter())
        .map(|p| p.to_relay_info())
        .collect();

    let selector = CircuitSelector::new(SelectionConfig::default());

    let mut high_cap_count = 0;
    let high_cap_ids: HashSet<_> = high_cap_peers.iter().map(|p| p.peer_id).collect();

    for _ in 0..100 {
        if let Ok(hops) = selector.select_diverse_hops(&all_peers, 3) {
            for hop in hops {
                if high_cap_ids.contains(&hop) {
                    high_cap_count += 1;
                }
            }
        }
    }

    assert!(
        high_cap_count > 150,
        "High capacity peers selected {} times (expected >150)",
        high_cap_count
    );
}

// ============================================================================
// Relay Tests
// ============================================================================

#[test]
fn test_relay_under_load() {
    let mut network = PrivacyTestNetwork::new(20);

    let mut circuit_indices = Vec::new();
    for origin_idx in 0..10 {
        if let Ok(idx) = network.build_circuit(origin_idx) {
            circuit_indices.push(idx);
        }
    }

    assert!(
        circuit_indices.len() >= 5,
        "Should build at least 5 circuits"
    );

    for i in 0..100 {
        let circuit_idx = circuit_indices[i % circuit_indices.len()];
        let tx_hash = random_tx_hash();
        network.send_transaction(circuit_idx, tx_hash);
    }

    let total_relays: u64 = network.peers.iter().map(|p| p.relay_count()).sum();
    assert_eq!(total_relays, 300, "Each tx should traverse 3 hops");

    let max_relay = network.peers.iter().map(|p| p.relay_count()).max().unwrap();
    let avg_relay = total_relays as f64 / network.peers.len() as f64;

    assert!(
        max_relay as f64 <= avg_relay * 10.0,
        "Relay load should be distributed, max={} avg={:.1}",
        max_relay,
        avg_relay
    );
}

#[test]
fn test_peer_churn_resilience() {
    let mut network = PrivacyTestNetwork::new(20);

    // Build initial circuits from nodes 0-4
    let mut initial_circuits = Vec::new();
    for origin_idx in 0..5 {
        if let Ok(idx) = network.build_circuit(origin_idx) {
            initial_circuits.push(idx);
        }
    }
    assert!(
        initial_circuits.len() >= 3,
        "Should build at least 3 circuits"
    );

    // Kill 30% of nodes (nodes 10-15)
    for i in 10..16 {
        network.kill_peer(i);
    }

    let alive_peers = network.alive_peer_indices();
    assert!(
        alive_peers.len() >= 14,
        "Should have at least 14 alive peers"
    );

    // Build new circuits using only alive peers
    // Use the selector directly with filtered peers to avoid using dead peers
    let alive_relay_peers: Vec<RelayPeerInfo> = network
        .peers
        .iter()
        .enumerate()
        .filter(|(_, p)| network.peer_index.contains_key(&p.peer_id))
        .map(|(_, p)| p.to_relay_info())
        .collect();

    let selector = CircuitSelector::new(SelectionConfig::default());

    // Try to build circuits
    let mut success_count = 0;
    for _ in 0..5 {
        if selector.select_diverse_hops(&alive_relay_peers, 3).is_ok() {
            success_count += 1;
        }
    }

    // Should be able to build at least 2 circuits with remaining peers
    assert!(
        success_count >= 2,
        "Should be able to build at least 2 circuits after churn, got {}",
        success_count
    );

    // Verify the selector can consistently find diverse peers
    // (14 alive peers in different subnets is plenty for diversity)
    assert!(
        alive_relay_peers.len() >= 14,
        "Should have 14+ alive peers for circuit building"
    );
}

#[test]
fn test_rate_limiting_under_flood() {
    use botho::network::privacy::rate_limit::RelayRateLimits;

    let config = RelayStateConfig {
        rate_limits: RelayRateLimits {
            relay_msgs_per_sec: 2,
            relay_bandwidth_per_peer: 10_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut relay_state = RelayState::new(config);
    let handler = RelayHandler::new();

    let circuit_id = CircuitId::random(&mut rand::thread_rng());
    let key = SymmetricKey::random(&mut rand::thread_rng());
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    let inner = InnerMessage::Cover;
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
    let encrypted = encrypt_exit_layer(&key, &inner_bytes);

    let msg = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: encrypted.clone(),
    };

    let abusive_peer = PeerId::random();

    let mut rate_limited_count = 0;
    for _ in 0..20 {
        let action = handler.handle_message(&mut relay_state, &abusive_peer, msg.clone());
        if let RelayAction::Dropped { reason } = action {
            if reason.contains("rate limited") {
                rate_limited_count += 1;
            }
        }
    }

    // Token bucket starts with some capacity (2x rate = 4 tokens for 2 msgs/sec)
    // So first ~4 messages may succeed, then rate limiting kicks in
    // We expect at least some messages to be rate limited
    assert!(
        rate_limited_count >= 4,
        "Rate limiter should block burst traffic, blocked {} of 20",
        rate_limited_count
    );
}

#[test]
fn test_multi_hop_relay_chain() {
    let mut hop1_state = RelayState::new(RelayStateConfig::default());
    let mut hop2_state = RelayState::new(RelayStateConfig::default());
    let mut hop3_state = RelayState::new(RelayStateConfig::default());

    let hop1_handler = RelayHandler::new();
    let hop2_handler = RelayHandler::new();
    let hop3_handler = RelayHandler::new();

    let circuit_id = CircuitId::random(&mut rand::thread_rng());
    let key1 = SymmetricKey::random(&mut rand::thread_rng());
    let key2 = SymmetricKey::random(&mut rand::thread_rng());
    let key3 = SymmetricKey::random(&mut rand::thread_rng());

    let hop2_peer = PeerId::random();
    let hop3_peer = PeerId::random();

    hop1_state.add_circuit_key(
        circuit_id,
        CircuitHopKey::new_forward(key1.duplicate(), hop2_peer),
    );
    hop2_state.add_circuit_key(
        circuit_id,
        CircuitHopKey::new_forward(key2.duplicate(), hop3_peer),
    );
    hop3_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key3.duplicate()));

    let tx_data = b"test transaction payload".to_vec();
    let tx_hash = [42u8; 32];
    let inner = InnerMessage::Transaction {
        tx_data: tx_data.clone(),
        tx_hash,
    };
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();

    let hops = [PeerId::random(), hop2_peer, hop3_peer];
    let keys = [key1.duplicate(), key2.duplicate(), key3.duplicate()];
    let wrapped = wrap_onion(&inner_bytes, &hops, &keys);

    let msg1 = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: wrapped,
    };

    let sender = PeerId::random();
    let action1 = hop1_handler.handle_message(&mut hop1_state, &sender, msg1);

    let msg2 = match action1 {
        RelayAction::Forward { next_hop, message } => {
            assert_eq!(next_hop, hop2_peer);
            message
        }
        other => panic!("Expected Forward at hop 1, got {:?}", other),
    };

    let action2 = hop2_handler.handle_message(&mut hop2_state, &hop2_peer, msg2);

    let msg3 = match action2 {
        RelayAction::Forward { next_hop, message } => {
            assert_eq!(next_hop, hop3_peer);
            message
        }
        other => panic!("Expected Forward at hop 2, got {:?}", other),
    };

    let action3 = hop3_handler.handle_message(&mut hop3_state, &hop3_peer, msg3);

    match action3 {
        RelayAction::Exit { inner } => match inner {
            InnerMessage::Transaction {
                tx_data: td,
                tx_hash: th,
            } => {
                assert_eq!(td, tx_data);
                assert_eq!(th, tx_hash);
            }
            other => panic!("Expected Transaction, got {:?}", other),
        },
        other => panic!("Expected Exit at hop 3, got {:?}", other),
    }

    assert_eq!(hop1_handler.metrics().snapshot().messages_forwarded, 1);
    assert_eq!(hop2_handler.metrics().snapshot().messages_forwarded, 1);
    assert_eq!(hop3_handler.metrics().snapshot().messages_exited, 1);
}

#[test]
fn test_malformed_message_handling() {
    let mut relay_state = RelayState::new(RelayStateConfig::default());
    let handler = RelayHandler::new();

    let circuit_id = CircuitId::random(&mut rand::thread_rng());
    let key = SymmetricKey::random(&mut rand::thread_rng());
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    let test_cases = vec![
        (vec![], "empty payload"),
        (vec![0xDE; 10], "short garbage"),
        (vec![0xAB; 1000], "long garbage"),
        (vec![0x00; 100], "zeros"),
    ];

    for (payload, desc) in test_cases {
        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload,
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Dropped { reason } => {
                assert!(
                    reason.contains("decryption failed"),
                    "Malformed {} should cause decryption failure",
                    desc
                );
            }
            other => panic!("Expected Dropped for {}, got {:?}", desc, other),
        }
    }

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.decryption_failures, 4);
}

#[test]
fn test_load_distribution() {
    let network = PrivacyTestNetwork::new(30);
    let selector = CircuitSelector::new(SelectionConfig::default());
    let relay_peers = network.relay_peers();

    let mut peer_usage: HashMap<PeerId, u32> = HashMap::new();

    for _ in 0..100 {
        if let Ok(hops) = selector.select_diverse_hops(&relay_peers, 3) {
            for hop in hops {
                *peer_usage.entry(hop).or_insert(0) += 1;
            }
        }
    }

    let usages: Vec<_> = peer_usage.values().cloned().collect();
    let total: u32 = usages.iter().sum();
    let max_usage = *usages.iter().max().unwrap_or(&0);

    let max_allowed = total / 5;
    assert!(
        max_usage <= max_allowed,
        "Load distribution too skewed: max={}, allowed={}",
        max_usage,
        max_allowed
    );

    assert!(
        peer_usage.len() >= 20,
        "At least 20 of 30 peers should be used, got {}",
        peer_usage.len()
    );
}

// ============================================================================
// Adversarial Tests
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
enum HopPosition {
    Entry,
    Middle,
    Exit,
    #[allow(dead_code)]
    Unknown,
}

struct Adversary {
    controlled_peers: HashSet<PeerId>,
    observations: HashMap<PeerId, Vec<([u8; 32], HopPosition)>>,
    attributions: HashMap<[u8; 32], PeerId>,
}

impl Adversary {
    fn new(controlled_peers: HashSet<PeerId>) -> Self {
        Self {
            controlled_peers,
            observations: HashMap::new(),
            attributions: HashMap::new(),
        }
    }

    fn observe(&mut self, tx_hash: [u8; 32], at_peer: PeerId, position: HopPosition) {
        if self.controlled_peers.contains(&at_peer) {
            self.observations
                .entry(at_peer)
                .or_insert_with(Vec::new)
                .push((tx_hash, position));
        }
    }

    fn attempt_attribution(&mut self, tx_hash: [u8; 32], sender: PeerId) {
        for observations in self.observations.values() {
            for (hash, position) in observations {
                if *hash == tx_hash && *position == HopPosition::Entry {
                    self.attributions.insert(tx_hash, sender);
                    return;
                }
            }
        }
    }

    fn attribution_correct(&self, tx_hash: &[u8; 32], actual_origin: PeerId) -> bool {
        self.attributions
            .get(tx_hash)
            .map(|guessed| *guessed == actual_origin)
            .unwrap_or(false)
    }
}

#[test]
fn test_adversarial_deanonymization_10_percent() {
    let mut network = PrivacyTestNetwork::with_adversaries(100, 10);

    let adversary_peers: HashSet<_> = network
        .peers
        .iter()
        .filter(|p| p.is_adversary)
        .map(|p| p.peer_id)
        .collect();

    let mut adversary = Adversary::new(adversary_peers.clone());

    let mut attributions_attempted = 0;
    let mut correct_attributions = 0;

    let honest_indices: Vec<_> = network
        .peers
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.is_adversary)
        .map(|(i, _)| i)
        .collect();

    let mut rng = rand::thread_rng();

    for _ in 0..500 {
        let sender_idx = honest_indices[rng.gen_range(0..honest_indices.len())];
        let sender_peer_id = network.peers[sender_idx].peer_id;

        if let Ok(circuit_idx) = network.build_circuit(sender_idx) {
            let tx_hash = random_tx_hash();
            let _ = network.send_transaction(circuit_idx, tx_hash);

            let circuit = &network.circuits[circuit_idx].circuit;
            let hops = circuit.hops();

            for (i, hop) in hops.iter().enumerate() {
                if adversary_peers.contains(hop) {
                    let position = match i {
                        0 => HopPosition::Entry,
                        1 => HopPosition::Middle,
                        2 => HopPosition::Exit,
                        _ => continue,
                    };

                    adversary.observe(tx_hash, *hop, position);

                    if i == 0 {
                        adversary.attempt_attribution(tx_hash, sender_peer_id);
                        attributions_attempted += 1;

                        if adversary.attribution_correct(&tx_hash, sender_peer_id) {
                            correct_attributions += 1;
                        }
                    }
                }
            }
        }
    }

    let overall_rate = correct_attributions as f64 / 500.0;
    assert!(
        overall_rate < 0.15,
        "Overall attribution rate too high: {:.2}%",
        overall_rate * 100.0
    );
}

#[test]
fn test_sybil_resistance_subnet_diversity() {
    let mut network = PrivacyTestNetwork::with_subnets(vec![
        ((10, 0), 20),
        ((192, 168), 5),
        ((172, 16), 5),
        ((10, 1), 5),
    ]);

    for i in 0..20 {
        network.peers[i].is_adversary = true;
    }

    let selector = CircuitSelector::new(SelectionConfig {
        strict_diversity: true,
        ..Default::default()
    });

    let all_peers = network.relay_peers();
    let adversary_ids: HashSet<_> = network
        .peers
        .iter()
        .filter(|p| p.is_adversary)
        .map(|p| p.peer_id)
        .collect();

    let mut adversary_hop_count = 0;

    for _ in 0..100 {
        if let Ok(hops) = selector.select_diverse_hops(&all_peers, 3) {
            adversary_hop_count += hops.iter().filter(|h| adversary_ids.contains(h)).count();
        }
    }

    let avg_adversary_per_circuit = adversary_hop_count as f64 / 100.0;
    assert!(
        avg_adversary_per_circuit <= 1.0,
        "Subnet diversity should limit adversary to 1 hop per circuit, got avg {:.2}",
        avg_adversary_per_circuit
    );
}

#[test]
fn test_correlation_resistance() {
    let network = PrivacyTestNetwork::new(50);
    let selector = CircuitSelector::new(SelectionConfig::default());
    let relay_peers = network.relay_peers();

    let mut circuits: Vec<Vec<PeerId>> = Vec::new();

    for _ in 0..100 {
        if let Ok(hops) = selector.select_diverse_hops(&relay_peers, 3) {
            circuits.push(hops);
        }
    }

    let unique_circuits: HashSet<_> = circuits.iter().collect();
    let diversity_ratio = unique_circuits.len() as f64 / circuits.len() as f64;

    assert!(
        diversity_ratio > 0.8,
        "Circuit diversity too low: {:.2}%",
        diversity_ratio * 100.0
    );

    let mut hop_frequency: HashMap<PeerId, u32> = HashMap::new();
    for circuit in &circuits {
        for hop in circuit {
            *hop_frequency.entry(*hop).or_insert(0) += 1;
        }
    }

    let max_frequency = *hop_frequency.values().max().unwrap_or(&0);
    let total_hops = circuits.len() * 3;
    let max_concentration = max_frequency as f64 / total_hops as f64;

    assert!(
        max_concentration < 0.15,
        "Hop concentration too high: {:.2}%",
        max_concentration * 100.0
    );
}

#[test]
fn test_exit_node_origin_hiding() {
    let network = PrivacyTestNetwork::new(30);
    let selector = CircuitSelector::new(SelectionConfig::default());
    let relay_peers = network.relay_peers();

    let mut exit_by_origin: HashMap<usize, HashSet<PeerId>> = HashMap::new();

    for origin_idx in 0..10 {
        let mut exits = HashSet::new();

        for _ in 0..20 {
            if let Ok(hops) = selector.select_diverse_hops(&relay_peers, 3) {
                exits.insert(hops[2]);
            }
        }

        exit_by_origin.insert(origin_idx, exits);
    }

    for (origin, exits) in &exit_by_origin {
        assert!(
            exits.len() > 5,
            "Origin {} should use multiple exits, got {}",
            origin,
            exits.len()
        );
    }

    let all_exits: HashSet<_> = exit_by_origin.values().flatten().collect();
    let mut shared_exits = 0;

    for exit in all_exits {
        let origins_using_exit = exit_by_origin
            .values()
            .filter(|exits| exits.contains(exit))
            .count();
        if origins_using_exit > 1 {
            shared_exits += 1;
        }
    }

    assert!(shared_exits > 0, "Exits should be shared between origins");
}

#[test]
fn test_first_hop_timing_resistance() {
    let mut network = PrivacyTestNetwork::with_adversaries(50, 10);

    let adversary_peers: HashSet<_> = network
        .peers
        .iter()
        .filter(|p| p.is_adversary)
        .map(|p| p.peer_id)
        .collect();

    let mut adversary_entry_count = 0;
    let mut total_circuits = 0;

    let honest_indices: Vec<_> = (10..50).collect();
    let mut rng = rand::thread_rng();

    for _ in 0..200 {
        let origin_idx = honest_indices[rng.gen_range(0..honest_indices.len())];

        if let Ok(circuit_idx) = network.build_circuit(origin_idx) {
            total_circuits += 1;
            let entry_hop = network.circuits[circuit_idx].circuit.entry_hop();
            if adversary_peers.contains(entry_hop) {
                adversary_entry_count += 1;
            }
        }
    }

    let entry_control_rate = adversary_entry_count as f64 / total_circuits as f64;

    assert!(
        entry_control_rate > 0.10 && entry_control_rate < 0.30,
        "Entry control rate should be ~20%, got {:.1}%",
        entry_control_rate * 100.0
    );
}

// ============================================================================
// Test Utility Tests
// ============================================================================

#[test]
fn test_network_creation() {
    let network = PrivacyTestNetwork::new(20);
    assert_eq!(network.peers.len(), 20);
    assert_eq!(network.peer_index.len(), 20);
}

#[test]
fn test_network_with_subnets() {
    let network =
        PrivacyTestNetwork::with_subnets(vec![((192, 168), 5), ((10, 0), 5), ((172, 16), 5)]);
    assert_eq!(network.peers.len(), 15);
}

#[test]
fn test_adversary_network() {
    let network = PrivacyTestNetwork::with_adversaries(100, 10);
    assert_eq!(network.peers.len(), 100);
    assert_eq!(network.adversary_count(), 10);
    assert_eq!(network.honest_peer_count(), 90);
}

#[test]
fn test_simulated_peer() {
    let peer = SimulatedPeer::new(Ipv4Addr::new(10, 0, 0, 1));
    assert!(!peer.is_adversary);

    let tx_hash = random_tx_hash();
    assert!(!peer.has_observed(&tx_hash));
    peer.observe_tx(tx_hash);
    assert!(peer.has_observed(&tx_hash));

    assert_eq!(peer.relay_count(), 0);
    peer.record_relay();
    peer.record_relay();
    assert_eq!(peer.relay_count(), 2);
}
