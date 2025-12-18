use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{Hash, PrivateKey, PublicKey};
use ockham::storage::Storage;
use ockham::types::{Block, QuorumCertificate, U256, Vote, VoteType};
use revm::Database;
use std::sync::Arc;
use std::sync::Mutex;

#[test]
fn test_slashing_flow() {
    // 1. Setup Node (Validator 0)
    let keys: Vec<(PublicKey, PrivateKey)> = (0..4)
        .map(|i| ockham::crypto::generate_keypair_from_id(i as u64))
        .collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    // We act as Validator 0. Validator 1 will equivocate.
    let my_id = keys[0].0.clone();
    let my_key = keys[0].1.clone();

    let offender_idx = 1;
    let offender_id = keys[offender_idx].0.clone();
    let offender_key = keys[offender_idx].1.clone();

    let storage = Arc::new(ockham::storage::MemStorage::new());
    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));

    // Initial State: Give offender some balance to slash
    let offender_pk_bytes = offender_id.0.to_bytes();
    let offender_hash = ockham::types::keccak256(offender_pk_bytes);
    let offender_addr = ockham::types::Address::from_slice(&offender_hash[12..]);

    let initial_balance = U256::from(5000u64);
    let account = ockham::storage::AccountInfo {
        nonce: 0,
        balance: initial_balance,
        code_hash: Hash(ockham::types::keccak256([]).into()),
        code: None,
    };
    storage.save_account(&offender_addr, &account).unwrap();

    let state_manager = Arc::new(Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(
        state_manager.clone(),
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    let mut validator = SimplexState::new(
        my_id.clone(),
        my_key,
        committee.clone(),
        storage.clone(),
        tx_pool,
        executor.clone(),
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    // 2. Create Equivocation Votes (View 2)
    let view = 2;
    let block_a_hash = Hash([1u8; 32]);
    let block_b_hash = Hash([2u8; 32]);

    let vote_a = Vote {
        view,
        block_hash: block_a_hash,
        vote_type: VoteType::Notarize,
        author: offender_id.clone(),
        signature: ockham::crypto::sign(&offender_key, &block_a_hash.0),
    };

    let vote_b = Vote {
        view,
        block_hash: block_b_hash,
        vote_type: VoteType::Notarize,
        author: offender_id.clone(),
        signature: ockham::crypto::sign(&offender_key, &block_b_hash.0),
    };

    // 3. Receive Vote A
    let _ = validator.on_vote(vote_a.clone()).unwrap();

    // 4. Receive Vote B (Should trigger detection)
    let actions = validator.on_vote(vote_b.clone()).unwrap();

    // Check for BroadcastEvidence
    let evidence_action = actions
        .iter()
        .find(|a| matches!(a, ConsensusAction::BroadcastEvidence(_)));
    assert!(
        evidence_action.is_some(),
        "Should broadcast evidence on detection"
    );

    let evidence = match evidence_action.unwrap() {
        ConsensusAction::BroadcastEvidence(e) => e.clone(),
        _ => panic!("Wrong action type"),
    };

    assert_eq!(evidence.vote_a, vote_a);
    assert_eq!(evidence.vote_b, vote_b);

    // Check Pool
    assert!(
        !validator.evidence_pool.is_empty(),
        "Evidence should be in pool"
    );

    // 5. Propose Block (As leader of View 3, for example)
    // We need to set validator as leader for View 3?
    // keys[3] is leader for view 3 (3 % 4 = 3).
    // Let's use View 4 where 4 % 4 = 0 (us).
    validator.current_view = 4;

    // Need a QC for View 3 (or whatever parent).
    // Let's just mock creating a proposal directly using the internal helper or `try_propose` if we set up QC.
    // Easier: Mock QC for View 3.
    let qc_mock = QuorumCertificate {
        view: 3,
        block_hash: Hash::default(), // Dummy parent
        signature: ockham::crypto::Signature::default(),
        signers: vec![],
    };
    storage.save_qc(&qc_mock).unwrap();
    storage
        .save_block(&Block::new_dummy(
            keys[3].0.clone(),
            3,
            Hash::default(),
            QuorumCertificate::default(),
        ))
        .unwrap();

    let actions = validator.try_propose().unwrap();
    let block_action = actions
        .iter()
        .find(|a| matches!(a, ConsensusAction::BroadcastBlock(_)));
    assert!(block_action.is_some(), "Should propose block");

    let block = match block_action.unwrap() {
        ConsensusAction::BroadcastBlock(b) => b.clone(),
        _ => panic!(),
    };

    // 6. Check Inclusion
    assert!(!block.evidence.is_empty(), "Block should contain evidence");
    assert_eq!(block.evidence[0], evidence, "Included evidence matches");

    // 7. Check Slashing Execution
    // `try_propose` calls `executor.execute_block`.
    // So the state should already be updated in the ephemeral state... wait.
    // `try_propose` executes on `StateOverlay`. It does NOT commit to main DB (unless saved block triggers commit, but here it saves Block Data, not Account State).
    // The account state change (balance reduction) happens when `execute_block` is called.
    // In `try_propose`, it calls `executor.execute_block(&mut block)`.
    // The `executor` uses a fork.

    // Let's verify the fork logic or manually execute again.
    let mut block_to_exec = block.clone();

    // Use the main executor (which points to real storage state manager? No it creates a fork in try_propose).
    // Let's inspect the state AFTER the block is committed/finalized.
    // Or we can manually run `executor.execute_block` against the raw storage to simulate finalization.

    let state_manager = validator.executor.state.clone(); // This is the real state manager
    let executor = ockham::vm::Executor::new(state_manager, 10_000_000);

    executor.execute_block(&mut block_to_exec).unwrap();

    // Check balance
    let mut db = validator.executor.state.lock().unwrap();
    let account = db.basic(offender_addr).unwrap().unwrap();

    // Slashed amount is 1000. Initial 5000. Should be 4000.
    assert_eq!(
        account.balance,
        U256::from(4000u64),
        "Balance should be slashed"
    );

    println!("Slashing Test Passed!");
}
