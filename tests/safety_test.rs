use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{Hash, PrivateKey, PublicKey, hash_data};
use ockham::storage::Storage;
use ockham::types::{Block, QuorumCertificate, U256};
use std::sync::Arc;

#[test]
fn test_prevent_double_voting() {
    // 1. Setup Node (Validator)
    let keys: Vec<(PublicKey, PrivateKey)> = (0..1)
        .map(|i| ockham::crypto::generate_keypair_from_id(i as u64))
        .collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    let storage = Arc::new(ockham::storage::MemStorage::new());
    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = Arc::new(std::sync::Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(
        state_manager.clone(),
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    let mut validator = SimplexState::new(
        keys[0].0.clone(),
        keys[0].1.clone(),
        committee.clone(),
        storage.clone(),
        tx_pool,
        executor,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    // 2. Create Two Different Blocks for View 2
    let view = 2;
    let genesis_hash = validator.preferred_block;
    let qc_genesis = QuorumCertificate {
        view: 0,
        block_hash: genesis_hash,
        signature: ockham::crypto::sign(&keys[0].1, &Hash::default().0), // Mock valid sig structure (hash doesn't matter for genesis qc check)
        signers: vec![],
    };

    let comm_hash = hash_data(&committee);

    let block_a = Block::new(
        keys[0].0.clone(),
        view,
        genesis_hash,
        qc_genesis.clone(),
        ockham::crypto::Hash::default(),
        ockham::crypto::Hash::default(),
        vec![], // Empty payload
        U256::from(10_000_000),
        0,
        vec![],
        comm_hash,
    );

    // Block B (Different Payload/Hash)
    let mut block_b = block_a.clone();
    block_b.gas_used = 123; // Change something to change hash

    // 3. Receive Proposal A
    let actions_a = validator.on_proposal(block_a.clone()).unwrap();
    let vote_a = actions_a
        .iter()
        .find(|a| matches!(a, ConsensusAction::BroadcastVote(_)));
    assert!(vote_a.is_some(), "Should vote for valid first proposal");

    // Check we persisted this vote
    let state = storage.get_consensus_state().unwrap().unwrap();
    assert_eq!(state.last_voted_view, view);

    // 4. Receive Proposal B (Equivocation Attempt)
    let actions_b = validator.on_proposal(block_b).unwrap();
    let vote_b = actions_b
        .iter()
        .find(|a| matches!(a, ConsensusAction::BroadcastVote(_)));

    // ASSERTION: Should NOT vote for B
    assert!(
        vote_b.is_none(),
        "Should NOT vote for second proposal in same view"
    );

    // Check state didn't change (last_voted_view is still 2)
    let state_after = storage.get_consensus_state().unwrap().unwrap();
    assert_eq!(state_after.last_voted_view, view);
}
