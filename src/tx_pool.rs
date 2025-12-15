use crate::crypto::{Hash, verify};
use crate::storage::Storage;
use crate::types::Transaction;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("Transaction already exists")]
    AlreadyExists,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Invalid Nonce: expected {0}, got {1}")]
    InvalidNonce(u64, u64),
    #[error("Storage Error: {0}")]
    StorageError(String),
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
        // 1. Validate Signature
        let sighash = tx.sighash();
        if !verify(&tx.public_key, &sighash.0, &tx.signature) {
            return Err(PoolError::InvalidSignature);
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

        if tx.nonce < account_nonce {
            return Err(PoolError::InvalidNonce(account_nonce, tx.nonce));
        }

        // TODO: Also check if nonce is already in pool? (Pending Nonce)
        // For MVP we just check against state.

        let hash = crate::crypto::hash_data(&tx);

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
            .filter(|tx| tx.max_fee_per_gas >= base_fee)
            .collect();

        // 2. Sort by Effective Tip Descending
        // Effective Tip = min(max_priority_fee, max_fee - base_fee)
        all_txs.sort_by(|a, b| {
            let tip_a = std::cmp::min(a.max_priority_fee_per_gas, a.max_fee_per_gas - base_fee);
            let tip_b = std::cmp::min(b.max_priority_fee_per_gas, b.max_fee_per_gas - base_fee);
            tip_b.cmp(&tip_a) // Descending
        });

        // 3. Select fitting transactions
        let mut current_gas = 0u64;

        for tx in all_txs {
            if current_gas + tx.gas_limit <= block_gas_limit {
                pending.push(tx.clone());
                current_gas += tx.gas_limit;
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
    use crate::crypto::{generate_keypair, sign};
    use crate::storage::MemStorage;
    use crate::types::{Address, Bytes, U256}; // AccessListItem not used in test but needed if we construct

    #[test]
    fn test_add_transaction_validation() {
        let storage = Arc::new(MemStorage::new());
        let pool = TxPool::new(storage.clone());

        let (pk, sk) = generate_keypair();

        let mut tx = Transaction {
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

        // 1. Sign properly
        let sighash = tx.sighash();
        let sig = sign(&sk, &sighash.0);
        tx.signature = sig;

        // Add proper tx -> Ok
        assert!(pool.add_transaction(tx.clone()).is_ok());

        // 2. Replay -> Error
        assert!(matches!(
            pool.add_transaction(tx.clone()),
            Err(PoolError::AlreadyExists)
        ));

        // 3. Bad Signature
        let mut bad_tx = tx.clone();
        bad_tx.nonce = 1; // Change body => sighash changes
        // Signature remains for nonce 0 => Invalid
        assert!(matches!(
            pool.add_transaction(bad_tx).unwrap_err(),
            PoolError::InvalidSignature
        ));

        // 4. Bad Nonce
        // Set account nonce in storage to 5
        let sender = tx.sender();
        // Manually save account to storage
        // Needs AccountInfo struct
        let account = crate::storage::AccountInfo {
            nonce: 5,
            balance: U256::ZERO,
            code_hash: crate::crypto::Hash::default(),
            code: None,
        };
        storage.save_account(&sender, &account).unwrap();

        let mut low_nonce_tx = tx.clone();
        low_nonce_tx.nonce = 4;
        let sigh = low_nonce_tx.sighash();
        low_nonce_tx.signature = sign(&sk, &sigh.0);

        // Should fail nonce check
        match pool.add_transaction(low_nonce_tx) {
            Err(PoolError::InvalidNonce(expected, got)) => {
                assert_eq!(expected, 5);
                assert_eq!(got, 4);
            }
            _ => panic!("Expected InvalidNonce"),
        }
    }
}
