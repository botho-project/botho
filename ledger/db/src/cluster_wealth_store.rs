// Copyright (c) 2024 Botho Foundation

//! Cluster wealth tracking for progressive fee computation.
//!
//! This store maintains the total wealth attributed to each cluster across
//! all unspent outputs. When outputs are spent (via key images), their
//! cluster contributions are subtracted. When new outputs are created,
//! their cluster contributions are added.
//!
//! The wealth is tracked as the sum of (output_value * cluster_weight / TAG_WEIGHT_SCALE)
//! for all unspent outputs.

use crate::{key_bytes_to_u64, u64_to_key_bytes, Error};
use lmdb::{Database, DatabaseFlags, Environment, RwTransaction, Transaction, WriteFlags};
use bt_common::HashMap;
use bt_transaction_core::{ClusterId, ClusterTagVector, TAG_WEIGHT_SCALE};
// LMDB Database names.
/// Maps cluster_id -> total wealth (u64)
pub const CLUSTER_WEALTH_DB_NAME: &str = "cluster_wealth_store:wealth_by_cluster";

/// Tracks the total supply attributed to each cluster.
/// This provides the data needed for progressive fee computation.
#[derive(Clone)]
pub struct ClusterWealthStore {
    /// Wealth by cluster ID
    wealth_by_cluster: Database,
}

impl ClusterWealthStore {
    /// Opens an existing ClusterWealthStore.
    pub fn new(env: &Environment) -> Result<Self, Error> {
        Ok(ClusterWealthStore {
            wealth_by_cluster: env.open_db(Some(CLUSTER_WEALTH_DB_NAME))?,
        })
    }

    /// Creates a fresh ClusterWealthStore.
    pub fn create(env: &Environment) -> Result<(), Error> {
        env.create_db(Some(CLUSTER_WEALTH_DB_NAME), DatabaseFlags::empty())?;
        Ok(())
    }

    /// Get the total wealth attributed to a cluster.
    pub fn get_cluster_wealth(
        &self,
        cluster_id: ClusterId,
        db_transaction: &impl Transaction,
    ) -> Result<u64, Error> {
        match db_transaction.get(self.wealth_by_cluster, &u64_to_key_bytes(cluster_id.0)) {
            Ok(bytes) => Ok(key_bytes_to_u64(bytes)),
            Err(lmdb::Error::NotFound) => Ok(0),
            Err(e) => Err(Error::Lmdb(e)),
        }
    }

    /// Get wealth for multiple clusters at once.
    pub fn get_cluster_wealths(
        &self,
        cluster_ids: &[ClusterId],
        db_transaction: &impl Transaction,
    ) -> Result<HashMap<ClusterId, u64>, Error> {
        let mut result = HashMap::default();
        for &cluster_id in cluster_ids {
            let wealth = self.get_cluster_wealth(cluster_id, db_transaction)?;
            result.insert(cluster_id, wealth);
        }
        Ok(result)
    }

    /// Compute the wealth contribution of a TxOut to each cluster.
    /// Returns a map of cluster_id -> attributed_value.
    pub fn compute_txout_contributions(
        value: u64,
        cluster_tags: Option<&ClusterTagVector>,
    ) -> HashMap<ClusterId, u64> {
        let mut contributions = HashMap::default();

        if let Some(tags) = cluster_tags {
            for entry in &tags.entries {
                // contribution = value * weight / TAG_WEIGHT_SCALE
                let contribution =
                    ((value as u128) * (entry.weight as u128)) / (TAG_WEIGHT_SCALE as u128);
                contributions.insert(entry.cluster_id, contribution as u64);
            }
        }

        contributions
    }

    /// Apply wealth deltas for a block.
    ///
    /// This is called during block append to update cluster wealth:
    /// - Subtract contributions from spent outputs (inputs)
    /// - Add contributions from new outputs
    ///
    /// # Arguments
    /// * `input_contributions` - Contributions to subtract (from spent outputs)
    /// * `output_contributions` - Contributions to add (from new outputs)
    /// * `db_transaction` - The database transaction
    pub fn apply_wealth_deltas(
        &self,
        input_contributions: &HashMap<ClusterId, u64>,
        output_contributions: &HashMap<ClusterId, u64>,
        db_transaction: &mut RwTransaction,
    ) -> Result<(), Error> {
        // Collect all cluster IDs involved
        let mut all_clusters: Vec<ClusterId> = input_contributions.keys().copied().collect();
        for cluster_id in output_contributions.keys() {
            if !all_clusters.contains(cluster_id) {
                all_clusters.push(*cluster_id);
            }
        }

        // Update each cluster's wealth
        for cluster_id in all_clusters {
            let current_wealth = self.get_cluster_wealth(cluster_id, db_transaction)?;
            let subtract = input_contributions.get(&cluster_id).copied().unwrap_or(0);
            let add = output_contributions.get(&cluster_id).copied().unwrap_or(0);

            // Compute new wealth (saturating to prevent underflow)
            let new_wealth = current_wealth.saturating_sub(subtract).saturating_add(add);

            // Write updated wealth
            let key = u64_to_key_bytes(cluster_id.0);
            if new_wealth == 0 {
                // Remove entry if wealth is zero to save space
                match db_transaction.del(self.wealth_by_cluster, &key, None) {
                    Ok(()) => {}
                    Err(lmdb::Error::NotFound) => {} // Already gone
                    Err(e) => return Err(Error::Lmdb(e)),
                }
            } else {
                db_transaction.put(
                    self.wealth_by_cluster,
                    &key,
                    &u64_to_key_bytes(new_wealth),
                    WriteFlags::empty(),
                )?;
            }
        }

        Ok(())
    }

    /// Get all cluster wealths (for analysis/debugging).
    /// Returns a map of all clusters to their current wealth.
    pub fn get_all_cluster_wealths(
        &self,
        db_transaction: &impl Transaction,
    ) -> Result<HashMap<ClusterId, u64>, Error> {
        use lmdb::Cursor;

        let mut result = HashMap::default();
        let mut cursor = db_transaction.open_ro_cursor(self.wealth_by_cluster)?;

        for item in cursor.iter_start() {
            let (key, value) = item?;
            if key.len() == 8 && value.len() == 8 {
                let cluster_id = ClusterId(key_bytes_to_u64(key));
                let wealth = key_bytes_to_u64(value);
                result.insert(cluster_id, wealth);
            }
        }

        Ok(result)
    }

    /// Get the total wealth across all clusters.
    pub fn get_total_tracked_wealth(
        &self,
        db_transaction: &impl Transaction,
    ) -> Result<u64, Error> {
        let all_wealths = self.get_all_cluster_wealths(db_transaction)?;
        Ok(all_wealths.values().sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmdb::{Environment, EnvironmentFlags};
    use bt_transaction_core::ClusterTagEntry;
    use tempfile::TempDir;

    fn create_test_env() -> (TempDir, Environment) {
        let temp_dir = TempDir::new().unwrap();
        let env = Environment::new()
            .set_flags(EnvironmentFlags::empty())
            .set_max_dbs(10)
            .open(temp_dir.path())
            .unwrap();
        (temp_dir, env)
    }

    #[test]
    fn test_empty_store() {
        let (_temp_dir, env) = create_test_env();
        ClusterWealthStore::create(&env).unwrap();
        let store = ClusterWealthStore::new(&env).unwrap();

        let txn = env.begin_ro_txn().unwrap();
        assert_eq!(store.get_cluster_wealth(ClusterId(1), &txn).unwrap(), 0);
        assert_eq!(store.get_total_tracked_wealth(&txn).unwrap(), 0);
    }

    #[test]
    fn test_add_wealth() {
        let (_temp_dir, env) = create_test_env();
        ClusterWealthStore::create(&env).unwrap();
        let store = ClusterWealthStore::new(&env).unwrap();

        // Add 1000 units to cluster 1
        let mut txn = env.begin_rw_txn().unwrap();
        let input_contribs = HashMap::default();
        let mut output_contribs = HashMap::default();
        output_contribs.insert(ClusterId(1), 1000u64);
        store
            .apply_wealth_deltas(&input_contribs, &output_contribs, &mut txn)
            .unwrap();
        txn.commit().unwrap();

        // Verify
        let txn = env.begin_ro_txn().unwrap();
        assert_eq!(store.get_cluster_wealth(ClusterId(1), &txn).unwrap(), 1000);
        assert_eq!(store.get_cluster_wealth(ClusterId(2), &txn).unwrap(), 0);
    }

    #[test]
    fn test_subtract_wealth() {
        let (_temp_dir, env) = create_test_env();
        ClusterWealthStore::create(&env).unwrap();
        let store = ClusterWealthStore::new(&env).unwrap();

        // Add 1000 units first
        {
            let mut txn = env.begin_rw_txn().unwrap();
            let mut output_contribs = HashMap::default();
            output_contribs.insert(ClusterId(1), 1000u64);
            store
                .apply_wealth_deltas(&HashMap::default(), &output_contribs, &mut txn)
                .unwrap();
            txn.commit().unwrap();
        }

        // Subtract 300 units
        {
            let mut txn = env.begin_rw_txn().unwrap();
            let mut input_contribs = HashMap::default();
            input_contribs.insert(ClusterId(1), 300u64);
            store
                .apply_wealth_deltas(&input_contribs, &HashMap::default(), &mut txn)
                .unwrap();
            txn.commit().unwrap();
        }

        // Verify
        let txn = env.begin_ro_txn().unwrap();
        assert_eq!(store.get_cluster_wealth(ClusterId(1), &txn).unwrap(), 700);
    }

    #[test]
    fn test_compute_txout_contributions() {
        // TxOut worth 1000 with 50% cluster 1, 30% cluster 2
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 500_000, // 50%
        });
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(2),
            weight: 300_000, // 30%
        });

        let contribs = ClusterWealthStore::compute_txout_contributions(1000, Some(&tags));

        assert_eq!(contribs.get(&ClusterId(1)), Some(&500)); // 1000 * 0.5
        assert_eq!(contribs.get(&ClusterId(2)), Some(&300)); // 1000 * 0.3
        assert_eq!(contribs.get(&ClusterId(3)), None);
    }

    #[test]
    fn test_wealth_removed_when_zero() {
        let (_temp_dir, env) = create_test_env();
        ClusterWealthStore::create(&env).unwrap();
        let store = ClusterWealthStore::new(&env).unwrap();

        // Add 1000 units
        {
            let mut txn = env.begin_rw_txn().unwrap();
            let mut output_contribs = HashMap::default();
            output_contribs.insert(ClusterId(1), 1000u64);
            store
                .apply_wealth_deltas(&HashMap::default(), &output_contribs, &mut txn)
                .unwrap();
            txn.commit().unwrap();
        }

        // Subtract all 1000 units
        {
            let mut txn = env.begin_rw_txn().unwrap();
            let mut input_contribs = HashMap::default();
            input_contribs.insert(ClusterId(1), 1000u64);
            store
                .apply_wealth_deltas(&input_contribs, &HashMap::default(), &mut txn)
                .unwrap();
            txn.commit().unwrap();
        }

        // Verify it's zero
        let txn = env.begin_ro_txn().unwrap();
        assert_eq!(store.get_cluster_wealth(ClusterId(1), &txn).unwrap(), 0);

        // And that get_all_cluster_wealths doesn't include it
        let all = store.get_all_cluster_wealths(&txn).unwrap();
        assert!(!all.contains_key(&ClusterId(1)));
    }

    #[test]
    fn test_multiple_clusters() {
        let (_temp_dir, env) = create_test_env();
        ClusterWealthStore::create(&env).unwrap();
        let store = ClusterWealthStore::new(&env).unwrap();

        // Add wealth to multiple clusters
        {
            let mut txn = env.begin_rw_txn().unwrap();
            let mut output_contribs = HashMap::default();
            output_contribs.insert(ClusterId(1), 1000u64);
            output_contribs.insert(ClusterId(2), 2000u64);
            output_contribs.insert(ClusterId(3), 3000u64);
            store
                .apply_wealth_deltas(&HashMap::default(), &output_contribs, &mut txn)
                .unwrap();
            txn.commit().unwrap();
        }

        // Verify
        let txn = env.begin_ro_txn().unwrap();
        assert_eq!(store.get_cluster_wealth(ClusterId(1), &txn).unwrap(), 1000);
        assert_eq!(store.get_cluster_wealth(ClusterId(2), &txn).unwrap(), 2000);
        assert_eq!(store.get_cluster_wealth(ClusterId(3), &txn).unwrap(), 3000);
        assert_eq!(store.get_total_tracked_wealth(&txn).unwrap(), 6000);

        let all = store.get_all_cluster_wealths(&txn).unwrap();
        assert_eq!(all.len(), 3);
    }
}
