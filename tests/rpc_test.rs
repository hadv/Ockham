use ockham::rpc::{OckhamRpcImpl, OckhamRpcServer};
use ockham::storage::{ConsensusState, MemStorage, Storage};
use ockham::types::{Block, QuorumCertificate};
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
    };
    storage.save_consensus_state(&state).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new());
    let rpc = OckhamRpcImpl::new(storage, tx_pool);

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
    let block = Block::new(pk, 1, ockham::crypto::Hash::default(), qc, ockham::crypto::Hash::default(), ockham::crypto::Hash::default(), vec![]);
    let block_hash = ockham::crypto::hash_data(&block);

    storage.save_block(&block).unwrap();

    // Also set as latest/preferred for get_latest_block test
    let state = ConsensusState {
        view: 1,
        finalized_height: 0,
        preferred_block: block_hash,
        preferred_view: 1,
    };
    storage.save_consensus_state(&state).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new());
    let rpc = OckhamRpcImpl::new(storage, tx_pool);

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
}
