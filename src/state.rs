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

use serde::{Deserialize, Serialize};
use sparse_merkle_tree::traits::{StoreReadOps, StoreWriteOps};
use sparse_merkle_tree::{BranchKey, BranchNode};

// --- Serialization Mirrors ---

#[derive(Serialize, Deserialize)]
enum SerdeMergeValue {
    Value([u8; 32]),
    MergeWithZero {
        base_node: [u8; 32],
        zero_bits: [u8; 32],
        zero_count: u8,
    },
    // ShortCut not supported (feature 'trie' off)
}

impl From<sparse_merkle_tree::merge::MergeValue> for SerdeMergeValue {
    fn from(v: sparse_merkle_tree::merge::MergeValue) -> Self {
        use sparse_merkle_tree::merge::MergeValue::*;
        match v {
            Value(h) => SerdeMergeValue::Value(h.into()),
            MergeWithZero {
                base_node,
                zero_bits,
                zero_count,
            } => SerdeMergeValue::MergeWithZero {
                base_node: base_node.into(),
                zero_bits: zero_bits.into(),
                zero_count,
            },
        }
    }
}

impl Into<sparse_merkle_tree::merge::MergeValue> for SerdeMergeValue {
    fn into(self) -> sparse_merkle_tree::merge::MergeValue {
        use sparse_merkle_tree::merge::MergeValue::*;
        match self {
            SerdeMergeValue::Value(h) => Value(H256::from(h)),
            SerdeMergeValue::MergeWithZero {
                base_node,
                zero_bits,
                zero_count,
            } => MergeWithZero {
                base_node: H256::from(base_node),
                zero_bits: H256::from(zero_bits),
                zero_count,
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SerdeBranchNode {
    left: SerdeMergeValue,
    right: SerdeMergeValue,
}

impl From<BranchNode> for SerdeBranchNode {
    fn from(n: BranchNode) -> Self {
        SerdeBranchNode {
            left: n.left.into(),
            right: n.right.into(),
        }
    }
}

impl Into<BranchNode> for SerdeBranchNode {
    fn into(self) -> BranchNode {
        BranchNode {
            left: self.left.into(),
            right: self.right.into(),
        }
    }
}

// --- Store Implementation ---

#[derive(Clone)]
pub struct OckhamSmtStore {
    storage: Arc<dyn Storage>,
}

impl OckhamSmtStore {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }
}

impl StoreReadOps<H256> for OckhamSmtStore {
    fn get_branch(
        &self,
        branch_key: &BranchKey,
    ) -> Result<Option<BranchNode>, sparse_merkle_tree::error::Error> {
        let node_hash = Hash(branch_key.node_key.into());
        match self.storage.get_smt_branch(branch_key.height, &node_hash) {
            Ok(Some(bytes)) => {
                let serde_node: SerdeBranchNode = bincode::deserialize(&bytes)
                    .map_err(|e| sparse_merkle_tree::error::Error::Store(e.to_string()))?;
                Ok(Some(serde_node.into()))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(sparse_merkle_tree::error::Error::Store(e.to_string())),
        }
    }

    fn get_leaf(&self, leaf_key: &H256) -> Result<Option<H256>, sparse_merkle_tree::error::Error> {
        let hash = Hash((*leaf_key).into());
        match self.storage.get_smt_leaf(&hash) {
            Ok(Some(bytes)) => {
                let val: [u8; 32] = bincode::deserialize(&bytes)
                    .map_err(|e| sparse_merkle_tree::error::Error::Store(e.to_string()))?;
                Ok(Some(H256::from(val)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(sparse_merkle_tree::error::Error::Store(e.to_string())),
        }
    }
}

impl StoreWriteOps<H256> for OckhamSmtStore {
    fn insert_branch(
        &mut self,
        node_key: BranchKey,
        branch: BranchNode,
    ) -> Result<(), sparse_merkle_tree::error::Error> {
        let serde_node: SerdeBranchNode = branch.into();
        let bytes = bincode::serialize(&serde_node)
            .map_err(|e| sparse_merkle_tree::error::Error::Store(e.to_string()))?;

        let hash = Hash(node_key.node_key.into());
        self.storage
            .save_smt_branch(node_key.height, &hash, &bytes)
            .map_err(|e| sparse_merkle_tree::error::Error::Store(e.to_string()))
    }

    fn insert_leaf(
        &mut self,
        leaf_key: H256,
        leaf: H256,
    ) -> Result<(), sparse_merkle_tree::error::Error> {
        let leaf_bytes: [u8; 32] = leaf.into();
        let bytes = bincode::serialize(&leaf_bytes)
            .map_err(|e| sparse_merkle_tree::error::Error::Store(e.to_string()))?;

        let hash = Hash(leaf_key.into());
        self.storage
            .save_smt_leaf(&hash, &bytes)
            .map_err(|e| sparse_merkle_tree::error::Error::Store(e.to_string()))
    }

    fn remove_branch(
        &mut self,
        _node_key: &BranchKey,
    ) -> Result<(), sparse_merkle_tree::error::Error> {
        Ok(())
    }

    fn remove_leaf(&mut self, _leaf_key: &H256) -> Result<(), sparse_merkle_tree::error::Error> {
        Ok(())
    }
}

pub type SmtStore = OckhamSmtStore;
pub type StateTree = SparseMerkleTree<sparse_merkle_tree::blake2b::Blake2bHasher, H256, SmtStore>;

pub struct StateManager {
    tree: Arc<Mutex<StateTree>>,
    storage: Arc<dyn Storage>,
}

impl StateManager {
    // Keep signature compatible with tests (ignoring initial_root for now)
    pub fn new(storage: Arc<dyn Storage>, initial_root: Option<Hash>) -> Self {
        let store = SmtStore::new(storage.clone());
        let root = initial_root
            .map(|h| H256::from(h.0))
            .unwrap_or(H256::zero());
        let tree = SparseMerkleTree::new(root, store);
        Self {
            tree: Arc::new(Mutex::new(tree)),
            storage,
        }
    }

    pub fn new_from_tree(storage: Arc<dyn Storage>, tree: StateTree) -> Self {
        Self {
            tree: Arc::new(Mutex::new(tree)),
            storage,
        }
    }

    pub fn fork(&self, new_root: Hash, storage: Arc<dyn Storage>) -> Self {
        // Create a new SmtStore backed by the provided storage (e.g. Overlay)
        let store = SmtStore::new(storage.clone());
        let new_tree = SparseMerkleTree::new(sparse_merkle_tree::H256::from(new_root.0), store);
        Self {
            tree: Arc::new(Mutex::new(new_tree)),
            storage,
        }
    }

    pub fn snapshot(&self) -> StateTree {
        let tree = self.tree.lock().unwrap();
        let root = *tree.root();
        let store = tree.store().clone();
        SparseMerkleTree::new(root, store)
    }

    pub fn update_account(&self, address: Address, account_hash: Hash) -> Result<Hash, StateError> {
        let key_hash = keccak256(address);
        let key = H256::from(key_hash.0);
        let value = H256::from(account_hash.0);

        let mut tree = self.tree.lock().unwrap();
        tree.update(key, value)
            .map_err(|e| StateError::Smt(format!("{:?}", e)))?;

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
        self.storage
            .save_account(&address, &info)
            .map_err(|e| StateError::Smt(e.to_string()))?;

        let hash = hash_data(&info);
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

    pub fn get_consensus_state(
        &self,
    ) -> Result<Option<crate::storage::ConsensusState>, StateError> {
        self.storage
            .get_consensus_state()
            .map_err(|e| StateError::Smt(e.to_string()))
    }

    pub fn save_consensus_state(
        &self,
        state: &crate::storage::ConsensusState,
    ) -> Result<(), StateError> {
        self.storage
            .save_consensus_state(state)
            .map_err(|e| StateError::Smt(e.to_string()))
    }
}

impl Database for StateManager {
    type Error = StateError;

    fn basic(&mut self, address: Address) -> Result<Option<RevmAccountInfo>, Self::Error> {
        if let Some(info) = self
            .storage
            .get_account(&address)
            .map_err(|e| StateError::Smt(e.to_string()))?
        {
            let code = if let Some(c) = info.code {
                Some(Bytecode::new_raw(c))
            } else if info.code_hash != Hash::default() {
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
        Ok(B256::ZERO)
    }
}
