use crate::crypto::{Hash, PublicKey};
use crate::types::{Address, Block, QuorumCertificate, View};
use alloy_primitives::{Bytes, U256};
use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

const TABLE_BLOCKS: TableDefinition<&[u8; 32], Vec<u8>> = TableDefinition::new("blocks");
const TABLE_QCS: TableDefinition<u64, Vec<u8>> = TableDefinition::new("qcs");
const TABLE_META: TableDefinition<&str, Vec<u8>> = TableDefinition::new("meta");

// New Tables for EVM State
const TABLE_ACCOUNTS: TableDefinition<&[u8; 20], Vec<u8>> = TableDefinition::new("accounts");
const TABLE_STORAGE: TableDefinition<&[u8], Vec<u8>> = TableDefinition::new("storage"); // Key: Address + StorageKey
const TABLE_CODE: TableDefinition<&[u8; 32], Vec<u8>> = TableDefinition::new("code");
const TABLE_SMT_LEAVES: TableDefinition<&[u8; 32], Vec<u8>> = TableDefinition::new("smt_leaves");
const TABLE_SMT_BRANCHES: TableDefinition<&[u8], Vec<u8>> = TableDefinition::new("smt_branches");

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
    pub last_voted_view: View,
    pub committee: Vec<PublicKey>,
    pub pending_validators: Vec<(PublicKey, View)>,
    pub exiting_validators: Vec<(PublicKey, View)>,
    pub stakes: HashMap<Address, U256>,
    pub inactivity_scores: HashMap<PublicKey, u64>,
}

/// Account Information stored in the Global State
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountInfo {
    pub nonce: u64,
    pub balance: U256,
    pub code_hash: Hash,     // keccak256(code)
    pub code: Option<Bytes>, // Cache code here or just check TABLE_CODE
}

impl Default for AccountInfo {
    fn default() -> Self {
        Self {
            nonce: 0,
            balance: U256::ZERO,
            code_hash: Hash::default(), // Should be empty hash?
            code: None,
        }
    }
}

pub trait Storage: Send + Sync {
    fn save_block(&self, block: &Block) -> Result<(), StorageError>;
    fn get_block(&self, hash: &Hash) -> Result<Option<Block>, StorageError>;

    fn save_qc(&self, qc: &QuorumCertificate) -> Result<(), StorageError>;
    fn get_qc(&self, view: View) -> Result<Option<QuorumCertificate>, StorageError>;

    fn save_consensus_state(&self, state: &ConsensusState) -> Result<(), StorageError>;
    fn get_consensus_state(&self) -> Result<Option<ConsensusState>, StorageError>;

    // EVM State
    fn get_account(&self, address: &Address) -> Result<Option<AccountInfo>, StorageError>;
    fn save_account(&self, address: &Address, info: &AccountInfo) -> Result<(), StorageError>;
    fn get_code(&self, hash: &Hash) -> Result<Option<Bytes>, StorageError>;
    fn save_code(&self, hash: &Hash, code: &Bytes) -> Result<(), StorageError>;
    fn get_storage(&self, address: &Address, index: &U256) -> Result<U256, StorageError>;
    fn save_storage(
        &self,
        address: &Address,
        index: &U256,
        value: &U256,
    ) -> Result<(), StorageError>;

    // SMT Storage
    fn get_smt_branch(&self, height: u8, node_key: &Hash) -> Result<Option<Vec<u8>>, StorageError>;
    fn save_smt_branch(&self, height: u8, node_key: &Hash, node: &[u8])
    -> Result<(), StorageError>;
    fn get_smt_leaf(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError>;
    fn save_smt_leaf(&self, hash: &Hash, node: &[u8]) -> Result<(), StorageError>;
}

// -----------------------------------------------------------------------------
// In-Memory Storage (for Copy/Clone tests where DB is too heavy or needs paths)
// -----------------------------------------------------------------------------
pub type SmtBranchMap = HashMap<(u8, Hash), Vec<u8>>;

#[derive(Clone, Default)]
pub struct MemStorage {
    blocks: Arc<Mutex<HashMap<Hash, Block>>>,
    qcs: Arc<Mutex<HashMap<View, QuorumCertificate>>>,
    state: Arc<Mutex<Option<ConsensusState>>>,
    // EVM State
    accounts: Arc<Mutex<HashMap<Address, AccountInfo>>>,
    code: Arc<Mutex<HashMap<Hash, Bytes>>>,
    storage: Arc<Mutex<HashMap<(Address, U256), U256>>>,
    smt_leaves: Arc<Mutex<HashMap<Hash, Vec<u8>>>>,
    smt_branches: Arc<Mutex<SmtBranchMap>>,
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

    fn get_account(&self, address: &Address) -> Result<Option<AccountInfo>, StorageError> {
        Ok(self.accounts.lock().unwrap().get(address).cloned())
    }

    fn save_account(&self, address: &Address, info: &AccountInfo) -> Result<(), StorageError> {
        self.accounts.lock().unwrap().insert(*address, info.clone());
        Ok(())
    }

    fn get_code(&self, hash: &Hash) -> Result<Option<Bytes>, StorageError> {
        Ok(self.code.lock().unwrap().get(hash).cloned())
    }

    fn save_code(&self, hash: &Hash, code: &Bytes) -> Result<(), StorageError> {
        self.code.lock().unwrap().insert(*hash, code.clone());
        Ok(())
    }

    fn get_storage(&self, address: &Address, index: &U256) -> Result<U256, StorageError> {
        Ok(self
            .storage
            .lock()
            .unwrap()
            .get(&(*address, *index))
            .cloned()
            .unwrap_or(U256::ZERO))
    }

    fn save_storage(
        &self,
        address: &Address,
        index: &U256,
        value: &U256,
    ) -> Result<(), StorageError> {
        self.storage
            .lock()
            .unwrap()
            .insert((*address, *index), *value);
        Ok(())
    }

    fn get_smt_branch(&self, height: u8, node_key: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self
            .smt_branches
            .lock()
            .unwrap()
            .get(&(height, *node_key))
            .cloned())
    }

    fn save_smt_branch(
        &self,
        height: u8,
        node_key: &Hash,
        node: &[u8],
    ) -> Result<(), StorageError> {
        self.smt_branches
            .lock()
            .unwrap()
            .insert((height, *node_key), node.to_vec());
        Ok(())
    }

    fn get_smt_leaf(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.smt_leaves.lock().unwrap().get(hash).cloned())
    }

    fn save_smt_leaf(&self, hash: &Hash, node: &[u8]) -> Result<(), StorageError> {
        self.smt_leaves.lock().unwrap().insert(*hash, node.to_vec());
        Ok(())
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
            let _ = write_txn.open_table(TABLE_ACCOUNTS)?;
            let _ = write_txn.open_table(TABLE_STORAGE)?;
            let _ = write_txn.open_table(TABLE_CODE)?;
            let _ = write_txn.open_table(TABLE_SMT_LEAVES)?;
            let _ = write_txn.open_table(TABLE_SMT_BRANCHES)?;
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

    fn get_account(&self, address: &Address) -> Result<Option<AccountInfo>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_ACCOUNTS)?;
        if let Some(val) = table.get(&*address.0)? {
            let info = bincode::deserialize(&val.value())?;
            Ok(Some(info))
        } else {
            Ok(None)
        }
    }

    fn save_account(&self, address: &Address, info: &AccountInfo) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_ACCOUNTS)?;
            let val = bincode::serialize(info)?;
            table.insert(&*address.0, val)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_code(&self, hash: &Hash) -> Result<Option<Bytes>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_CODE)?;
        if let Some(val) = table.get(&hash.0)? {
            // Store as Vec<u8> which Bytes is wrapper for.
            let bytes: Vec<u8> = bincode::deserialize(&val.value())?;
            Ok(Some(Bytes::from(bytes)))
        } else {
            Ok(None)
        }
    }

    fn save_code(&self, hash: &Hash, code: &Bytes) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_CODE)?;
            let val = bincode::serialize(&code.to_vec())?;
            table.insert(&hash.0, val)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_storage(&self, address: &Address, index: &U256) -> Result<U256, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_STORAGE)?;

        // Composite Key: Address + Index
        // 20 bytes + 32 bytes = 52 bytes
        let mut key = Vec::with_capacity(52);
        key.extend_from_slice(address.as_slice());
        key.extend_from_slice(&index.to_be_bytes::<32>());

        if let Some(val) = table.get(key.as_slice())? {
            let value = bincode::deserialize(&val.value())?;
            Ok(value)
        } else {
            Ok(U256::ZERO)
        }
    }

    fn save_storage(
        &self,
        address: &Address,
        index: &U256,
        value: &U256,
    ) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_STORAGE)?;
            let mut key = Vec::with_capacity(52);
            key.extend_from_slice(address.as_slice());
            key.extend_from_slice(&index.to_be_bytes::<32>());

            let val = bincode::serialize(value)?;
            table.insert(key.as_slice(), val)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_smt_branch(&self, height: u8, node_key: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_SMT_BRANCHES)?;
        let mut key = Vec::with_capacity(33);
        key.push(height);
        key.extend_from_slice(&node_key.0);
        if let Some(val) = table.get(key.as_slice())? {
            Ok(Some(val.value().to_vec()))
        } else {
            Ok(None)
        }
    }

    fn save_smt_branch(
        &self,
        height: u8,
        node_key: &Hash,
        node: &[u8],
    ) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_SMT_BRANCHES)?;
            let mut key = Vec::with_capacity(33);
            key.push(height);
            key.extend_from_slice(&node_key.0);
            table.insert(key.as_slice(), node.to_vec())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_smt_leaf(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE_SMT_LEAVES)?;
        if let Some(val) = table.get(&hash.0)? {
            Ok(Some(val.value().to_vec()))
        } else {
            Ok(None)
        }
    }

    fn save_smt_leaf(&self, hash: &Hash, node: &[u8]) -> Result<(), StorageError> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_SMT_LEAVES)?;
            table.insert(&hash.0, node.to_vec())?;
        }
        write_txn.commit()?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// State Overlay (In-Memory Sandbox for Validation)
// -----------------------------------------------------------------------------
pub struct StateOverlay {
    inner: Arc<dyn Storage>,
    // Overlay Cache
    accounts: Arc<Mutex<HashMap<Address, AccountInfo>>>,
    storage: Arc<Mutex<HashMap<(Address, U256), U256>>>,
    code: Arc<Mutex<HashMap<Hash, Bytes>>>,
    smt_leaves: Arc<Mutex<HashMap<Hash, Vec<u8>>>>,
    smt_branches: Arc<Mutex<SmtBranchMap>>,
}

impl StateOverlay {
    pub fn new(inner: Arc<dyn Storage>) -> Self {
        Self {
            inner,
            accounts: Arc::new(Mutex::new(HashMap::new())),
            storage: Arc::new(Mutex::new(HashMap::new())),
            code: Arc::new(Mutex::new(HashMap::new())),
            smt_leaves: Arc::new(Mutex::new(HashMap::new())),
            smt_branches: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Storage for StateOverlay {
    fn save_block(&self, _block: &Block) -> Result<(), StorageError> {
        // We typically don't need to save blocks in overlay during execution,
        // but if validation needs to save it to be read back?
        // SimplexState::validate_and_store_block saves it.
        // But for validation we might just keep it in memory?
        // Let's pass through to inner? NO. Inner is persistent.
        // We should PROHIBIT saving blocks to persistent DB via overlay?
        // OR we just use a MemStorage for blocks in Overlay?
        // For this refactor, we are mostly concerned with STATE (Accounts/Storage).
        // Let's just error or ignore?
        // Actually, validate_and_store_block calls save_block.
        // If we use Overlay, we don't want to save to DB.
        // So we should mock it or ignore it.
        Ok(())
    }

    fn get_block(&self, hash: &Hash) -> Result<Option<Block>, StorageError> {
        self.inner.get_block(hash)
    }

    fn save_qc(&self, _qc: &QuorumCertificate) -> Result<(), StorageError> {
        // Overlay shouldn't be saving QCs usually, but if it does, ignore/mock.
        Ok(())
    }

    fn get_qc(&self, view: View) -> Result<Option<QuorumCertificate>, StorageError> {
        self.inner.get_qc(view)
    }

    fn save_consensus_state(&self, _state: &ConsensusState) -> Result<(), StorageError> {
        Ok(())
    }

    fn get_consensus_state(&self) -> Result<Option<ConsensusState>, StorageError> {
        self.inner.get_consensus_state()
    }

    // EVM State - Check Overlay First
    fn get_account(&self, address: &Address) -> Result<Option<AccountInfo>, StorageError> {
        if let Some(info) = self.accounts.lock().unwrap().get(address) {
            return Ok(Some(info.clone()));
        }
        self.inner.get_account(address)
    }

    fn save_account(&self, address: &Address, info: &AccountInfo) -> Result<(), StorageError> {
        self.accounts.lock().unwrap().insert(*address, info.clone());
        Ok(())
    }

    fn get_code(&self, hash: &Hash) -> Result<Option<Bytes>, StorageError> {
        if let Some(code) = self.code.lock().unwrap().get(hash) {
            return Ok(Some(code.clone()));
        }
        self.inner.get_code(hash)
    }

    fn save_code(&self, hash: &Hash, code: &Bytes) -> Result<(), StorageError> {
        self.code.lock().unwrap().insert(*hash, code.clone());
        Ok(())
    }

    fn get_storage(&self, address: &Address, index: &U256) -> Result<U256, StorageError> {
        if let Some(val) = self.storage.lock().unwrap().get(&(*address, *index)) {
            return Ok(*val);
        }
        self.inner.get_storage(address, index)
    }

    fn save_storage(
        &self,
        address: &Address,
        index: &U256,
        value: &U256,
    ) -> Result<(), StorageError> {
        self.storage
            .lock()
            .unwrap()
            .insert((*address, *index), *value);
        Ok(())
    }

    fn get_smt_branch(&self, height: u8, node_key: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        if let Some(node) = self.smt_branches.lock().unwrap().get(&(height, *node_key)) {
            return Ok(Some(node.clone()));
        }
        self.inner.get_smt_branch(height, node_key)
    }

    fn save_smt_branch(
        &self,
        height: u8,
        node_key: &Hash,
        node: &[u8],
    ) -> Result<(), StorageError> {
        self.smt_branches
            .lock()
            .unwrap()
            .insert((height, *node_key), node.to_vec());
        Ok(())
    }

    fn get_smt_leaf(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        if let Some(node) = self.smt_leaves.lock().unwrap().get(hash) {
            return Ok(Some(node.clone()));
        }
        self.inner.get_smt_leaf(hash)
    }

    fn save_smt_leaf(&self, hash: &Hash, node: &[u8]) -> Result<(), StorageError> {
        self.smt_leaves.lock().unwrap().insert(*hash, node.to_vec());
        Ok(())
    }
}
