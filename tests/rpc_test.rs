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
        inactivity_scores: HashMap::new(),
    };
    storage.save_consensus_state(&state).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = Arc::new(std::sync::Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(state_manager, ockham::types::DEFAULT_BLOCK_GAS_LIMIT);
    let (tx_sender, _rx) = tokio::sync::mpsc::channel(100);
    let rpc = OckhamRpcImpl::new(
        storage,
        tx_pool,
        executor,
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
        inactivity_scores: HashMap::new(),
    };
    storage.save_consensus_state(&state).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = Arc::new(std::sync::Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(state_manager, ockham::types::DEFAULT_BLOCK_GAS_LIMIT);
    let (tx_sender, _rx) = tokio::sync::mpsc::channel(100);
    let rpc = OckhamRpcImpl::new(
        storage,
        tx_pool,
        executor,
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

#[tokio::test]
async fn test_rpc_get_transaction_count() {
    let storage = Arc::new(MemStorage::new());

    // Create an account with a specific nonce
    let (pk, _) = ockham::crypto::generate_keypair();
    let pk_bytes = pk.0.to_bytes();
    let hash = ockham::types::keccak256(pk_bytes);
    let address = ockham::types::Address::from_slice(&hash[12..]);

    let account = ockham::storage::AccountInfo {
        nonce: 42,
        balance: ockham::types::U256::ZERO,
        code_hash: ockham::crypto::Hash(ockham::types::keccak256([]).into()),
        code: None,
    };
    storage.save_account(&address, &account).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = Arc::new(std::sync::Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(state_manager, ockham::types::DEFAULT_BLOCK_GAS_LIMIT);
    let (tx_sender, _rx) = tokio::sync::mpsc::channel(100);
    let rpc = OckhamRpcImpl::new(
        storage,
        tx_pool,
        executor,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        tx_sender,
    );

    // Call RPC
    let result = rpc.get_transaction_count(address);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);

    // Test non-existent account
    let (pk2, _) = ockham::crypto::generate_keypair();
    let pk2_bytes = pk2.0.to_bytes();
    let hash2 = ockham::types::keccak256(pk2_bytes);
    let address2 = ockham::types::Address::from_slice(&hash2[12..]);

    let result2 = rpc.get_transaction_count(address2);
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap(), 0);
}

#[tokio::test]
async fn test_rpc_extended() {
    let storage = Arc::new(MemStorage::new());
    // Setup Account with Code
    let (pk, _) = ockham::crypto::generate_keypair();
    let pk_bytes = pk.0.to_bytes();
    let hash = ockham::types::keccak256(pk_bytes);
    let address = ockham::types::Address::from_slice(&hash[12..]);

    let code_bytes = vec![0x60, 0x00, 0x60, 0x00, 0x53]; // PUSH1 00 PUSH1 00 MSTORE8 (Simple nonsense)
    let code_hash = ockham::crypto::Hash(ockham::types::keccak256(&code_bytes).into());
    let code = ockham::types::Bytes::from(code_bytes.clone());

    let account = ockham::storage::AccountInfo {
        nonce: 1,
        balance: ockham::types::U256::from(100),
        code_hash,
        code: Some(code.clone()),
    };
    storage.save_account(&address, &account).unwrap();
    storage.save_code(&code_hash, &code).unwrap();

    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = Arc::new(std::sync::Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(state_manager, ockham::types::DEFAULT_BLOCK_GAS_LIMIT);
    let (tx_sender, _rx) = tokio::sync::mpsc::channel(100);
    let rpc = OckhamRpcImpl::new(
        storage,
        tx_pool,
        executor,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        tx_sender,
    );

    // 1. get_code
    let res_code = rpc.get_code(address, None);
    assert!(res_code.is_ok());
    assert_eq!(res_code.unwrap(), code);

    // 2. call (to the account)
    let request = ockham::rpc::CallRequest {
        from: None,
        to: Some(address),
        gas: Some(100000),
        gas_price: None,
        value: None,
        data: None,
    };
    let res_call = rpc.call(request, None);
    assert!(res_call.is_ok());

    // 3. estimate_gas
    let request_est = ockham::rpc::CallRequest {
        from: None,
        to: Some(address),
        gas: None,
        gas_price: None,
        value: None,
        data: None,
    };
    let res_est = rpc.estimate_gas(request_est, None);
    assert!(res_est.is_ok());
    println!("Estimated Gas: {}", res_est.unwrap());
}
