use ockham::crypto::{Hash, PrivateKey, PublicKey};
use ockham::storage::Storage;
use ockham::types::{Block, QuorumCertificate, U256};
use std::sync::Arc;
use std::sync::Mutex;

#[test]
fn test_liveness_slashing() {
    // 1. Setup Committee (4 Nodes)
    let keys: Vec<(PublicKey, PrivateKey)> = (0..4)
        .map(|i| ockham::crypto::generate_keypair_from_id(i as u64))
        .collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    // Node 0 is our reference, but we are testing the VM logic which is shared
    let _my_id = keys[0].0.clone();
    let _my_key = keys[0].1.clone();

    // Target Victim: Node 1 (Offender)
    let victim_idx = 1;
    let victim_id = keys[victim_idx].0.clone();

    let storage = Arc::new(ockham::storage::MemStorage::new());
    let _tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));

    // Initialize Victim Balance (1000 units)
    let victim_pk_bytes = victim_id.0.to_bytes();
    let victim_hash = ockham::types::keccak256(victim_pk_bytes);
    let victim_addr = ockham::types::Address::from_slice(&victim_hash[12..]);

    let initial_balance = U256::from(1000u64);
    let account = ockham::storage::AccountInfo {
        nonce: 0,
        balance: initial_balance,
        code_hash: Hash(ockham::types::keccak256([]).into()),
        code: None,
    };
    storage.save_account(&victim_addr, &account).unwrap();

    let state_manager = Arc::new(Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(
        state_manager.clone(),
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    // Initialize State
    let initial_state = ockham::storage::ConsensusState {
        view: 1,
        finalized_height: 0,
        preferred_block: Hash::default(),
        preferred_view: 0,
        last_voted_view: 0,
        committee: committee.clone(),
        pending_validators: vec![],
        exiting_validators: vec![],
        stakes: {
            let mut m = std::collections::HashMap::new();
            m.insert(victim_addr, U256::from(1000u64));
            m
        },
        inactivity_scores: std::collections::HashMap::new(),
    };
    storage.save_consensus_state(&initial_state).unwrap();

    // 2. Simulate Timeout of View 1 (Leader: Node 1)
    // View 1 -> 1 % 4 = 1. So Node 1 is Leader of View 1.
    // We create a Block in View 2 (Leader: Node 2) that justifies View 1 with a Timeout QC.

    let timeout_view = 1;
    let timeout_qc = QuorumCertificate {
        view: timeout_view,
        block_hash: Hash::default(), // ZeroHash = Timeout
        signature: ockham::crypto::Signature::default(),
        signers: vec![],
    };

    // Block Author: Node 2
    let author_idx = 2;
    let block = Block::new(
        keys[author_idx].0.clone(),
        2,               // View
        Hash::default(), // Parent
        timeout_qc,
        Hash::default(),
        Hash::default(),
        vec![],
        U256::ZERO,
        0,
        vec![],
        Hash::default(),
    );

    // 3. Execute Block
    let mut block_to_exec = block.clone();
    executor.execute_block(&mut block_to_exec).unwrap();

    // 4. Verify Slashing
    {
        let db = state_manager.lock().unwrap();
        // Check Stake
        let state = db.get_consensus_state().unwrap().unwrap();
        let stake = state.stakes.get(&victim_addr).expect("Stake should exist");
        assert_eq!(*stake, U256::from(990u64), "Stake should be slashed by 10");

        // Check Score
        let score = state
            .inactivity_scores
            .get(&victim_id)
            .expect("Score should exist");
        assert_eq!(*score, 1, "Score should be 1");
    }

    // 5. Reward Check (Node 2 should decrement, but it's 0 so stays 0)
    // Let's set Node 2 score to 5 first.
    {
        let db = state_manager.lock().unwrap();
        let mut state = db.get_consensus_state().unwrap().unwrap();
        state
            .inactivity_scores
            .insert(keys[author_idx].0.clone(), 5);
        db.save_consensus_state(&state).unwrap();
    }

    // Execute again (same block reuse is fine for logic testing)
    executor.execute_block(&mut block_to_exec).unwrap();

    {
        let db = state_manager.lock().unwrap();
        let state = db.get_consensus_state().unwrap().unwrap();
        let score = state.inactivity_scores.get(&keys[author_idx].0).unwrap();
        assert_eq!(*score, 4, "Author score should decrement");

        let victim_score = state.inactivity_scores.get(&victim_id).unwrap();
        assert_eq!(*victim_score, 2, "Victim score should increment again");

        let stake = state.stakes.get(&victim_addr).unwrap();
        assert_eq!(*stake, U256::from(980u64), "Stake slashed again");
    }

    // 6. Threshold Removal
    // Repeat until score > 50
    // Current score 2. Need 49 more loops.
    for _ in 0..50 {
        executor.execute_block(&mut block_to_exec).unwrap();
    }

    {
        let db = state_manager.lock().unwrap();
        let state = db.get_consensus_state().unwrap().unwrap();

        // Check if removed from committee
        assert!(
            !state.committee.contains(&victim_id),
            "Victim should be removed from committee"
        );

        // Check if score reset
        assert!(
            state.inactivity_scores.get(&victim_id).is_none(),
            "Score should be clear"
        );
    }

    println!("Liveness Slashing Test Passed!");
}
