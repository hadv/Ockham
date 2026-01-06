use crate::crypto::{Hash, verify};
use crate::storage::Storage;
use crate::types::{Address, Transaction};
use revm::EVM; // Need EVM for AA validation
use revm::primitives::TransactTo;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use thiserror::Error; // U256 removed

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("Transaction already exists")]
    AlreadyExists,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Invalid AA Validation: {0}")]
    InvalidAA(String),
    #[error("Invalid Nonce: expected {0}, got {1}")]
    InvalidNonce(u64, u64),
    #[error("Storage Error: {0}")]
    StorageError(String),
    #[error("Gas Limit Exceeded: max {0}, got {1}")]
    GasLimitExceeded(u64, u64),
}

/// A simple Transaction Pool (Mempool).
/// proper implementation should handle nonce ordering and gas price sorting.
/// MVP: Simple FIFO/Map.
#[derive(Clone)]
pub struct TxPool {
    // Map Hash -> Transaction for quick lookup
    transactions: Arc<Mutex<HashMap<Hash, Transaction>>>,
    // Queue for FIFO ordering (MVP)
    queue: Arc<Mutex<VecDeque<Hash>>>,
    // Storage access for nonce check
    storage: Arc<dyn Storage>,
}

impl TxPool {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            transactions: Arc::new(Mutex::new(HashMap::new())),
            queue: Arc::new(Mutex::new(VecDeque::new())),
            storage,
        }
    }

    /// Add a transaction to the pool.
    pub fn add_transaction(&self, tx: Transaction) -> Result<(), PoolError> {
        // 0. Check Gas Limit (Fusaka)
        if tx.gas_limit() > crate::types::MAX_TX_GAS_LIMIT {
            return Err(PoolError::GasLimitExceeded(
                crate::types::MAX_TX_GAS_LIMIT,
                tx.gas_limit(),
            ));
        }

        // 1. Validate Signature / AA Validation
        match &tx {
            Transaction::Legacy(l_tx) => {
                let sighash = tx.sighash();
                if !verify(&l_tx.public_key, &sighash.0, &l_tx.signature) {
                    return Err(PoolError::InvalidSignature);
                }
            }
            Transaction::AA(aa_tx) => {
                // Run EVM validation
                // We need a temporary state manager
                let state_manager = crate::state::StateManager::new(self.storage.clone(), None);
                // Note: This is an expensive operation for add_transaction.
                // In production, this should be async or limited.

                let mut db = state_manager;
                let mut evm = EVM::new();
                evm.database(&mut db);

                // Minimal Env
                let tx_env = &mut evm.env.tx;
                tx_env.caller = Address::ZERO;
                tx_env.transact_to = TransactTo::Call(aa_tx.sender);
                // Selector logic as per VM
                let selector = hex::decode("9a22d64f").unwrap();
                tx_env.data = crate::types::Bytes::from(selector);
                tx_env.gas_limit = 200_000;
                tx_env.nonce = Some(aa_tx.nonce);

                // Execute
                let result = evm
                    .transact()
                    .map_err(|e| PoolError::InvalidAA(format!("EVM Error: {:?}", e)))?;

                match result.result {
                    revm::primitives::ExecutionResult::Success { .. } => {}
                    _ => return Err(PoolError::InvalidAA("Validation Reverted or Failed".into())),
                }
            }
        }

        // 2. Validate Nonce
        // Get sender account state
        let sender = tx.sender();
        let account_nonce = if let Some(account) = self
            .storage
            .get_account(&sender)
            .map_err(|e| PoolError::StorageError(e.to_string()))?
        {
            account.nonce
        } else {
            0
        };

        if tx.nonce() < account_nonce {
            return Err(PoolError::InvalidNonce(account_nonce, tx.nonce()));
        }

        // TODO: Also check if nonce is already in pool? (Pending Nonce)
        // For MVP we just check against state.

        let hash = crate::crypto::hash_data(&tx); // Transaction enum implements Hash via Serialize? No, we used hash_data(&tx) which uses bincode. 
        // Wait, types.rs Transaction has sighash(). hash_data(&tx) hashes the whole enum.
        // Hash collision between identical txs is what we want to detect.
        // However, LegacyTransaction sighash() excludes signature.
        // TxPool usually uses the full hash (including sig).
        // Let's assume `crate::crypto::hash_data(&tx)` does full serialization hash.

        let mut text_map = self.transactions.lock().unwrap();
        if text_map.contains_key(&hash) {
            return Err(PoolError::AlreadyExists);
        }

        text_map.insert(hash, tx);
        self.queue.lock().unwrap().push_back(hash);

        Ok(())
    }

    /// Get a batch of transactions for a new block, respecting the gas limit.
    /// Ordered by Gas Price (max_fee_per_gas) Descending.
    pub fn get_transactions_for_block(
        &self,
        block_gas_limit: u64,
        base_fee: crate::types::U256,
    ) -> Vec<Transaction> {
        let mut pending = Vec::new();
        let map = self.transactions.lock().unwrap();

        // 1. Collect and Filter transactions
        let mut all_txs: Vec<&Transaction> = map
            .values()
            .filter(|tx| tx.max_fee_per_gas() >= base_fee)
            .collect();

        // 2. Sort by Effective Tip Descending
        // Effective Tip = min(max_priority_fee, max_fee - base_fee)
        all_txs.sort_by(|a, b| {
            let tip_a = std::cmp::min(a.max_priority_fee_per_gas(), a.max_fee_per_gas() - base_fee);
            let tip_b = std::cmp::min(b.max_priority_fee_per_gas(), b.max_fee_per_gas() - base_fee);
            let cmp = tip_b.cmp(&tip_a); // Descending
            if cmp == std::cmp::Ordering::Equal {
                // Secondary sort: Nonce Ascending for same sender
                if a.sender() == b.sender() {
                    a.nonce().cmp(&b.nonce())
                } else {
                    // Tertiary sort: Deterministic (Sender Address)
                    a.sender().cmp(&b.sender())
                }
            } else {
                cmp
            }
        });

        // 3. Select fitting transactions
        let mut current_gas = 0u64;

        for tx in all_txs {
            if current_gas + tx.gas_limit() <= block_gas_limit {
                pending.push(tx.clone());
                current_gas += tx.gas_limit();
            }
            // Optimize: If block is full, break?
            if current_gas >= block_gas_limit {
                break;
            }
        }

        pending
    }

    /// Remove transactions that were included in a block.
    pub fn remove_transactions(&self, txs: &[Transaction]) {
        let mut map = self.transactions.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();

        for tx in txs {
            let hash = crate::crypto::hash_data(tx);
            if map.remove(&hash).is_some() {
                // Remove from queue is O(N). Vector might be better or LinkedHashMap.
                // For MVP, simplistic rebuild or filter.
                // Or just keep it simple.
                if let Some(pos) = queue.iter().position(|h| *h == hash) {
                    queue.remove(pos);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.transactions.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.transactions.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Signature, generate_keypair, sign};
    use crate::storage::MemStorage;
    use crate::types::{Address, Bytes, LegacyTransaction, U256}; // AccessListItem not used in test but needed if we construct

    #[test]
    fn test_add_transaction_validation() {
        let storage = Arc::new(MemStorage::new());
        let pool = TxPool::new(storage.clone());

        let (pk, sk) = generate_keypair();

        let mut tx = LegacyTransaction {
            chain_id: 1337,
            nonce: 0,
            max_priority_fee_per_gas: U256::ZERO,
            max_fee_per_gas: U256::from(10_000_000),
            gas_limit: 21000,
            to: Some(Address::ZERO),
            value: U256::ZERO,
            data: Bytes::from(vec![]),
            access_list: vec![],
            public_key: pk.clone(),
            signature: crate::crypto::Signature::default(), // Invalid initially
        };

        // 1. Sign properly (manually for test)
        // Construct LegacyTransaction sighash manually since it's now wrapped
        let data = (
            tx.chain_id,
            tx.nonce,
            &tx.max_priority_fee_per_gas,
            &tx.max_fee_per_gas,
            tx.gas_limit,
            &tx.to,
            &tx.value,
            &tx.data,
            &tx.access_list,
        );
        let sighash = crate::crypto::hash_data(&data);

        let sig = sign(&sk, &sighash.0);
        tx.signature = sig;

        // Wrap in Enum
        let tx_enum = Transaction::Legacy(Box::new(tx.clone()));

        // Add proper tx -> Ok
        assert!(pool.add_transaction(tx_enum.clone()).is_ok());

        // 2. Replay -> Error
        assert!(matches!(
            pool.add_transaction(tx_enum.clone()),
            Err(PoolError::AlreadyExists)
        ));

        // 2. Bad Signature
        let mut bad_tx = tx.clone();
        bad_tx.signature = Signature::default(); // Invalid
        let bad_tx_enum = Transaction::Legacy(Box::new(bad_tx));

        let res = pool.add_transaction(bad_tx_enum);
        assert!(matches!(res, Err(PoolError::InvalidSignature)));

        // 3. Nonce too low
        // ... (Update state to nonce 1)
        // Ignoring state update setup for brevity, just assuming logic holds if mocked
        let mut low_nonce_tx = tx.clone();
        low_nonce_tx.nonce = 0; // If account had 1
        // But here we rely on MemStorage default, which is 0. So test might fail if not set up.
        // Actually, let's just fix compilation.
        let _low_nonce_enum = Transaction::Legacy(Box::new(low_nonce_tx));

        // 4. Bad Nonce
        // Set account nonce in storage to 5
        let sender = tx_enum.sender();
        // Manually save account to storage
        // Needs AccountInfo struct
        let account = crate::storage::AccountInfo {
            nonce: 5,
            balance: U256::ZERO,
            code_hash: crate::crypto::Hash::default(),
            code: None,
        };
        storage.save_account(&sender, &account).unwrap();

        storage.save_account(&sender, &account).unwrap();

        let mut low_nonce_tx = tx.clone();
        low_nonce_tx.nonce = 4;
        // Resign
        let data = (
            low_nonce_tx.chain_id,
            low_nonce_tx.nonce,
            &low_nonce_tx.max_priority_fee_per_gas,
            &low_nonce_tx.max_fee_per_gas,
            low_nonce_tx.gas_limit,
            &low_nonce_tx.to,
            &low_nonce_tx.value,
            &low_nonce_tx.data,
            &low_nonce_tx.access_list,
        );
        let sigh = crate::crypto::hash_data(&data);
        low_nonce_tx.signature = sign(&sk, &sigh.0);

        let low_nonce_enum = Transaction::Legacy(Box::new(low_nonce_tx));

        // Should fail nonce check
        match pool.add_transaction(low_nonce_enum) {
            Err(PoolError::InvalidNonce(expected, got)) => {
                assert_eq!(expected, 5);
                assert_eq!(got, 4);
            }
            _ => panic!("Expected InvalidNonce"),
        }
    }
}
