use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{PrivateKey, PublicKey, hash_data, sign};
use ockham::types::{Block, QuorumCertificate, Vote};

#[test]
fn test_timeout_chain_extension() {
    // 1. Setup Committee (1 node for simplicity of leadership)
    let keys: Vec<(PublicKey, PrivateKey)> =
        (0..1).map(|_| ockham::crypto::generate_keypair()).collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    // Node 0
    let storage = std::sync::Arc::new(ockham::storage::MemStorage::new());
    let tx_pool = std::sync::Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = std::sync::Arc::new(std::sync::Mutex::new(
        ockham::state::StateManager::new(storage.clone(), None),
    ));
    let executor = ockham::vm::Executor::new(
        state_manager.clone(),
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    let mut node0 = SimplexState::new(
        keys[0].0.clone(),
        keys[0].1.clone(),
        committee.clone(),
        storage,
        tx_pool,
        executor,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );
    let genesis_block_hash = node0.preferred_block;

    // --- VIEW 1 (Normal) ---
    // Create Block 1
    let qc0 = QuorumCertificate::default();
    let b1 = Block::new(
        keys[0].0.clone(),
        1,
        genesis_block_hash,
        qc0,
        ockham::crypto::Hash::default(),
        ockham::crypto::Hash::default(),
        vec![],
        ockham::types::U256::ZERO,
        0,
        vec![],
        hash_data(&committee),
    );
    let b1_hash = hash_data(&b1);

    // Node 0 processes B1
    node0.on_proposal(b1.clone()).unwrap();

    // Vote V1 (QC1)
    let v1 = Vote {
        view: 1,
        block_hash: b1_hash,
        vote_type: ockham::types::VoteType::Notarize,
        author: keys[0].0.clone(),
        signature: sign(&keys[0].1, &b1_hash.0),
    };
    node0.on_vote(v1).unwrap();

    // Check Preferred Block is B1
    assert_eq!(
        node0.preferred_block, b1_hash,
        "Preferred block should be B1"
    );

    // --- VIEW 2 (Timeout) ---
    // Vote V2 (Dummy)
    let dummy_hash = ockham::crypto::Hash::default();
    let v2 = Vote {
        view: 2,
        block_hash: dummy_hash,
        vote_type: ockham::types::VoteType::Notarize,
        author: keys[0].0.clone(),
        signature: sign(&keys[0].1, &dummy_hash.0),
    };
    node0.on_vote(v2).unwrap();

    // Check QC2 is formed (Dummy)
    let qc2 = node0.storage.get_qc(2).unwrap().expect("QC2 should exist");
    assert_eq!(qc2.block_hash, ockham::crypto::Hash::default());

    // Check Preferred Block is STILL B1 (Not Dummy)
    assert_eq!(
        node0.preferred_block, b1_hash,
        "Preferred block should NOT change to Dummy"
    );

    // --- VIEW 3 (Proposal) ---
    // Node 0 prepares to propose for View 3.
    // It sees QC(2) (Dummy).
    node0.current_view = 3;
    let actions = node0.try_propose().unwrap();

    if let ConsensusAction::BroadcastBlock(b3) = &actions[0] {
        println!("Block 3 Parent: {:?}", b3.parent_hash);
        // CRITICAL CHECK: B3 must extend B1, not ZeroHash
        assert_eq!(
            b3.parent_hash, b1_hash,
            "Block 3 must extend Block 1, NOT Dummy"
        );
        assert_ne!(b3.parent_hash, dummy_hash);
    } else {
        panic!("Expected BroadcastBlock");
    }
}
