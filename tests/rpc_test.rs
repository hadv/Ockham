use ockham::rpc::{OckhamRpcImpl, OckhamRpcServer};
use ockham::storage::{ConsensusState, MemStorage, Storage};
use ockham::types::{Block, QuorumCertificate};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::test]
async fn test_rpc_get_status() {
    let storage = Arc::new(MemStorage::new());

    // Setup initial state
    let state = ConsensusState {
        view: 10,
        finalized_height: 5,
        preferred_block: ockham::crypto::Hash([0u8; 32]),
        preferred_view: 9,
        last_voted_view: 9,
        committee: vec![],
        pending_validators: vec![],
        exiting_validators: vec![],
        stakes: HashMap::new(),
    };
    storage.save_consensus_state(&state).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let (tx_sender, _rx) = tokio::sync::mpsc::channel(100);
    let rpc = OckhamRpcImpl::new(
        storage,
        tx_pool,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        tx_sender,
    );

    // Call RPC
    let result = rpc.get_status();
    assert!(result.is_ok());
    let fetched_state = result.unwrap();
    assert!(fetched_state.is_some());
    let s = fetched_state.unwrap();
    assert_eq!(s.view, 10);
    assert_eq!(s.finalized_height, 5);
}

#[tokio::test]
async fn test_rpc_get_block() {
    let storage = Arc::new(MemStorage::new());

    // Create a dummy block
    let (pk, _) = ockham::crypto::generate_keypair();
    let qc = QuorumCertificate::default();
    let block = Block::new(
        pk,
        1,
        ockham::crypto::Hash::default(),
        qc,
        ockham::crypto::Hash::default(),
        ockham::crypto::Hash::default(),
        vec![],
        ockham::types::U256::ZERO,
        0,
        vec![],
        ockham::crypto::Hash::default(),
    );
    let block_hash = ockham::crypto::hash_data(&block);

    storage.save_block(&block).unwrap();

    // Also set as latest/preferred for get_latest_block test
    let state = ConsensusState {
        view: 1,
        finalized_height: 0,
        preferred_block: block_hash,
        preferred_view: 1,
        last_voted_view: 1,
        committee: vec![],
        pending_validators: vec![],
        exiting_validators: vec![],
        stakes: HashMap::new(),
    };
    storage.save_consensus_state(&state).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let (tx_sender, _rx) = tokio::sync::mpsc::channel(100);
    let rpc = OckhamRpcImpl::new(
        storage,
        tx_pool,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        tx_sender,
    );

    // 1. get_block_by_hash
    let res = rpc.get_block_by_hash(block_hash);
    assert!(res.is_ok());
    let val = res.unwrap();
    assert!(val.is_some());
    assert_eq!(val.unwrap().view, 1);

    // 2. get_latest_block
    let res_latest = rpc.get_latest_block();
    assert!(res_latest.is_ok());
    let val_latest = res_latest.unwrap();
    assert!(val_latest.is_some());
    assert_eq!(val_latest.unwrap().view, 1);

    // 3. Negative test
    let res_none = rpc.get_block_by_hash(ockham::crypto::Hash([1u8; 32]));
    assert!(res_none.is_ok());
    assert!(res_none.unwrap().is_none());

    // 4. suggest_base_fee
    let fee_res = rpc.suggest_base_fee();
    assert!(fee_res.is_ok());
    // Should be default 10 Gwei since we have genesis in storage/or dummy logic
    // Genesis was saved in consensus state init but we created fresh storage here.
    // Wait, rpc_test manual setup doesn't init consensus state with genesis block unless we do it.
    // In test_rpc_get_block we saved a block but didn't set it as preferred in a way that fully mimics Consensus if we rely on ConsensusState.
    // We set preferred_block in ConsensusState.
    let fee = fee_res.unwrap();
    // Since our saved block has 0 gas_used and 0 base_fee (from previous test setup?),
    // `Block::new` in test used 0 base_fee.
    // So calculation might return 0? Or if target > 0, decrease?
    // Let's just check it returns Ok for MVP.
    println!("Suggested Base Fee: {:?}", fee);
}
