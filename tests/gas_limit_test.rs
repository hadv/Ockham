use ockham::crypto::{Hash, generate_keypair, sign};
use ockham::state::StateManager;
use ockham::storage::{AccountInfo, MemStorage, Storage};
use ockham::tx_pool::{PoolError, TxPool};
use ockham::types::{
    Address, Block, Bytes, MAX_TX_GAS_LIMIT, QuorumCertificate, Transaction, U256,
};
use ockham::vm::{ExecutionError, Executor};
use std::sync::{Arc, Mutex};

#[test]
fn test_transaction_gas_limit() {
    // Generate keys
    let (pk, sk) = generate_keypair();

    let storage = Arc::new(MemStorage::new());
    {
        // Fund the sender
        let addr = ockham::types::keccak256(pk.0.to_bytes());
        let address = Address::from_slice(&addr[12..]);
        let account = AccountInfo {
            nonce: 0,
            balance: U256::from(10_000_000_000_000_000_000u64), // 10 ETH
            code_hash: Hash(ockham::types::keccak256(&[]).0),   // Empty Code Hash
            code: None,
        };
        storage.save_account(&address, &account).unwrap();
    }

    let state = Arc::new(Mutex::new(StateManager::new(storage, None)));
    // Block limit 30M, Tx limit 16.7M
    let executor = Executor::new(state, 30_000_000);

    // 1. Valid Tx (Below Limit)
    let mut tx_ok = Transaction {
        chain_id: 1,
        nonce: 0,
        max_priority_fee_per_gas: U256::from(1_000_000_000u64), // 1 Gwei
        max_fee_per_gas: U256::from(10_000_000_000u64),         // 10 Gwei
        gas_limit: MAX_TX_GAS_LIMIT,                            // Borderline OK
        to: None,
        value: U256::ZERO,
        data: Bytes::new(),
        access_list: vec![],
        public_key: pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    tx_ok.signature = sign(&sk, &tx_ok.sighash().0);

    let mut block = Block::new_dummy(pk.clone(), 1, Hash::default(), QuorumCertificate::default());
    block.base_fee_per_gas = U256::from(10_000_000); // 0.01 Gwei
    block.payload = vec![tx_ok];

    if let Err(e) = executor.execute_block(&mut block) {
        panic!("Valid Transaction Failed: {:?}", e);
    }

    // 2. Invalid Tx (Above Limit)
    let mut tx_bad = Transaction {
        chain_id: 1,
        nonce: 1,
        max_priority_fee_per_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        gas_limit: MAX_TX_GAS_LIMIT + 1, // Exceeds
        to: None,
        value: U256::ZERO,
        data: Bytes::new(),
        access_list: vec![],
        public_key: pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    tx_bad.signature = sign(&sk, &tx_bad.sighash().0);

    let mut block_bad =
        Block::new_dummy(pk.clone(), 2, Hash::default(), QuorumCertificate::default());
    block_bad.payload = vec![tx_bad];

    let res = executor.execute_block(&mut block_bad);
    match res {
        Err(ExecutionError::Transaction(msg)) => {
            assert!(msg.contains("Fusaka"));
        }
        _ => panic!(
            "Expected Transaction Error with Fusaka message, got {:?}",
            res
        ),
    }
}

#[test]
fn test_pool_gas_limit() {
    let storage = Arc::new(MemStorage::new());
    let pool = TxPool::new(storage);
    let (pk, sk) = generate_keypair();

    let mut tx = Transaction {
        chain_id: 1,
        nonce: 0,
        max_priority_fee_per_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        gas_limit: MAX_TX_GAS_LIMIT + 1, // Exceeds
        to: None,
        value: U256::ZERO,
        data: Bytes::new(),
        access_list: vec![],
        public_key: pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    tx.signature = sign(&sk, &tx.sighash().0);

    let res = pool.add_transaction(tx);
    assert!(matches!(res, Err(PoolError::GasLimitExceeded(..))));
}
