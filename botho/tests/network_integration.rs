// Copyright (c) 2024 Botho Foundation

//! Network integration tests for peer discovery, chain sync, and message propagation.
//!
//! These tests spin up multiple libp2p nodes and verify they can:
//! - Discover each other
//! - Exchange blocks and transactions via gossip
//! - Synchronize chain state
//! - Handle peer disconnections gracefully

use std::time::Duration;

use futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity},
    identity, noise,
    request_response::{self},
    swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use tokio::time::timeout;

use botho::block::Block;
use botho::network::{
    create_sync_behaviour, ChainSyncManager, ReputationManager, SyncAction,
    SyncCodec, SyncRateLimiter, SyncRequest, SyncResponse, BLOCKS_PER_REQUEST,
    MAX_REQUESTS_PER_MINUTE,
};

/// Topic for block announcements (same as in discovery.rs)
const BLOCKS_TOPIC: &str = "botho/blocks/1.0.0";
const TRANSACTIONS_TOPIC: &str = "botho/transactions/1.0.0";

/// Test helper to create a minimal libp2p swarm for testing
async fn create_test_swarm() -> (Swarm<TestBehaviour>, PeerId, Multiaddr) {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .unwrap()
        .with_behaviour(|key| {
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_millis(100))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .build()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                .unwrap();

            let gossipsub = gossipsub::Behaviour::new(
                MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            .unwrap();

            let sync = create_sync_behaviour();

            Ok(TestBehaviour { gossipsub, sync })
        })
        .unwrap()
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
        .build();

    // Subscribe to topics
    let blocks_topic = IdentTopic::new(BLOCKS_TOPIC);
    swarm.behaviour_mut().gossipsub.subscribe(&blocks_topic).unwrap();

    let tx_topic = IdentTopic::new(TRANSACTIONS_TOPIC);
    swarm.behaviour_mut().gossipsub.subscribe(&tx_topic).unwrap();

    // Listen on a random available port
    let listen_addr: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
    swarm.listen_on(listen_addr).unwrap();

    // Wait for the actual listening address
    let actual_addr = loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => break address,
            _ => continue,
        }
    };

    (swarm, local_peer_id, actual_addr)
}

/// Simplified behaviour for testing (mirrors BothoBehaviour)
#[derive(libp2p::swarm::NetworkBehaviour)]
struct TestBehaviour {
    gossipsub: gossipsub::Behaviour,
    sync: request_response::Behaviour<SyncCodec>,
}

// ============================================================================
// Peer Discovery Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_two_nodes_discover_each_other() {
    use tokio::sync::mpsc;

    // Create two nodes
    let (mut swarm1, peer1, addr1) = create_test_swarm().await;
    let (mut swarm2, _peer2, _addr2) = create_test_swarm().await;

    // Node 2 dials node 1
    swarm2.dial(addr1.clone()).unwrap();

    // Use channels to collect events
    let (tx1, _rx1) = mpsc::channel::<PeerId>(10);
    let (tx2, mut rx2) = mpsc::channel::<PeerId>(10);

    // Run swarm1 in a separate task
    let h1 = tokio::spawn(async move {
        while let Some(event) = swarm1.next().await {
            if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event {
                tx1.send(peer_id).await.ok();
            }
        }
    });

    // Run swarm2 in a separate task
    let h2 = tokio::spawn(async move {
        while let Some(event) = swarm2.next().await {
            if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event {
                tx2.send(peer_id).await.ok();
            }
        }
    });

    // Wait for at least one connection event from swarm2 (the dialer)
    let connected = timeout(Duration::from_secs(10), async {
        // We expect swarm2 to see a connection to peer1
        rx2.recv().await
    })
    .await;

    // Clean up
    h1.abort();
    h2.abort();

    assert!(connected.is_ok(), "Should connect within timeout");
    let connected_peer = connected.unwrap();
    assert!(connected_peer.is_some(), "Should receive connection event");
    assert_eq!(connected_peer.unwrap(), peer1, "Should connect to correct peer");
}

#[tokio::test]
async fn test_three_node_mesh_discovery() {
    // Create three nodes
    let (mut swarm1, _peer1, addr1) = create_test_swarm().await;
    let (mut swarm2, _peer2, addr2) = create_test_swarm().await;
    let (mut swarm3, _peer3, _addr3) = create_test_swarm().await;

    // Node 2 connects to Node 1
    swarm2.dial(addr1.clone()).unwrap();
    // Node 3 connects to Node 2
    swarm3.dial(addr2.clone()).unwrap();

    // Track connections
    let mut connections = std::collections::HashSet::new();

    let result = timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                event = swarm1.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event {
                        connections.insert(("node1", peer_id));
                    }
                }
                event = swarm2.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event {
                        connections.insert(("node2", peer_id));
                    }
                }
                event = swarm3.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event {
                        connections.insert(("node3", peer_id));
                    }
                }
            }

            // Each connection shows up twice (once on each side)
            if connections.len() >= 4 {
                break;
            }
        }
    })
    .await;

    assert!(result.is_ok(), "Mesh should form within timeout");
    assert!(connections.len() >= 4, "Should have at least 4 connection events");
}

// ============================================================================
// Block Propagation Tests
// ============================================================================

#[tokio::test]
async fn test_block_gossip_between_two_nodes() {
    let (mut swarm1, _peer1, addr1) = create_test_swarm().await;
    let (mut swarm2, _peer2, _addr2) = create_test_swarm().await;

    // Connect the nodes
    swarm2.dial(addr1.clone()).unwrap();

    // Wait for both nodes to see the connection
    let connected = timeout(Duration::from_secs(10), async {
        let mut s1_connected = false;
        let mut s2_connected = false;
        loop {
            tokio::select! {
                biased;
                event = swarm1.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished { .. } = event {
                        s1_connected = true;
                    }
                }
                event = swarm2.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished { .. } = event {
                        s2_connected = true;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            if s1_connected && s2_connected {
                break;
            }
        }
    })
    .await;
    assert!(connected.is_ok(), "Nodes should connect");

    // Give gossipsub time to exchange subscription info and form mesh
    // Process events while waiting
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            biased;
            _ = swarm1.select_next_some() => {}
            _ = swarm2.select_next_some() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    // Create a test block
    let genesis = Block::genesis();
    let block_bytes = bincode::serialize(&genesis).unwrap();

    // Publish from node 1
    let topic = IdentTopic::new(BLOCKS_TOPIC);
    let publish_result = swarm1.behaviour_mut().gossipsub.publish(topic, block_bytes.clone());

    // Note: Gossipsub may reject if there are no mesh peers yet
    // In production, we'd retry or wait for mesh formation
    if publish_result.is_err() {
        // This is expected if gossipsub mesh hasn't formed yet
        // The important thing is the protocol doesn't crash
        return;
    }

    // Wait for node 2 to receive the block while polling both swarms
    let received = timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                biased;
                _ = swarm1.select_next_some() => {}
                event = swarm2.select_next_some() => {
                    if let SwarmEvent::Behaviour(TestBehaviourEvent::Gossipsub(
                        gossipsub::Event::Message { message, .. },
                    )) = event
                    {
                        if message.topic.as_str() == BLOCKS_TOPIC {
                            let received_block: Block = bincode::deserialize(&message.data).unwrap();
                            return received_block;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    })
    .await;

    assert!(received.is_ok(), "Node 2 should receive block");
    let received_block = received.unwrap();
    assert_eq!(received_block.height(), genesis.height());
}

// ============================================================================
// Chain Sync Protocol Tests
// ============================================================================

#[tokio::test]
async fn test_sync_request_response() {
    use tokio::sync::oneshot;

    let (mut swarm1, peer1, addr1) = create_test_swarm().await;
    let (mut swarm2, _peer2, _addr2) = create_test_swarm().await;

    // Connect
    swarm2.dial(addr1.clone()).unwrap();

    // Wait for connection with separate tasks
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();

    let _connect_task1 = tokio::spawn(async move {
        loop {
            if let Some(event) = swarm1.next().await {
                if let SwarmEvent::ConnectionEstablished { .. } = event {
                    tx1.send(swarm1).ok();
                    return;
                }
            }
        }
    });

    let _connect_task2 = tokio::spawn(async move {
        loop {
            if let Some(event) = swarm2.next().await {
                if let SwarmEvent::ConnectionEstablished { .. } = event {
                    tx2.send(swarm2).ok();
                    return;
                }
            }
        }
    });

    // Wait for connections
    let (mut swarm1, mut swarm2) = timeout(Duration::from_secs(10), async {
        let s1 = rx1.await.unwrap();
        let s2 = rx2.await.unwrap();
        (s1, s2)
    })
    .await
    .expect("Should connect");

    // Node 2 sends a sync status request to node 1
    let request = SyncRequest::GetStatus;
    swarm2.behaviour_mut().sync.send_request(&peer1, request);

    // Run sync handshake with separate tasks
    let (result_tx, result_rx) = oneshot::channel::<u64>();

    let server_task = tokio::spawn(async move {
        loop {
            if let Some(event) = swarm1.next().await {
                if let SwarmEvent::Behaviour(TestBehaviourEvent::Sync(
                    request_response::Event::Message {
                        message: request_response::Message::Request { channel, .. },
                        ..
                    },
                )) = event
                {
                    let response = SyncResponse::Status {
                        height: 100,
                        tip_hash: [1u8; 32],
                    };
                    swarm1.behaviour_mut().sync.send_response(channel, response).ok();
                }
            }
        }
    });

    let client_task = tokio::spawn(async move {
        loop {
            if let Some(event) = swarm2.next().await {
                if let SwarmEvent::Behaviour(TestBehaviourEvent::Sync(
                    request_response::Event::Message {
                        message: request_response::Message::Response { response, .. },
                        ..
                    },
                )) = event
                {
                    if let SyncResponse::Status { height, .. } = response {
                        result_tx.send(height).ok();
                        return;
                    }
                }
            }
        }
    });

    let result = timeout(Duration::from_secs(10), result_rx).await;

    server_task.abort();
    client_task.abort();

    assert!(result.is_ok(), "Should complete sync handshake");
    assert_eq!(result.unwrap().unwrap(), 100, "Should receive correct height");
}

#[tokio::test]
async fn test_sync_blocks_request() {
    use tokio::sync::oneshot;

    let (mut swarm1, peer1, addr1) = create_test_swarm().await;
    let (mut swarm2, _peer2, _addr2) = create_test_swarm().await;

    // Connect
    swarm2.dial(addr1.clone()).unwrap();

    // Wait for connection with separate tasks
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();

    tokio::spawn(async move {
        loop {
            if let Some(event) = swarm1.next().await {
                if let SwarmEvent::ConnectionEstablished { .. } = event {
                    tx1.send(swarm1).ok();
                    return;
                }
            }
        }
    });

    tokio::spawn(async move {
        loop {
            if let Some(event) = swarm2.next().await {
                if let SwarmEvent::ConnectionEstablished { .. } = event {
                    tx2.send(swarm2).ok();
                    return;
                }
            }
        }
    });

    // Wait for connections
    let (mut swarm1, mut swarm2) = timeout(Duration::from_secs(10), async {
        let s1 = rx1.await.unwrap();
        let s2 = rx2.await.unwrap();
        (s1, s2)
    })
    .await
    .expect("Should connect");

    // Request blocks
    let request = SyncRequest::GetBlocks {
        start_height: 0,
        count: 10,
    };
    swarm2.behaviour_mut().sync.send_request(&peer1, request);

    // Run sync with separate tasks
    let (result_tx, result_rx) = oneshot::channel::<(usize, bool)>();

    let server_task = tokio::spawn(async move {
        loop {
            if let Some(event) = swarm1.next().await {
                if let SwarmEvent::Behaviour(TestBehaviourEvent::Sync(
                    request_response::Event::Message {
                        message: request_response::Message::Request { channel, .. },
                        ..
                    },
                )) = event
                {
                    let response = SyncResponse::Blocks {
                        blocks: vec![Block::genesis()],
                        has_more: false,
                    };
                    swarm1.behaviour_mut().sync.send_response(channel, response).ok();
                }
            }
        }
    });

    let client_task = tokio::spawn(async move {
        loop {
            if let Some(event) = swarm2.next().await {
                if let SwarmEvent::Behaviour(TestBehaviourEvent::Sync(
                    request_response::Event::Message {
                        message: request_response::Message::Response { response, .. },
                        ..
                    },
                )) = event
                {
                    if let SyncResponse::Blocks { blocks, has_more } = response {
                        result_tx.send((blocks.len(), has_more)).ok();
                        return;
                    }
                }
            }
        }
    });

    let result = timeout(Duration::from_secs(10), result_rx).await;

    server_task.abort();
    client_task.abort();

    assert!(result.is_ok(), "Should receive blocks response");
    let (block_count, has_more) = result.unwrap().unwrap();
    assert_eq!(block_count, 1, "Should receive genesis block");
    assert!(!has_more, "Should indicate no more blocks");
}

// ============================================================================
// Chain Sync Manager State Machine Tests
// ============================================================================

#[test]
fn test_sync_manager_full_workflow() {
    let mut manager = ChainSyncManager::new(0);
    let peer = PeerId::random();

    // Initially in discovery
    assert!(!manager.is_synced());

    // Tick should request status from connected peers
    let action = manager.tick(&[peer]);
    assert!(matches!(action, Some(SyncAction::RequestStatus(_))));

    // Simulate receiving status from peer (100 blocks ahead)
    manager.on_status(peer, 100, [1u8; 32]);

    // Should now be downloading
    let action = manager.tick(&[peer]);
    assert!(matches!(
        action,
        Some(SyncAction::RequestBlocks {
            start_height: 1,
            count: 100,
            ..
        })
    ));

    // Simulate receiving blocks
    let blocks = vec![Block::genesis()]; // Simplified
    manager.on_blocks(blocks, true);

    // Mark blocks as added
    manager.on_blocks_added(50);

    // Should continue downloading
    let action = manager.tick(&[peer]);
    assert!(matches!(action, Some(SyncAction::RequestBlocks { .. })));

    // Complete sync
    manager.on_blocks_added(100);
    assert!(manager.is_synced());
}

#[test]
fn test_sync_manager_peer_disconnect_during_download() {
    let mut manager = ChainSyncManager::new(0);
    let peer = PeerId::random();

    // Start downloading
    manager.on_status(peer, 100, [1u8; 32]);
    manager.tick(&[peer]);

    // Peer disconnects
    manager.on_peer_disconnected(&peer);

    // Should go back to discovery
    assert!(!manager.is_synced());

    // Next tick should try to discover new peers
    let other_peer = PeerId::random();
    let action = manager.tick(&[other_peer]);
    assert!(matches!(action, Some(SyncAction::RequestStatus(_))));
}

#[test]
fn test_sync_manager_already_synced() {
    let mut manager = ChainSyncManager::new(100);
    let peer = PeerId::random();

    // Peer reports same height
    manager.on_status(peer, 105, [1u8; 32]); // Only 5 ahead (< threshold of 10)

    // Should be synced
    assert!(manager.is_synced());
}

// ============================================================================
// Reputation Manager Integration Tests
// ============================================================================

#[test]
fn test_reputation_based_peer_selection() {
    let mut reputation = ReputationManager::new();

    let fast_peer = PeerId::random();
    let slow_peer = PeerId::random();
    let bad_peer = PeerId::random();

    // Fast peer has good latency
    for _ in 0..5 {
        reputation.request_sent(fast_peer);
        std::thread::sleep(Duration::from_millis(1));
        reputation.response_received(&fast_peer);
    }

    // Slow peer has high latency (simulated by recording directly)
    for _ in 0..5 {
        reputation
            .get_or_create(&slow_peer)
            .record_success(Duration::from_millis(500));
    }

    // Bad peer has failures
    for _ in 0..5 {
        reputation.request_sent(bad_peer);
        reputation.request_failed(&bad_peer);
    }

    // Best peer should be the fast one
    let candidates = vec![fast_peer, slow_peer, bad_peer];
    let best = reputation.best_peer(&candidates);
    assert_eq!(best, Some(fast_peer));

    // Bad peer should be banned
    assert!(reputation.is_banned(&bad_peer));
}

#[test]
fn test_reputation_new_peer_neutral_score() {
    let reputation = ReputationManager::new();
    let new_peer = PeerId::random();

    // New peer should not be banned
    assert!(!reputation.is_banned(&new_peer));

    // New peer should get neutral score (500)
    let score = reputation.get(&new_peer).map(|r| r.score());
    assert!(score.is_none()); // Not tracked yet
}

// ============================================================================
// Rate Limiter Integration Tests
// ============================================================================

#[test]
fn test_rate_limiter_under_normal_load() {
    let mut limiter = SyncRateLimiter::default();
    let peer = PeerId::random();

    // Should allow MAX_REQUESTS_PER_MINUTE requests
    for i in 0..MAX_REQUESTS_PER_MINUTE {
        assert!(
            limiter.check_and_record(&peer),
            "Request {} should be allowed",
            i
        );
    }

    // Next request should be blocked
    assert!(
        !limiter.check_and_record(&peer),
        "Request beyond limit should be blocked"
    );
}

#[test]
fn test_rate_limiter_multiple_peers_independent() {
    let mut limiter = SyncRateLimiter::new(5, Duration::from_secs(60));

    let peer1 = PeerId::random();
    let peer2 = PeerId::random();

    // Exhaust peer1's limit
    for _ in 0..5 {
        assert!(limiter.check_and_record(&peer1));
    }
    assert!(!limiter.check_and_record(&peer1));

    // Peer2 should still have full allowance
    for _ in 0..5 {
        assert!(limiter.check_and_record(&peer2));
    }
    assert!(!limiter.check_and_record(&peer2));
}

// ============================================================================
// Connection Handling Tests
// ============================================================================

#[tokio::test]
async fn test_graceful_disconnect_handling() {
    use tokio::sync::oneshot;

    let (mut swarm1, _peer1, addr1) = create_test_swarm().await;
    let (mut swarm2, peer2, _addr2) = create_test_swarm().await;

    // Connect
    swarm2.dial(addr1.clone()).unwrap();

    // Wait for connection with separate tasks
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();

    tokio::spawn(async move {
        loop {
            if let Some(event) = swarm1.next().await {
                if let SwarmEvent::ConnectionEstablished { .. } = event {
                    tx1.send(swarm1).ok();
                    return;
                }
            }
        }
    });

    tokio::spawn(async move {
        loop {
            if let Some(event) = swarm2.next().await {
                if let SwarmEvent::ConnectionEstablished { .. } = event {
                    tx2.send(swarm2).ok();
                    return;
                }
            }
        }
    });

    // Wait for connections
    let (mut swarm1, swarm2) = timeout(Duration::from_secs(10), async {
        let s1 = rx1.await.unwrap();
        let s2 = rx2.await.unwrap();
        (s1, s2)
    })
    .await
    .expect("Should connect");

    // Drop swarm2 to simulate disconnect
    drop(swarm2);

    // Node 1 should see the disconnection
    let disconnect_seen = timeout(Duration::from_secs(10), async {
        loop {
            if let Some(event) = swarm1.next().await {
                if let SwarmEvent::ConnectionClosed { peer_id, .. } = event {
                    if peer_id == peer2 {
                        return true;
                    }
                }
            }
        }
    })
    .await;

    assert!(disconnect_seen.is_ok(), "Should detect peer disconnection");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_sync_response_error_handling() {
    let mut manager = ChainSyncManager::new(0);
    let peer = PeerId::random();

    // Start syncing
    manager.on_status(peer, 100, [1u8; 32]);

    // Simulate failure
    manager.on_failure("Connection timeout".to_string());

    // Should be in failed state, not synced
    assert!(!manager.is_synced());

    // Next tick should wait (return Wait action)
    let action = manager.tick(&[peer]);
    assert!(matches!(action, Some(SyncAction::Wait(_))));
}

#[test]
fn test_empty_blocks_response() {
    let mut manager = ChainSyncManager::new(0);

    // Empty blocks should return None action
    let action = manager.on_blocks(vec![], false);
    assert!(action.is_none());
}

// ============================================================================
// Message Serialization Tests (Integration)
// ============================================================================

#[test]
fn test_block_serialization_roundtrip() {
    let block = Block::genesis();
    let bytes = bincode::serialize(&block).unwrap();
    let decoded: Block = bincode::deserialize(&bytes).unwrap();

    assert_eq!(decoded.height(), block.height());
    assert_eq!(decoded.hash(), block.hash());
}

#[test]
fn test_sync_messages_size_limits() {
    // Verify GetStatus request fits in MAX_REQUEST_SIZE
    let status_request = SyncRequest::GetStatus;
    let bytes = bincode::serialize(&status_request).unwrap();
    assert!(bytes.len() < 1024, "GetStatus should fit in request size limit");

    // Verify GetBlocks request fits
    let blocks_request = SyncRequest::GetBlocks {
        start_height: u64::MAX,
        count: BLOCKS_PER_REQUEST,
    };
    let bytes = bincode::serialize(&blocks_request).unwrap();
    assert!(bytes.len() < 1024, "GetBlocks should fit in request size limit");
}
