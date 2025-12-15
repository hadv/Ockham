use crate::crypto::{Hash, hash_data};
use alloy_primitives::{Address, keccak256};

use crate::storage::Storage;
use revm::Database;
use revm::primitives::{AccountInfo as RevmAccountInfo, B256, Bytecode, U256};
use sparse_merkle_tree::{H256, SparseMerkleTree};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Error type for State operations
#[derive(Debug, Error)]
pub enum StateError {
    #[error("SMT Error: {0}")]
    Smt(String),
}

/// We use the default `Blake2bHasher` provided by the crate for the Tree structure itself.
/// We can still use Keccak for leaf keys before inserting.
pub type SmtStore = sparse_merkle_tree::default_store::DefaultStore<H256>;

pub type StateTree = SparseMerkleTree<sparse_merkle_tree::blake2b::Blake2bHasher, H256, SmtStore>;

pub struct StateManager {
    tree: Arc<Mutex<StateTree>>,
    storage: Arc<dyn Storage>,
}

impl StateManager {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        let store = SmtStore::default();
        let tree = SparseMerkleTree::new(H256::zero(), store);
        Self {
            tree: Arc::new(Mutex::new(tree)),
            storage,
        }
    }

    pub fn update_account(&self, address: Address, account_hash: Hash) -> Result<Hash, StateError> {
        // Convert Address (20 bytes) to H256 (32 bytes) for the key.
        // We can just pad it or hash it. Hashing it is safer for distribution.
        let key_hash = keccak256(address);
        let key = H256::from(key_hash.0);

        // Value is the hash of the AccountInfo
        let value = H256::from(account_hash.0);

        let mut tree = self.tree.lock().unwrap();
        tree.update(key, value)
            .map_err(|e| StateError::Smt(format!("{:?}", e)))?;

        // Also save to storage?
        // We assume account_info is already saved in TABLE_ACCOUNTS by caller.
        // If not, we should probably take AccountInfo here too.

        let root = tree.root();
        let mut root_bytes = [0u8; 32];
        root_bytes.copy_from_slice(root.as_slice());
        Ok(Hash(root_bytes))
    }

    pub fn root(&self) -> Hash {
        let tree = self.tree.lock().unwrap();
        let mut root_bytes = [0u8; 32];
        root_bytes.copy_from_slice(tree.root().as_slice());
        Hash(root_bytes)
    }

    pub fn commit_account(
        &self,
        address: Address,
        info: crate::storage::AccountInfo,
    ) -> Result<(), StateError> {
        // 1. Save full account data to persistent storage (for VM execution)
        self.storage
            .save_account(&address, &info)
            .map_err(|e| StateError::Smt(e.to_string()))?;

        // 2. Hash the account info (Serialize -> Hash)
        let hash = hash_data(&info);

        // 3. Update the SMT (Commitment)
        self.update_account(address, hash)?;

        Ok(())
    }

    pub fn commit_storage(
        &self,
        address: Address,
        index: U256,
        value: U256,
    ) -> Result<(), StateError> {
        self.storage
            .save_storage(&address, &index, &value)
            .map_err(|e| StateError::Smt(e.to_string()))
    }
}

impl Database for StateManager {
    type Error = StateError;

    fn basic(&mut self, address: Address) -> Result<Option<RevmAccountInfo>, Self::Error> {
        // Fetch from storage
        if let Some(info) = self
            .storage
            .get_account(&address)
            .map_err(|e| StateError::Smt(e.to_string()))?
        {
            let code = if let Some(c) = info.code {
                Some(Bytecode::new_raw(c))
            } else if info.code_hash != Hash::default() {
                // Fetch code by hash
                let code_bytes = self
                    .storage
                    .get_code(&info.code_hash)
                    .map_err(|e| StateError::Smt(e.to_string()))?;
                code_bytes.map(Bytecode::new_raw)
            } else {
                None
            };

            Ok(Some(RevmAccountInfo {
                balance: info.balance,
                nonce: info.nonce,
                code_hash: B256::from(info.code_hash.0),
                code,
            }))
        } else {
            Ok(None)
        }
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let code_bytes = self
            .storage
            .get_code(&Hash(code_hash.0))
            .map_err(|e| StateError::Smt(e.to_string()))?;
        if let Some(bytes) = code_bytes {
            Ok(Bytecode::new_raw(bytes))
        } else {
            Ok(Bytecode::default())
        }
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage
            .get_storage(&address, &index)
            .map_err(|e| StateError::Smt(e.to_string()))
    }

    fn block_hash(&mut self, _number: U256) -> Result<B256, Self::Error> {
        // TODO: Implement block hash lookup by number
        // For now return zero
        Ok(B256::ZERO)
    }
}
