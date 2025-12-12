use crate::crypto::Hash;
use crate::types::{Block, QuorumCertificate, View};
use rocksdb::{ColumnFamilyDescriptor, DB, Options};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("RocksDB error: {0}")]
    RocksDB(#[from] rocksdb::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("Not found")]
    NotFound,
}

/// Persistent State that needs to be saved atomically (or somewhat atomically)
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct ConsensusState {
    pub view: View,
    pub finalized_height: View,
    pub preferred_block: Hash,
    pub preferred_view: View,
}

pub trait Storage: Send + Sync {
    fn save_block(&self, block: &Block) -> Result<(), StorageError>;
    fn get_block(&self, hash: &Hash) -> Result<Option<Block>, StorageError>;

    fn save_qc(&self, qc: &QuorumCertificate) -> Result<(), StorageError>;
    fn get_qc(&self, view: View) -> Result<Option<QuorumCertificate>, StorageError>;

    fn save_consensus_state(&self, state: &ConsensusState) -> Result<(), StorageError>;
    fn get_consensus_state(&self) -> Result<Option<ConsensusState>, StorageError>;
}

// -----------------------------------------------------------------------------
// In-Memory Storage (for Copy/Clone tests where RocksDB is too heavy or needs paths)
// -----------------------------------------------------------------------------
#[derive(Clone, Default)]
pub struct MemStorage {
    blocks: Arc<Mutex<HashMap<Hash, Block>>>,
    qcs: Arc<Mutex<HashMap<View, QuorumCertificate>>>,
    state: Arc<Mutex<Option<ConsensusState>>>,
}

impl MemStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Storage for MemStorage {
    fn save_block(&self, block: &Block) -> Result<(), StorageError> {
        let hash = crate::crypto::hash_data(block);
        self.blocks.lock().unwrap().insert(hash, block.clone());
        Ok(())
    }

    fn get_block(&self, hash: &Hash) -> Result<Option<Block>, StorageError> {
        Ok(self.blocks.lock().unwrap().get(hash).cloned())
    }

    fn save_qc(&self, qc: &QuorumCertificate) -> Result<(), StorageError> {
        self.qcs.lock().unwrap().insert(qc.view, qc.clone());
        Ok(())
    }

    fn get_qc(&self, view: View) -> Result<Option<QuorumCertificate>, StorageError> {
        Ok(self.qcs.lock().unwrap().get(&view).cloned())
    }

    fn save_consensus_state(&self, state: &ConsensusState) -> Result<(), StorageError> {
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }

    fn get_consensus_state(&self) -> Result<Option<ConsensusState>, StorageError> {
        Ok(self.state.lock().unwrap().clone())
    }
}

// -----------------------------------------------------------------------------
// RocksDB Storage
// -----------------------------------------------------------------------------
pub struct RocksStorage {
    db: DB,
}

impl RocksStorage {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfs = vec![
            ColumnFamilyDescriptor::new("default", Options::default()), // Metadata (ConsensusState)
            ColumnFamilyDescriptor::new("blocks", Options::default()),
            ColumnFamilyDescriptor::new("qcs", Options::default()),
        ];

        let db = DB::open_cf_descriptors(&opts, path, cfs)?;
        Ok(Self { db })
    }
}

impl Storage for RocksStorage {
    fn save_block(&self, block: &Block) -> Result<(), StorageError> {
        let hash = crate::crypto::hash_data(block);
        let cf = self.db.cf_handle("blocks").unwrap();
        let key = hash.0; // [u8; 32]
        let val = bincode::serialize(block)?;
        self.db.put_cf(cf, key, val)?;
        Ok(())
    }

    fn get_block(&self, hash: &Hash) -> Result<Option<Block>, StorageError> {
        let cf = self.db.cf_handle("blocks").unwrap();
        if let Some(val) = self.db.get_cf(cf, hash.0)? {
            let block = bincode::deserialize(&val)?;
            Ok(Some(block))
        } else {
            Ok(None)
        }
    }

    fn save_qc(&self, qc: &QuorumCertificate) -> Result<(), StorageError> {
        let cf = self.db.cf_handle("qcs").unwrap();
        let key = qc.view.to_be_bytes();
        let val = bincode::serialize(qc)?;
        self.db.put_cf(cf, key, val)?;
        Ok(())
    }

    fn get_qc(&self, view: View) -> Result<Option<QuorumCertificate>, StorageError> {
        let cf = self.db.cf_handle("qcs").unwrap();
        if let Some(val) = self.db.get_cf(cf, view.to_be_bytes())? {
            let qc = bincode::deserialize(&val)?;
            Ok(Some(qc))
        } else {
            Ok(None)
        }
    }

    fn save_consensus_state(&self, state: &ConsensusState) -> Result<(), StorageError> {
        let key = b"consensus_state";
        // Default CF
        let val = bincode::serialize(state)?;
        self.db.put(key, val)?;
        Ok(())
    }

    fn get_consensus_state(&self) -> Result<Option<ConsensusState>, StorageError> {
        let key = b"consensus_state";
        if let Some(val) = self.db.get(key)? {
            let state = bincode::deserialize(&val)?;
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }
}
