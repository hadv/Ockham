use crate::crypto::Hash;
use crate::types::{Block, QuorumCertificate, View};
use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

const TABLE_BLOCKS: TableDefinition<&[u8; 32], Vec<u8>> = TableDefinition::new("blocks");
const TABLE_QCS: TableDefinition<u64, Vec<u8>> = TableDefinition::new("qcs");
const TABLE_META: TableDefinition<&str, Vec<u8>> = TableDefinition::new("meta");

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Redb error: {0}")]
    Redb(Box<redb::Error>),
    #[error("Database error: {0}")]
    Database(Box<redb::DatabaseError>),
    #[error("Table error: {0}")]
    Table(Box<redb::TableError>),
    #[error("Storage error: {0}")]
    Storage(Box<redb::StorageError>),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("Transaction error: {0}")]
    Transaction(Box<redb::TransactionError>),
    #[error("Commit error: {0}")]
    Commit(Box<redb::CommitError>),
    #[error("Custom error: {0}")]
    Custom(String),
}

impl From<redb::Error> for StorageError {
    fn from(e: redb::Error) -> Self {
        Self::Redb(Box::new(e))
    }
}

impl From<redb::DatabaseError> for StorageError {
    fn from(e: redb::DatabaseError) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<redb::TableError> for StorageError {
    fn from(e: redb::TableError) -> Self {
        Self::Table(Box::new(e))
    }
}

impl From<redb::StorageError> for StorageError {
    fn from(e: redb::StorageError) -> Self {
        Self::Storage(Box::new(e))
    }
}

impl From<redb::TransactionError> for StorageError {
    fn from(e: redb::TransactionError) -> Self {
        Self::Transaction(Box::new(e))
    }
}

impl From<redb::CommitError> for StorageError {
    fn from(e: redb::CommitError) -> Self {
        Self::Commit(Box::new(e))
    }
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
// In-Memory Storage (for Copy/Clone tests where DB is too heavy or needs paths)
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
// Redb Storage
// -----------------------------------------------------------------------------
pub struct RedbStorage {
    db: Database,
}

impl RedbStorage {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StorageError::Custom(format!("Failed to create DB dir: {}", e)))?;
        }
        let db = Database::create(p)?;
        // Create tables if not exist
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(TABLE_BLOCKS)?;
            let _ = write_txn.open_table(TABLE_QCS)?;
            let _ = write_txn.open_table(TABLE_META)?;
        }
        write_txn.commit()?;
        Ok(Self { db })
    }
}

impl Storage for RedbStorage {
    fn save_block(&self, block: &Block) -> Result<(), StorageError> {
        let hash = crate::crypto::hash_data(block);
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_BLOCKS)?;
            let val = bincode::serialize(block)?;
            table.insert(&hash.0, val)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_block(&self, hash: &Hash) -> Result<Option<Block>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_BLOCKS)?;
        if let Some(val) = table.get(&hash.0)? {
            let block = bincode::deserialize(&val.value())?;
            Ok(Some(block))
        } else {
            Ok(None)
        }
    }

    fn save_qc(&self, qc: &QuorumCertificate) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_QCS)?;
            let val = bincode::serialize(qc)?;
            table.insert(qc.view, val)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_qc(&self, view: View) -> Result<Option<QuorumCertificate>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_QCS)?;
        if let Some(val) = table.get(view)? {
            let qc = bincode::deserialize(&val.value())?;
            Ok(Some(qc))
        } else {
            Ok(None)
        }
    }

    fn save_consensus_state(&self, state: &ConsensusState) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_META)?;
            let val = bincode::serialize(state)?;
            table.insert("consensus_state", val)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_consensus_state(&self) -> Result<Option<ConsensusState>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_META)?;
        if let Some(val) = table.get("consensus_state")? {
            let state = bincode::deserialize(&val.value())?;
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }
}
