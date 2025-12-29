// Copyright (c) 2018-2022 The Botho Foundation

//! NetworkState implementation that polls nodes for their current state and is
//! not part of consensus. This is currently implemented by faking SCP messages
//! and utilizing SCPNetworkState.

use crate::{NetworkState, SCPNetworkState};
use bth_blockchain_types::BlockIndex;
use bth_common::{
    logger::{log, Logger},
    ResponderId,
};
use bth_connection::{
    BlockInfo, BlockchainConnection, Connection, ConnectionManager, RetryableBlockchainConnection,
};
use bth_consensus_scp::{ballot::Ballot, msg::ExternalizePayload, Msg, QuorumSet, SlotIndex, Topic};
use bth_util_uri::ConnectionUri;
use retry::delay::{jitter, Fibonacci};
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

// Since PollingNetworkState is not a full-fledged consensus node, it does not
// have a local node id. However, quorum tests inside the scp crate require a
// local node id, so we provide one. Ideally this should not be a node id that
// can be used on a real network.
const FAKE_NODE_ID: &str = "fake:7777";

pub struct PollingNetworkState<BC: BlockchainConnection> {
    /// Connection manager (for consensus nodes we are going to poll).
    manager: ConnectionManager<BC>,

    /// SCPNetworkState instance that provides the actual blocking/quorum set
    /// check logic.
    scp_network_state: SCPNetworkState<ResponderId>,

    /// Last block info objects, per responder id
    block_infos: HashMap<ResponderId, BlockInfo>,

    /// Logger.
    logger: Logger,
}

impl<BC: BlockchainConnection + 'static> PollingNetworkState<BC> {
    pub fn new(
        quorum_set: QuorumSet<ResponderId>,
        manager: ConnectionManager<BC>,
        logger: Logger,
    ) -> Self {
        // Since we want to re-use the findQuorum method on our QuorumSet object,
        // fabricate a message map based on the current block indexes we're
        // aware of.
        let local_node_id = ResponderId::from_str(FAKE_NODE_ID).unwrap();

        Self {
            manager,
            scp_network_state: SCPNetworkState::new(local_node_id, quorum_set),
            block_infos: Default::default(),
            logger,
        }
    }

    /// Polls peers to find out the current state of the network.
    pub fn poll(&mut self) {
        type ResultsMap = HashMap<ResponderId, Option<BlockInfo>>;
        let results_and_condvar = Arc::new((Mutex::new(ResultsMap::default()), Condvar::new()));

        for conn in self.manager.conns() {
            // Create a new ResponderId out of the uri's host and port. This allows us to
            // distinguish between individual nodes that share the same "canonical"
            // ResponderId.
            //
            // Note: this is a hack that allows us to  use a ResponderId in the way that
            // we'd use a NodeID. While it'd be better to change SCPNetworkState
            // to use NodeID, this is a huge undertaking due to tech debt.
            let responder_id = conn
                .uri()
                .host_and_port_responder_id()
                .expect("Could not get host and port responder_id from URI");

            let thread_logger = self.logger.clone();
            let thread_results_and_condvar = results_and_condvar.clone();
            thread::Builder::new()
                .name(format!("Poll:{responder_id}"))
                .spawn(move || {
                    log::debug!(thread_logger, "Getting last block from {}", conn);

                    let (lock, condvar) = &*thread_results_and_condvar;

                    let block_info_result = conn.fetch_block_info(Self::get_retry_iterator());

                    let mut results = lock.lock().expect("mutex poisoned");

                    match &block_info_result {
                        Ok(info) => {
                            log::debug!(
                                thread_logger,
                                "Last block reported by {}: {}",
                                conn,
                                info.block_index
                            );
                            results.insert(responder_id.clone(), Some(info.clone()));
                        }
                        Err(err) => {
                            log::error!(
                                thread_logger,
                                "Failed getting block info from {}: {:?}",
                                conn,
                                err
                            );
                            results.insert(responder_id.clone(), None);
                        }
                    }
                    condvar.notify_one();
                })
                .expect("Failed spawning polling thread!");
        }

        // Wait until we get all results.
        let (lock, condvar) = &*results_and_condvar;
        let num_peers = self.manager.len();
        let results = condvar //.wait(lock.lock().unwrap()).unwrap();
            .wait_while(lock.lock().unwrap(), |ref mut results| {
                results.len() < num_peers
            })
            .expect("waiting on condvar failed");

        log::debug!(
            self.logger,
            "Polling finished, current results: {:?}",
            results
        );

        // Hackishly feed into SCPNetworkState
        for (responder_id, block_info) in results.iter() {
            if let Some(block_info) = block_info.as_ref() {
                self.scp_network_state.push(Msg::<&str, ResponderId>::new(
                    responder_id.clone(),
                    QuorumSet::empty(),
                    block_info.block_index as SlotIndex,
                    Topic::Externalize(ExternalizePayload {
                        C: Ballot::new(1, &["fake"]),
                        HN: 1,
                    }),
                ));
                self.block_infos
                    .insert(responder_id.clone(), block_info.clone());
            }
        }
    }

    pub fn peer_to_current_block_index(&self) -> &HashMap<ResponderId, BlockIndex> {
        self.scp_network_state.peer_to_current_slot()
    }

    pub fn peer_to_block_info(&self) -> &HashMap<ResponderId, BlockInfo> {
        &self.block_infos
    }

    fn get_retry_iterator() -> Box<dyn Iterator<Item = Duration>> {
        // Start at 50ms, make 10 attempts (total would be 7150ms)
        Box::new(Fibonacci::from_millis(50).take(10).map(jitter))
    }
}

impl<BC: BlockchainConnection> NetworkState for PollingNetworkState<BC> {
    /// Returns true if `connections` forms a blocking set for this node and, if
    /// the local node is included, a quorum.
    ///
    /// # Arguments
    /// * `responder_ids` - IDs of other nodes.
    fn is_blocking_and_quorum(&self, conn_ids: &HashSet<ResponderId>) -> bool {
        self.scp_network_state.is_blocking_and_quorum(conn_ids)
    }

    /// Returns true if the local node has "fallen behind its peers" and should
    /// attempt to sync.
    ///
    /// # Arguments
    /// * `local_block_index` - The highest block externalized by this node.
    fn is_behind(&self, local_block_index: BlockIndex) -> bool {
        self.scp_network_state.is_behind(local_block_index)
    }

    /// Returns the highest block index the network agrees on (the highest block
    /// index from a set of peers that passes the "is blocking nad quorum"
    /// test).
    fn highest_block_index_on_network(&self) -> Option<BlockIndex> {
        self.scp_network_state.highest_block_index_on_network()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_common::logger::{test_with_logger, Logger};
    use bth_connection::ConnectionManager;
    use bth_consensus_scp::QuorumSet;
    use bth_ledger_db::test_utils::get_mock_ledger;
    use bth_peers_test_utils::{test_node_id, test_peer_uri, MockPeerConnection};

    #[test_with_logger]
    fn test_new_creates_empty_state(logger: Logger) {
        let node_a = test_node_id(1);
        let node_b = test_node_id(2);
        let quorum_set: QuorumSet<ResponderId> =
            QuorumSet::new_with_node_ids(2, vec![node_a.responder_id, node_b.responder_id]);

        let conn_manager = ConnectionManager::<MockPeerConnection>::new(vec![], logger.clone());

        let network_state = PollingNetworkState::new(quorum_set, conn_manager, logger);

        // Initially, peer_to_current_block_index should be empty
        assert!(network_state.peer_to_current_block_index().is_empty());
        assert!(network_state.peer_to_block_info().is_empty());
    }

    #[test_with_logger]
    fn test_poll_updates_network_state(logger: Logger) {
        let local_node_id = test_node_id(123);

        // Create peers with mock ledgers at different block heights
        let ledger_10 = get_mock_ledger(10);
        let ledger_20 = get_mock_ledger(20);

        let peer_a = MockPeerConnection::new(
            test_peer_uri(1),
            local_node_id.clone(),
            ledger_10,
            50, // 50ms latency
        );
        let peer_b = MockPeerConnection::new(test_peer_uri(2), local_node_id, ledger_20, 50);

        let quorum_set: QuorumSet<ResponderId> = QuorumSet::new_with_node_ids(
            2,
            vec![
                test_peer_uri(1).responder_id().unwrap(),
                test_peer_uri(2).responder_id().unwrap(),
            ],
        );

        let conn_manager = ConnectionManager::new(vec![peer_a, peer_b], logger.clone());

        let mut network_state = PollingNetworkState::new(quorum_set, conn_manager, logger);

        // Initially empty
        assert!(network_state.peer_to_current_block_index().is_empty());

        // Poll the network
        network_state.poll();

        // After polling, we should have block info from both peers
        let block_info = network_state.peer_to_block_info();
        assert_eq!(block_info.len(), 2);

        // peer_to_current_block_index should also be populated
        let block_indexes = network_state.peer_to_current_block_index();
        assert_eq!(block_indexes.len(), 2);
    }

    #[test_with_logger]
    fn test_is_behind_delegates_to_scp_network_state(logger: Logger) {
        let local_node_id = test_node_id(123);

        // Create two peers at block height 10
        let ledger = get_mock_ledger(10);

        let peer_a =
            MockPeerConnection::new(test_peer_uri(1), local_node_id.clone(), ledger.clone(), 10);
        let peer_b = MockPeerConnection::new(test_peer_uri(2), local_node_id, ledger, 10);

        let quorum_set: QuorumSet<ResponderId> = QuorumSet::new_with_node_ids(
            2,
            vec![
                test_peer_uri(1).responder_id().unwrap(),
                test_peer_uri(2).responder_id().unwrap(),
            ],
        );

        let conn_manager = ConnectionManager::new(vec![peer_a, peer_b], logger.clone());

        let mut network_state = PollingNetworkState::new(quorum_set, conn_manager, logger);

        // Poll to populate the state
        network_state.poll();

        // Local block at 5 should be behind when network is at 9 (10 blocks, 0-indexed)
        assert!(network_state.is_behind(5));

        // Local block at 9 should not be behind
        assert!(!network_state.is_behind(9));
    }

    #[test_with_logger]
    fn test_highest_block_index_on_network(logger: Logger) {
        let local_node_id = test_node_id(123);

        // Create two peers at same block height
        let ledger = get_mock_ledger(15);

        let peer_a =
            MockPeerConnection::new(test_peer_uri(1), local_node_id.clone(), ledger.clone(), 10);
        let peer_b = MockPeerConnection::new(test_peer_uri(2), local_node_id, ledger, 10);

        let quorum_set: QuorumSet<ResponderId> = QuorumSet::new_with_node_ids(
            2,
            vec![
                test_peer_uri(1).responder_id().unwrap(),
                test_peer_uri(2).responder_id().unwrap(),
            ],
        );

        let conn_manager = ConnectionManager::new(vec![peer_a, peer_b], logger.clone());

        let mut network_state = PollingNetworkState::new(quorum_set, conn_manager, logger);

        // Initially None because no polling has occurred
        assert_eq!(network_state.highest_block_index_on_network(), None);

        // Poll to populate the state
        network_state.poll();

        // Both peers are at block 14 (15 blocks, 0-indexed), so network agrees on 14
        assert_eq!(network_state.highest_block_index_on_network(), Some(14));
    }

    #[test_with_logger]
    fn test_is_blocking_and_quorum(logger: Logger) {
        let node_a_responder = test_peer_uri(1).responder_id().unwrap();
        let node_b_responder = test_peer_uri(2).responder_id().unwrap();

        let quorum_set: QuorumSet<ResponderId> = QuorumSet::new_with_node_ids(
            2,
            vec![node_a_responder.clone(), node_b_responder.clone()],
        );

        let conn_manager = ConnectionManager::<MockPeerConnection>::new(vec![], logger.clone());

        let network_state = PollingNetworkState::new(quorum_set, conn_manager, logger);

        // Empty set is not blocking and quorum
        assert!(!network_state.is_blocking_and_quorum(&HashSet::new()));

        // Single node is not enough (threshold is 2)
        let single_node: HashSet<ResponderId> =
            vec![node_a_responder.clone()].into_iter().collect();
        assert!(!network_state.is_blocking_and_quorum(&single_node));

        // Both nodes together should be blocking and quorum
        let both_nodes: HashSet<ResponderId> = vec![node_a_responder, node_b_responder]
            .into_iter()
            .collect();
        assert!(network_state.is_blocking_and_quorum(&both_nodes));
    }

    // Note: Slow peer timeout test removed - it took too long to run due to
    // actual network polling with retries. The timeout behavior is adequately
    // tested by the existing tests in ledger_sync_service.
}
