use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{PrivateKey, PublicKey, hash_data};
use ockham::types::{Address, Bytes, Transaction, U256};

#[test]
fn test_state_ommitment_on_finalization() {
    let keys: Vec<(PublicKey, PrivateKey)> = (0..2)
        .map(|i| ockham::crypto::generate_keypair_from_id(i as u64))
        .collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    let setup_node = |i: usize| -> SimplexState {
        let storage = std::sync::Arc::new(ockham::storage::MemStorage::new());
        let tx_pool = std::sync::Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
        let state_manager = std::sync::Arc::new(std::sync::Mutex::new(
            ockham::state::StateManager::new(storage.clone(), None),
        ));
        let executor = ockham::vm::Executor::new(
            state_manager.clone(),
            ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        );
        SimplexState::new(
            keys[i].0.clone(),
            keys[i].1.clone(),
            committee.clone(),
            storage,
            tx_pool,
            executor,
            ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
        )
    };

    let mut node0 = setup_node(0);
    let mut node1 = setup_node(1);

    let receiver_addr = Address::ZERO;

    // Tx needs to be signed by Node 0 (Sender) and put in Node 1's Pool (Leader View 1)
    let dummy_sig = ockham::crypto::sign(&keys[0].1, &[0u8; 32]);
    let mut tx = Transaction {
        chain_id: 1,
        nonce: 0,
        max_priority_fee_per_gas: U256::from(1_000_000),
        max_fee_per_gas: U256::from(20_000_000),
        gas_limit: 21000,
        to: Some(receiver_addr),
        value: U256::from(1000),
        data: Bytes::default(),
        access_list: vec![],
        public_key: keys[0].0.clone(),
        signature: dummy_sig,
    };

    let sighash = tx.sighash();
    tx.signature = ockham::crypto::sign(&keys[0].1, &sighash.0);

    node1.tx_pool.add_transaction(tx.clone()).unwrap();

    // Node 1 Proposes Block
    let actions = node1.try_propose().unwrap();
    let block_1 = actions
        .iter()
        .find_map(|a| match a {
            ConsensusAction::BroadcastBlock(b) => Some(b.clone()),
            _ => None,
        })
        .expect("Node 1 should produce Block 1");

    // Node 0 Validates
    let _ = node0.on_block_response(block_1.clone()).unwrap();

    // Verify State Overlay worked (No commit)
    let bal = node0
        .storage
        .get_account(&receiver_addr)
        .unwrap()
        .map(|a| a.balance)
        .unwrap_or(U256::ZERO);
    assert_eq!(bal, U256::ZERO);

    // Finalize
    let b1_hash = hash_data(&block_1);
    let create_vote = |idx: usize| ockham::types::Vote {
        view: 1,
        block_hash: b1_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: keys[idx].0.clone(),
        signature: ockham::crypto::sign(&keys[idx].1, &b1_hash.0),
    };
    node0.on_vote(create_vote(0)).unwrap();
    node0.on_vote(create_vote(1)).unwrap();

    // Verify Commit
    let bal_after = node0
        .storage
        .get_account(&receiver_addr)
        .unwrap()
        .map(|a| a.balance)
        .unwrap_or(U256::ZERO);
    assert_eq!(bal_after, U256::from(21000001000u64));
}
