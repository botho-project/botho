// Copyright (c) 2018-2022 The MobileCoin Foundation
// Copyright (c) 2024 Cadence Foundation

//! Data access abstraction for TxOuts stored in the ledger.
//!
//! Note: The Merkle tree was removed as part of SGX removal. With SGX enclaves
//! removed, membership proofs are no longer needed - the consensus service can
//! directly query the ledger to verify that ring members exist.

use crate::{key_bytes_to_u64, u64_to_key_bytes, Error};
use lmdb::{Database, DatabaseFlags, Environment, RwTransaction, Transaction, WriteFlags};
use mc_common::Hash;
use mc_crypto_keys::CompressedRistrettoPublic;
use mc_transaction_core::tx::TxOut;
use mc_util_serial::{decode, encode};

// LMDB Database names.
pub const COUNTS_DB_NAME: &str = "tx_out_store:counts";
pub const TX_OUT_INDEX_BY_HASH_DB_NAME: &str = "tx_out_store:tx_out_index_by_hash";
pub const TX_OUT_INDEX_BY_PUBLIC_KEY_DB_NAME: &str = "tx_out_store:tx_out_index_by_public_key";
pub const TX_OUT_BY_INDEX_DB_NAME: &str = "tx_out_store:tx_out_by_index";
// Note: MERKLE_HASH_BY_RANGE_DB_NAME was removed with SGX - no longer needed.

// Keys used by the `counts` database.
pub const NUM_TX_OUTS_KEY: &str = "num_tx_outs";

#[derive(Clone)]
pub struct TxOutStore {
    /// Aggregate counts
    /// * `NUM_TX_OUTS_KEY` --> Number (u64) of TxOuts in the ledger.
    counts: Database,

    /// TxOut by index. `key_bytes_to_u64(index) -> encode(&tx_out)`
    tx_out_by_index: Database,

    /// `tx_out.hash() -> u64_to_key_bytes(index)`
    tx_out_index_by_hash: Database,

    /// `tx_out.public_key -> u64_to_key_bytes(index)`
    tx_out_index_by_public_key: Database,
}

impl TxOutStore {
    #[cfg(feature = "migration_support")]
    pub fn get_tx_out_index_by_public_key_database(&self) -> Database {
        self.tx_out_index_by_public_key
    }

    /// Opens an existing TxOutStore.
    pub fn new(env: &Environment) -> Result<Self, Error> {
        Ok(TxOutStore {
            counts: env.open_db(Some(COUNTS_DB_NAME))?,
            tx_out_index_by_hash: env.open_db(Some(TX_OUT_INDEX_BY_HASH_DB_NAME))?,
            tx_out_index_by_public_key: env.open_db(Some(TX_OUT_INDEX_BY_PUBLIC_KEY_DB_NAME))?,
            tx_out_by_index: env.open_db(Some(TX_OUT_BY_INDEX_DB_NAME))?,
        })
    }

    // Creates a fresh TxOutStore on disk.
    pub fn create(env: &Environment) -> Result<(), Error> {
        let counts = env.create_db(Some(COUNTS_DB_NAME), DatabaseFlags::empty())?;
        env.create_db(Some(TX_OUT_INDEX_BY_HASH_DB_NAME), DatabaseFlags::empty())?;
        env.create_db(
            Some(TX_OUT_INDEX_BY_PUBLIC_KEY_DB_NAME),
            DatabaseFlags::empty(),
        )?;
        env.create_db(Some(TX_OUT_BY_INDEX_DB_NAME), DatabaseFlags::empty())?;

        let mut db_transaction = env.begin_rw_txn()?;

        db_transaction.put(
            counts,
            &NUM_TX_OUTS_KEY,
            &u64_to_key_bytes(0),
            WriteFlags::empty(),
        )?;

        db_transaction.commit()?;
        Ok(())
    }

    /// Appends a TxOut to the end of the collection.
    /// Returns the index of the TxOut in the ledger, or an Error.
    pub fn push(&self, tx_out: &TxOut, db_transaction: &mut RwTransaction) -> Result<u64, Error> {
        let num_tx_outs: u64 = key_bytes_to_u64(db_transaction.get(self.counts, &NUM_TX_OUTS_KEY)?);
        let index: u64 = num_tx_outs;

        db_transaction.put(
            self.counts,
            &NUM_TX_OUTS_KEY,
            &u64_to_key_bytes(num_tx_outs + 1_u64),
            WriteFlags::empty(),
        )?;

        db_transaction.put(
            self.tx_out_index_by_hash,
            &tx_out.hash(),
            &u64_to_key_bytes(index),
            WriteFlags::NO_OVERWRITE,
        )?;

        db_transaction.put(
            self.tx_out_index_by_public_key,
            &tx_out.public_key,
            &u64_to_key_bytes(index),
            WriteFlags::NO_OVERWRITE,
        )?;

        let tx_out_bytes: Vec<u8> = encode(tx_out);

        db_transaction.put(
            self.tx_out_by_index,
            &u64_to_key_bytes(index),
            &tx_out_bytes,
            WriteFlags::NO_OVERWRITE,
        )?;

        Ok(index)
    }

    /// Get the total number of TxOuts in the ledger.
    pub fn num_tx_outs<T: Transaction>(&self, db_transaction: &T) -> Result<u64, Error> {
        Ok(key_bytes_to_u64(
            db_transaction.get(self.counts, &NUM_TX_OUTS_KEY)?,
        ))
    }

    /// Returns the index of the TxOut with the given hash.
    pub fn get_tx_out_index_by_hash<T: Transaction>(
        &self,
        tx_out_hash: &Hash,
        db_transaction: &T,
    ) -> Result<u64, Error> {
        let index_bytes = db_transaction.get(self.tx_out_index_by_hash, tx_out_hash)?;
        Ok(key_bytes_to_u64(index_bytes))
    }

    /// Returns the index of the TxOut with the public key.
    pub fn get_tx_out_index_by_public_key<T: Transaction>(
        &self,
        tx_out_public_key: &CompressedRistrettoPublic,
        db_transaction: &T,
    ) -> Result<u64, Error> {
        let index_bytes = db_transaction.get(self.tx_out_index_by_public_key, tx_out_public_key)?;
        Ok(key_bytes_to_u64(index_bytes))
    }

    /// Gets a TxOut by its index in the ledger.
    pub fn get_tx_out_by_index<T: Transaction>(
        &self,
        index: u64,
        db_transaction: &T,
    ) -> Result<TxOut, Error> {
        let tx_out_bytes = db_transaction.get(self.tx_out_by_index, &u64_to_key_bytes(index))?;
        let tx_out: TxOut = decode(tx_out_bytes)?;
        Ok(tx_out)
    }

    /// Check if a TxOut exists in the store by its hash.
    pub fn contains_tx_out_by_hash<T: Transaction>(
        &self,
        tx_out_hash: &Hash,
        db_transaction: &T,
    ) -> Result<bool, Error> {
        match db_transaction.get(self.tx_out_index_by_hash, tx_out_hash) {
            Ok(_) => Ok(true),
            Err(lmdb::Error::NotFound) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Check if a TxOut exists in the store by its public key.
    pub fn contains_tx_out_by_public_key<T: Transaction>(
        &self,
        tx_out_public_key: &CompressedRistrettoPublic,
        db_transaction: &T,
    ) -> Result<bool, Error> {
        match db_transaction.get(self.tx_out_index_by_public_key, tx_out_public_key) {
            Ok(_) => Ok(true),
            Err(lmdb::Error::NotFound) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Get a Merkle proof of membership for a TxOut.
    ///
    /// Note: Merkle proofs were removed with SGX. This stub returns an error.
    /// Consensus now validates ring members directly without membership proofs.
    pub fn get_merkle_proof_of_membership<T: Transaction>(
        &self,
        _index: u64,
        _db_transaction: &T,
    ) -> Result<mc_transaction_core::tx::TxOutMembershipProof, Error> {
        Err(Error::MerkleProofsNotSupported)
    }

    /// Get the root Merkle hash.
    ///
    /// Note: Merkle proofs were removed with SGX. This stub returns an error.
    pub fn get_root_merkle_hash<T: Transaction>(
        &self,
        _db_transaction: &T,
    ) -> Result<Hash, Error> {
        Err(Error::MerkleProofsNotSupported)
    }
}

#[cfg(test)]
pub mod tx_out_store_tests {
    use super::TxOutStore;
    use crate::Error;
    use lmdb::{Environment, RoTransaction, RwTransaction, Transaction};
    use mc_account_keys::AccountKey;
    use mc_common::Hash;
    use mc_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate};
    use mc_transaction_core::{
        encrypted_fog_hint::{EncryptedFogHint, ENCRYPTED_FOG_HINT_LEN},
        tokens::Mob,
        tx::TxOut,
        Amount, BlockVersion, Token,
    };
    use mc_util_from_random::FromRandom;
    use rand::{rngs::StdRng, SeedableRng};
    use std::path::Path;
    use tempfile::TempDir;

    /// Create an LMDB environment that can be used for testing.
    pub fn get_env() -> Environment {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        Environment::new()
            .set_max_dbs(10)
            .set_map_size(1_099_511_627_776)
            .open(Path::new(&path))
            .unwrap()
    }

    pub fn init_tx_out_store() -> (TxOutStore, Environment) {
        let env = get_env();
        TxOutStore::create(&env).unwrap();
        let tx_out_store: TxOutStore = TxOutStore::new(&env).unwrap();
        (tx_out_store, env)
    }

    /// Creates a number of TxOuts.
    ///
    /// All TxOuts are created as part of the same transaction, with the same
    /// recipient.
    pub fn get_tx_outs(num_tx_outs: u32) -> Vec<TxOut> {
        let mut rng: StdRng = SeedableRng::from_seed([1u8; 32]);
        let mut tx_outs: Vec<TxOut> = Vec::new();
        let recipient_account = AccountKey::random(&mut rng);
        let value: u64 = 100;
        let token_id = Mob::ID;

        for _i in 0..num_tx_outs {
            let amount = Amount { value, token_id };
            let tx_private_key = RistrettoPrivate::from_random(&mut rng);
            let tx_out = TxOut::new(
                BlockVersion::MAX,
                amount,
                &recipient_account.default_subaddress(),
                &tx_private_key,
                EncryptedFogHint::new(&[7u8; ENCRYPTED_FOG_HINT_LEN]),
            )
            .unwrap();
            tx_outs.push(tx_out);
        }
        tx_outs
    }

    #[test]
    // An empty `TxOutStore` should return the correct values or Errors.
    fn test_initial_tx_out_store() {
        let (tx_out_store, env) = init_tx_out_store();
        let db_transaction: RoTransaction = env.begin_ro_txn().unwrap();
        assert_eq!(0, tx_out_store.num_tx_outs(&db_transaction).unwrap());
    }

    #[test]
    // `get_tx_out_index_by_hash` should return the correct index, or
    // Error::NotFound.
    fn test_get_tx_out_index_by_hash() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(111);

        {
            // Push a number of TxOuts to the store.
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        let ro_transaction: RoTransaction = env.begin_ro_txn().unwrap();
        assert_eq!(
            tx_outs.len() as u64,
            tx_out_store.num_tx_outs(&ro_transaction).unwrap()
        );

        // `get_tx_out_by_index_by_hash` should return the correct index when given a
        // recognized hash.
        for (index, tx_out) in tx_outs.iter().enumerate() {
            assert_eq!(
                index as u64,
                tx_out_store
                    .get_tx_out_index_by_hash(&tx_out.hash(), &ro_transaction)
                    .unwrap()
            );
        }

        // `get_tx_out_index_by_hash` should return `Error::NotFound` for an
        // unrecognized hash.
        let unrecognized_hash: Hash = [0u8; 32];
        match tx_out_store.get_tx_out_index_by_hash(&unrecognized_hash, &ro_transaction) {
            Ok(index) => panic!("Returned index {index:?} for unrecognized hash."),
            Err(Error::NotFound) => {
                // This is expected.
            }
            Err(e) => panic!("Unexpected Error {e:?}"),
        }
    }

    #[test]
    // `get_tx_out_index_by_public_key` should return the correct index, or
    // Error::NotFound.
    fn test_get_tx_out_index_by_public_key() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(111);

        {
            // Push a number of TxOuts to the store.
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        let ro_transaction: RoTransaction = env.begin_ro_txn().unwrap();
        assert_eq!(
            tx_outs.len() as u64,
            tx_out_store.num_tx_outs(&ro_transaction).unwrap()
        );

        // `get_tx_out_by_index_by_hash` should return the correct index when given a
        // recognized hash.
        for (index, tx_out) in tx_outs.iter().enumerate() {
            assert_eq!(
                index as u64,
                tx_out_store
                    .get_tx_out_index_by_public_key(&tx_out.public_key, &ro_transaction)
                    .unwrap()
            );
        }

        // `get_tx_out_index_by_public_key` should return `Error::NotFound` for an
        // unrecognized hash.
        let unrecognized_public_key =
            CompressedRistrettoPublic::try_from(&[0; 32]).expect("Could not construct key");
        match tx_out_store.get_tx_out_index_by_public_key(&unrecognized_public_key, &ro_transaction)
        {
            Ok(index) => panic!("Returned index {index:?} for unrecognized public key."),
            Err(Error::NotFound) => {
                // This is expected.
            }
            Err(e) => panic!("Unexpected Error {e:?}"),
        }
    }

    #[test]
    // `get_tx_out_by_index` should return the correct TxOut, or Error::NotFound.
    fn test_get_tx_out_by_index() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(111);

        {
            // Push a number of TxOuts to the store.
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        let ro_transaction: RoTransaction = env.begin_ro_txn().unwrap();
        assert_eq!(
            tx_outs.len() as u64,
            tx_out_store.num_tx_outs(&ro_transaction).unwrap()
        );

        // `get_tx_out_by_index` should return the correct TxOut if the index is in the
        // ledger.
        for (index, tx_out) in tx_outs.iter().enumerate() {
            assert_eq!(
                *tx_out,
                tx_out_store
                    .get_tx_out_by_index(index as u64, &ro_transaction)
                    .unwrap()
            );
        }

        // `get_tx_out_by_index` should return `Error::NotFound` for out-of-bound
        // indices
        for index in tx_outs.len()..tx_outs.len() + 100 {
            match tx_out_store.get_tx_out_by_index(index as u64, &ro_transaction) {
                Ok(_tx_out) => panic!("Returned a TxOut for a nonexistent index."),
                Err(Error::NotFound) => {
                    // This is expected.
                }
                Err(e) => panic!("Unexpected Error {e:?}"),
            }
        }
        ro_transaction.commit().unwrap();
    }

    #[test]
    // Pushing a duplicate TxOut should fail.
    fn test_push_duplicate_txout_fails() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(10);

        {
            // Push a number of TxOuts to the store.
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
        match tx_out_store.push(&tx_outs[0], &mut rw_transaction) {
            Err(Error::Lmdb(lmdb::Error::KeyExist)) => {}
            Ok(_) => panic!("unexpected success"),
            Err(_) => panic!("unexpected error"),
        };
    }

    #[test]
    // Pushing a TxOut with a duplicate public key should fail.
    fn test_push_duplicate_public_key_fails() {
        let (tx_out_store, env) = init_tx_out_store();
        let mut tx_outs = get_tx_outs(10);

        {
            // Push a number of TxOuts to the store.
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs[1..] {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        tx_outs[0].public_key = tx_outs[1].public_key;

        let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
        match tx_out_store.push(&tx_outs[0], &mut rw_transaction) {
            Err(Error::Lmdb(lmdb::Error::KeyExist)) => {}
            Ok(_) => panic!("unexpected success"),
            Err(_) => panic!("unexpected error"),
        };
    }

    #[test]
    // `push` should add a TxOut to the correct index.
    fn test_push() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(100);

        let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
        assert_eq!(0, tx_out_store.num_tx_outs(&rw_transaction).unwrap());

        for (i, tx_out) in tx_outs.iter().enumerate() {
            let index = tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            assert_eq!(i as u64, index);
            let expected_count = i as u64 + 1;
            assert_eq!(
                expected_count,
                tx_out_store.num_tx_outs(&rw_transaction).unwrap()
            );
            assert_eq!(
                *tx_out,
                tx_out_store
                    .get_tx_out_by_index(index, &rw_transaction)
                    .unwrap()
            );
        }
        rw_transaction.commit().unwrap();
    }

    #[test]
    // `contains_tx_out_by_hash` should return true for known TxOuts and false for unknown.
    fn test_contains_tx_out_by_hash() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(10);

        {
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        let ro_transaction: RoTransaction = env.begin_ro_txn().unwrap();

        // Known TxOuts should return true
        for tx_out in &tx_outs {
            assert!(tx_out_store
                .contains_tx_out_by_hash(&tx_out.hash(), &ro_transaction)
                .unwrap());
        }

        // Unknown hash should return false
        let unknown_hash: Hash = [0u8; 32];
        assert!(!tx_out_store
            .contains_tx_out_by_hash(&unknown_hash, &ro_transaction)
            .unwrap());
    }

    #[test]
    // `contains_tx_out_by_public_key` should return true for known TxOuts and false for unknown.
    fn test_contains_tx_out_by_public_key() {
        let (tx_out_store, env) = init_tx_out_store();
        let tx_outs = get_tx_outs(10);

        {
            let mut rw_transaction: RwTransaction = env.begin_rw_txn().unwrap();
            for tx_out in &tx_outs {
                tx_out_store.push(tx_out, &mut rw_transaction).unwrap();
            }
            rw_transaction.commit().unwrap();
        }

        let ro_transaction: RoTransaction = env.begin_ro_txn().unwrap();

        // Known TxOuts should return true
        for tx_out in &tx_outs {
            assert!(tx_out_store
                .contains_tx_out_by_public_key(&tx_out.public_key, &ro_transaction)
                .unwrap());
        }

        // Unknown public key should return false
        let unknown_public_key =
            CompressedRistrettoPublic::try_from(&[0; 32]).expect("Could not construct key");
        assert!(!tx_out_store
            .contains_tx_out_by_public_key(&unknown_public_key, &ro_transaction)
            .unwrap());
    }
}
