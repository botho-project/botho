// Copyright (c) 2024 Cadence Foundation

//! Topology analyzer for quorum set suggestions.
//!
//! This module analyzes the network topology discovered via gossip and
//! provides suggestions for configuring quorum sets. It helps new nodes
//! join the network by recommending trust configurations based on
//! what other nodes in the network are doing.

use crate::{
    messages::NodeAnnouncement,
    store::SharedPeerStore,
};
use mc_common::{HashMap, HashSet, ResponderId};
use mc_consensus_scp_types::{QuorumSet, QuorumSetMember};
use std::sync::Arc;

/// A cluster of nodes that tend to trust each other.
#[derive(Debug, Clone)]
pub struct TrustCluster {
    /// A descriptive name for this cluster (auto-generated)
    pub name: String,

    /// The nodes in this cluster
    pub members: Vec<ResponderId>,

    /// How strongly connected this cluster is (0.0 - 1.0)
    pub cohesion: f64,

    /// What percentage of the network trusts at least one member
    pub network_coverage: f64,
}

/// A suggestion for a quorum set configuration.
#[derive(Debug, Clone)]
pub struct QuorumSetSuggestion {
    /// The suggested quorum set
    pub quorum_set: QuorumSet<ResponderId>,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,

    /// Description of why this was suggested
    pub rationale: String,

    /// Strategy used to generate this suggestion
    pub strategy: QuorumStrategy,
}

/// Strategies for suggesting quorum sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuorumStrategy {
    /// Trust the top N most popular nodes
    TopN,

    /// Mirror what a specific trusted node uses
    MirrorNode,

    /// Use hierarchical sets based on trust clusters
    Hierarchical,

    /// Conservative: high threshold, well-known nodes only
    Conservative,

    /// Aggressive: lower threshold, broader participation
    Aggressive,
}

/// Statistics about the network topology.
#[derive(Debug, Clone, Default)]
pub struct TopologyStats {
    /// Total number of known nodes
    pub total_nodes: usize,

    /// Number of nodes participating in consensus
    pub consensus_nodes: usize,

    /// Average quorum set size
    pub avg_quorum_set_size: f64,

    /// Average threshold as percentage
    pub avg_threshold_pct: f64,

    /// Number of distinct trust clusters identified
    pub cluster_count: usize,

    /// The most trusted node (by incoming trust count)
    pub most_trusted_node: Option<ResponderId>,

    /// How many nodes trust the most trusted node
    pub max_trust_count: usize,
}

/// Analyzes network topology and suggests quorum configurations.
pub struct TopologyAnalyzer {
    store: SharedPeerStore,
}

impl TopologyAnalyzer {
    /// Create a new topology analyzer.
    pub fn new(store: SharedPeerStore) -> Self {
        Self { store }
    }

    /// Get statistics about the current network topology.
    pub fn stats(&self) -> TopologyStats {
        let announcements = self.store.get_all();
        let consensus_nodes = self.store.get_consensus_nodes();

        if announcements.is_empty() {
            return TopologyStats::default();
        }

        // Calculate average quorum set size and threshold
        let mut total_size = 0usize;
        let mut total_threshold_pct = 0.0f64;
        let mut valid_qs_count = 0usize;

        for ann in &consensus_nodes {
            let size = ann.quorum_set.members.len();
            if size > 0 {
                total_size += size;
                total_threshold_pct += (ann.quorum_set.threshold as f64 / size as f64) * 100.0;
                valid_qs_count += 1;
            }
        }

        // Find most trusted node
        let trust_counts = self.get_trust_counts();
        let most_trusted = trust_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(id, count)| (id.clone(), *count));

        TopologyStats {
            total_nodes: announcements.len(),
            consensus_nodes: consensus_nodes.len(),
            avg_quorum_set_size: if valid_qs_count > 0 {
                total_size as f64 / valid_qs_count as f64
            } else {
                0.0
            },
            avg_threshold_pct: if valid_qs_count > 0 {
                total_threshold_pct / valid_qs_count as f64
            } else {
                0.0
            },
            cluster_count: self.find_trust_clusters().len(),
            most_trusted_node: most_trusted.as_ref().map(|(id, _)| id.clone()),
            max_trust_count: most_trusted.map(|(_, c)| c).unwrap_or(0),
        }
    }

    /// Get a map of how many nodes trust each node.
    pub fn get_trust_counts(&self) -> HashMap<ResponderId, usize> {
        let mut counts: HashMap<ResponderId, usize> = HashMap::default();

        for ann in self.store.get_consensus_nodes() {
            for trusted in ann.quorum_set.nodes() {
                *counts.entry(trusted).or_insert(0) += 1;
            }
        }

        counts
    }

    /// Get nodes sorted by how many other nodes trust them.
    pub fn get_popular_nodes(&self, min_trust_count: usize) -> Vec<(ResponderId, usize)> {
        let mut popular: Vec<_> = self
            .get_trust_counts()
            .into_iter()
            .filter(|(_, count)| *count >= min_trust_count)
            .collect();

        popular.sort_by(|a, b| b.1.cmp(&a.1));
        popular
    }

    /// Find clusters of nodes that mutually trust each other.
    pub fn find_trust_clusters(&self) -> Vec<TrustCluster> {
        let trust_graph = self.store.get_trust_graph();
        let all_nodes: Vec<_> = trust_graph.keys().cloned().collect();

        if all_nodes.is_empty() {
            return vec![];
        }

        // Simple clustering: group nodes that share significant trust overlap
        let mut clusters: Vec<TrustCluster> = vec![];
        let mut assigned: HashSet<ResponderId> = HashSet::default();

        for node in &all_nodes {
            if assigned.contains(node) {
                continue;
            }

            // Start a new cluster with this node
            let mut cluster_members = vec![node.clone()];
            assigned.insert(node.clone());

            // Find nodes that trust similar sets
            if let Some(node_trusts) = trust_graph.get(node) {
                for other in &all_nodes {
                    if assigned.contains(other) || other == node {
                        continue;
                    }

                    if let Some(other_trusts) = trust_graph.get(other) {
                        // Calculate Jaccard similarity
                        let intersection = node_trusts.intersection(other_trusts).count();
                        let union = node_trusts.union(other_trusts).count();

                        if union > 0 {
                            let similarity = intersection as f64 / union as f64;
                            if similarity > 0.5 {
                                // 50% overlap threshold
                                cluster_members.push(other.clone());
                                assigned.insert(other.clone());
                            }
                        }
                    }
                }
            }

            if cluster_members.len() >= 2 {
                let cohesion = self.calculate_cluster_cohesion(&cluster_members, &trust_graph);
                let coverage = cluster_members.len() as f64 / all_nodes.len() as f64;

                clusters.push(TrustCluster {
                    name: format!("cluster-{}", clusters.len() + 1),
                    members: cluster_members,
                    cohesion,
                    network_coverage: coverage,
                });
            }
        }

        // Sort by size
        clusters.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
        clusters
    }

    /// Calculate how cohesive a cluster is (how much members trust each other).
    fn calculate_cluster_cohesion(
        &self,
        members: &[ResponderId],
        trust_graph: &HashMap<ResponderId, HashSet<ResponderId>>,
    ) -> f64 {
        if members.len() < 2 {
            return 1.0;
        }

        let mut trust_count = 0;
        let possible_edges = members.len() * (members.len() - 1);

        for member in members {
            if let Some(trusts) = trust_graph.get(member) {
                for other in members {
                    if member != other && trusts.contains(other) {
                        trust_count += 1;
                    }
                }
            }
        }

        trust_count as f64 / possible_edges as f64
    }

    /// Suggest a quorum set using the specified strategy.
    pub fn suggest_quorum_set(&self, strategy: QuorumStrategy) -> Option<QuorumSetSuggestion> {
        match strategy {
            QuorumStrategy::TopN => self.suggest_top_n(5, 67),
            QuorumStrategy::MirrorNode => {
                // Mirror the most trusted node
                let popular = self.get_popular_nodes(1);
                if let Some((node, _)) = popular.first() {
                    self.suggest_mirror_node(node)
                } else {
                    None
                }
            }
            QuorumStrategy::Hierarchical => self.suggest_hierarchical(),
            QuorumStrategy::Conservative => self.suggest_top_n(3, 100), // All must agree
            QuorumStrategy::Aggressive => self.suggest_top_n(7, 51),    // Simple majority
        }
    }

    /// Suggest a quorum set of the top N most trusted nodes.
    pub fn suggest_top_n(&self, count: usize, threshold_pct: u32) -> Option<QuorumSetSuggestion> {
        let popular = self.get_popular_nodes(1);
        if popular.is_empty() {
            return None;
        }

        let top_nodes: Vec<_> = popular.into_iter().take(count).collect();
        let actual_count = top_nodes.len();
        let threshold = ((actual_count as u32 * threshold_pct) / 100).max(1);

        let members: Vec<QuorumSetMember<ResponderId>> = top_nodes
            .iter()
            .map(|(id, _)| QuorumSetMember::Node(id.clone()))
            .collect();

        let quorum_set = QuorumSet::new(threshold, members);

        let confidence = if actual_count >= count { 0.8 } else { 0.5 };

        Some(QuorumSetSuggestion {
            quorum_set,
            confidence,
            rationale: format!(
                "Top {} most trusted nodes with {}% threshold",
                actual_count, threshold_pct
            ),
            strategy: QuorumStrategy::TopN,
        })
    }

    /// Suggest a quorum set that mirrors what a specific node uses.
    pub fn suggest_mirror_node(&self, node: &ResponderId) -> Option<QuorumSetSuggestion> {
        let announcement = self.store.get(node)?;

        Some(QuorumSetSuggestion {
            quorum_set: announcement.quorum_set.clone(),
            confidence: 0.7,
            rationale: format!("Mirrors quorum set of {}", node),
            strategy: QuorumStrategy::MirrorNode,
        })
    }

    /// Suggest a hierarchical quorum set based on trust clusters.
    pub fn suggest_hierarchical(&self) -> Option<QuorumSetSuggestion> {
        let clusters = self.find_trust_clusters();
        if clusters.is_empty() {
            return self.suggest_top_n(5, 67);
        }

        // Take top 3 clusters and create inner sets for each
        let top_clusters: Vec<_> = clusters.into_iter().take(3).collect();

        let inner_sets: Vec<QuorumSetMember<ResponderId>> = top_clusters
            .iter()
            .map(|cluster| {
                let threshold = ((cluster.members.len() as u32 * 67) / 100).max(1);
                let members: Vec<_> = cluster
                    .members
                    .iter()
                    .map(|id| QuorumSetMember::Node(id.clone()))
                    .collect();
                QuorumSetMember::InnerSet(QuorumSet::new(threshold, members))
            })
            .collect();

        let threshold = ((inner_sets.len() as u32 * 67) / 100).max(1);
        let quorum_set = QuorumSet::new(threshold, inner_sets);

        Some(QuorumSetSuggestion {
            quorum_set,
            confidence: 0.75,
            rationale: format!(
                "Hierarchical quorum set with {} trust clusters",
                top_clusters.len()
            ),
            strategy: QuorumStrategy::Hierarchical,
        })
    }

    /// Get all suggestions using different strategies.
    pub fn get_all_suggestions(&self) -> Vec<QuorumSetSuggestion> {
        let strategies = [
            QuorumStrategy::TopN,
            QuorumStrategy::Hierarchical,
            QuorumStrategy::Conservative,
            QuorumStrategy::Aggressive,
        ];

        strategies
            .into_iter()
            .filter_map(|s| self.suggest_quorum_set(s))
            .collect()
    }

    /// Validate a proposed quorum set against the known network.
    pub fn validate_quorum_set(
        &self,
        quorum_set: &QuorumSet<ResponderId>,
    ) -> QuorumSetValidation {
        let known_nodes: HashSet<_> = self
            .store
            .get_responder_ids()
            .into_iter()
            .collect();

        let qs_nodes = quorum_set.nodes();
        let unknown_nodes: Vec<_> = qs_nodes
            .iter()
            .filter(|n| !known_nodes.contains(*n))
            .cloned()
            .collect();

        let trust_counts = self.get_trust_counts();
        let low_trust_nodes: Vec<_> = qs_nodes
            .iter()
            .filter(|n| trust_counts.get(*n).copied().unwrap_or(0) < 2)
            .cloned()
            .collect();

        let total_members = quorum_set.members.len();
        let threshold_pct = if total_members > 0 {
            (quorum_set.threshold as f64 / total_members as f64) * 100.0
        } else {
            0.0
        };

        let mut warnings = vec![];
        let mut is_valid = true;

        if !unknown_nodes.is_empty() {
            warnings.push(format!(
                "{} nodes in quorum set are not known to the network",
                unknown_nodes.len()
            ));
        }

        if !low_trust_nodes.is_empty() {
            warnings.push(format!(
                "{} nodes have low trust (trusted by fewer than 2 other nodes)",
                low_trust_nodes.len()
            ));
        }

        if threshold_pct < 50.0 {
            warnings.push("Threshold is below 50%, which may be insecure".to_string());
        }

        if total_members < 3 {
            warnings.push("Quorum set has fewer than 3 members".to_string());
            is_valid = false;
        }

        if !quorum_set.is_valid() {
            warnings.push("Quorum set is structurally invalid".to_string());
            is_valid = false;
        }

        QuorumSetValidation {
            is_valid,
            warnings,
            unknown_nodes,
            low_trust_nodes,
            threshold_pct,
        }
    }
}

/// Result of validating a quorum set.
#[derive(Debug, Clone)]
pub struct QuorumSetValidation {
    /// Whether the quorum set is valid
    pub is_valid: bool,

    /// Warnings about the quorum set
    pub warnings: Vec<String>,

    /// Nodes in the quorum set that aren't known to the network
    pub unknown_nodes: Vec<ResponderId>,

    /// Nodes with low trust scores
    pub low_trust_nodes: Vec<ResponderId>,

    /// Threshold as a percentage
    pub threshold_pct: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        messages::{NodeAnnouncement, NodeCapabilities},
        store::{PeerStore, PeerStoreConfig},
    };
    use mc_common::NodeID;
    use mc_crypto_keys::Ed25519Public;
    use std::str::FromStr;

    fn make_node_id(name: &str) -> NodeID {
        NodeID {
            responder_id: ResponderId::from_str(&format!("{name}:8443")).unwrap(),
            public_key: Ed25519Public::default(),
        }
    }

    fn make_announcement(
        name: &str,
        trusted: Vec<&str>,
        timestamp: u64,
    ) -> NodeAnnouncement {
        let node_id = make_node_id(name);
        let quorum_set = QuorumSet::new(
            ((trusted.len() as u32 * 67) / 100).max(1),
            trusted
                .iter()
                .map(|n| {
                    QuorumSetMember::Node(ResponderId::from_str(&format!("{n}:8443")).unwrap())
                })
                .collect(),
        );

        NodeAnnouncement::new(
            node_id,
            vec![],
            quorum_set,
            vec![],
            NodeCapabilities::CONSENSUS | NodeCapabilities::GOSSIP,
            "1.0.0".to_string(),
            timestamp,
        )
    }

    #[test]
    fn test_trust_counts() {
        let store = Arc::new(PeerStore::new(PeerStoreConfig::default()));

        // Manually insert announcements (bypassing signature check)
        {
            let mut guard = store.announcements.write().unwrap();

            let ann1 = make_announcement("node1", vec!["node2", "node3"], 1000);
            guard.insert(ann1.node_id.responder_id.clone(), ann1);

            let ann2 = make_announcement("node2", vec!["node1", "node3"], 1000);
            guard.insert(ann2.node_id.responder_id.clone(), ann2);

            let ann3 = make_announcement("node3", vec!["node1", "node2"], 1000);
            guard.insert(ann3.node_id.responder_id.clone(), ann3);
        }

        let analyzer = TopologyAnalyzer::new(store);
        let counts = analyzer.get_trust_counts();

        // Each node should be trusted by 2 others
        assert_eq!(
            counts.get(&ResponderId::from_str("node1:8443").unwrap()),
            Some(&2)
        );
        assert_eq!(
            counts.get(&ResponderId::from_str("node2:8443").unwrap()),
            Some(&2)
        );
        assert_eq!(
            counts.get(&ResponderId::from_str("node3:8443").unwrap()),
            Some(&2)
        );
    }

    #[test]
    fn test_popular_nodes() {
        let store = Arc::new(PeerStore::new(PeerStoreConfig::default()));

        {
            let mut guard = store.announcements.write().unwrap();

            // node1 is trusted by node2 and node3
            let ann1 = make_announcement("node1", vec![], 1000);
            guard.insert(ann1.node_id.responder_id.clone(), ann1);

            let ann2 = make_announcement("node2", vec!["node1"], 1000);
            guard.insert(ann2.node_id.responder_id.clone(), ann2);

            let ann3 = make_announcement("node3", vec!["node1", "node2"], 1000);
            guard.insert(ann3.node_id.responder_id.clone(), ann3);
        }

        let analyzer = TopologyAnalyzer::new(store);
        let popular = analyzer.get_popular_nodes(1);

        assert!(!popular.is_empty());
        // node1 should be most popular (trusted by 2)
        assert_eq!(
            popular[0].0,
            ResponderId::from_str("node1:8443").unwrap()
        );
        assert_eq!(popular[0].1, 2);
    }

    #[test]
    fn test_topology_stats() {
        let store = Arc::new(PeerStore::new(PeerStoreConfig::default()));

        {
            let mut guard = store.announcements.write().unwrap();

            let ann1 = make_announcement("node1", vec!["node2", "node3"], 1000);
            guard.insert(ann1.node_id.responder_id.clone(), ann1);

            let ann2 = make_announcement("node2", vec!["node1", "node3"], 1000);
            guard.insert(ann2.node_id.responder_id.clone(), ann2);
        }

        let analyzer = TopologyAnalyzer::new(store);
        let stats = analyzer.stats();

        assert_eq!(stats.total_nodes, 2);
        assert_eq!(stats.consensus_nodes, 2);
        assert!(stats.avg_quorum_set_size > 0.0);
    }
}
