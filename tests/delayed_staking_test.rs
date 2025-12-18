use ockham::consensus::SimplexState;
use ockham::crypto::{Hash, generate_keypair_from_id, hash_data, sign};
use ockham::storage::{MemStorage, Storage};
use ockham::types::{Address, Block, QuorumCertificate, Transaction, U256};
use revm::Database;
use std::sync::Arc;

#[test]
fn test_delayed_staking_lifecycle() {
    // 1. Setup Alice (Committee)
    let (alice_pk, alice_sk) = generate_keypair_from_id(0);
    let (bob_pk, bob_sk) = generate_keypair_from_id(1);

    let committee = vec![alice_pk.clone()];
    let storage = Arc::new(MemStorage::new());
    let tx_pool = Arc::new(ockham::tx_pool::TxPool::new(storage.clone()));
    let state_manager = Arc::new(std::sync::Mutex::new(ockham::state::StateManager::new(
        storage.clone(),
        None,
    )));
    let executor = ockham::vm::Executor::new(
        state_manager.clone(),
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    let mut alice = SimplexState::new(
        alice_pk.clone(),
        alice_sk.clone(),
        committee.clone(),
        storage.clone(),
        tx_pool.clone(),
        executor,
        ockham::types::DEFAULT_BLOCK_GAS_LIMIT,
    );

    let bob_addr =
        ockham::types::Address::from_slice(&ockham::types::keccak256(bob_pk.0.to_bytes())[12..]);

    // -------------------------------------------------------------
    // STAGE 0: FUND BOB (Block 1)
    // -------------------------------------------------------------
    println!("--- Funding Bob ---");
    let tx_fund = Transaction {
        chain_id: 1,
        nonce: 0,
        max_priority_fee_per_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        gas_limit: 21000,
        to: Some(bob_addr),
        value: U256::from(5000u64),
        data: vec![].into(),
        access_list: vec![],
        public_key: alice_pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    let mut tx_fund_signed = tx_fund.clone();
    tx_fund_signed.signature = sign(&alice_sk, &tx_fund.sighash().0);

    // Helper to calculate roots
    let prepare_block = |blk: &mut Block, store: Arc<MemStorage>| {
        let overlay = Arc::new(ockham::storage::StateOverlay::new(store));
        // Use Snapshot from main state manager to match Validator's state
        let tree = state_manager.lock().unwrap().snapshot();
        let sm = Arc::new(std::sync::Mutex::new(
            ockham::state::StateManager::new_from_tree(overlay, tree),
        ));
        let exec = ockham::vm::Executor::new(sm.clone(), ockham::types::DEFAULT_BLOCK_GAS_LIMIT);
        exec.execute_block(blk).unwrap();
    };

    let genesis_hash = alice.preferred_block;
    let mut b1 = Block::new(
        alice_pk.clone(),
        1,
        genesis_hash,
        QuorumCertificate::default(),
        Hash::default(),
        Hash::default(),
        vec![tx_fund_signed],
        U256::ZERO,
        0,
        vec![],
        hash_data(&committee),
    );

    // Calculate Roots
    prepare_block(&mut b1, storage.clone());
    let b1_hash = hash_data(&b1);

    alice.on_proposal(b1.clone()).unwrap();
    let vote_fin_1 = ockham::types::Vote {
        view: 1,
        block_hash: b1_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: alice_pk.clone(),
        signature: sign(&alice_sk, &b1_hash.0),
    };
    alice.on_vote(vote_fin_1).unwrap();

    // Verify Bob Funds
    {
        let mut db = state_manager.lock().unwrap();
        let acc = db.basic(bob_addr).unwrap().unwrap();
        assert_eq!(acc.balance, U256::from(5000u64));
        println!("Bob Funded: {}", acc.balance);
    }

    // -------------------------------------------------------------
    // STAGE 1: STAKE (Block 2)
    // -------------------------------------------------------------
    println!("--- Bob Staking ---");
    let stake_call = hex::decode("3a4b66f1").unwrap();
    let tx_stake = Transaction {
        chain_id: 1,
        nonce: 0,
        max_priority_fee_per_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        gas_limit: 100_000,
        to: Some(Address::from_slice(
            &hex::decode("0000000000000000000000000000000000001000").unwrap(),
        )),
        value: U256::from(2000u64),
        data: stake_call.into(),
        access_list: vec![],
        public_key: bob_pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    let mut tx_stake_signed = tx_stake.clone();
    tx_stake_signed.signature = sign(&bob_sk, &tx_stake.sighash().0);

    let sig1 = sign(&alice_sk, &b1_hash.0);
    let qc1 = QuorumCertificate {
        view: 1,
        block_hash: b1_hash,
        signature: sig1,
        signers: vec![alice_pk.clone()],
    };

    let mut b2 = Block::new(
        alice_pk.clone(),
        2,
        b1_hash,
        qc1.clone(),
        Hash::default(),
        Hash::default(),
        vec![tx_stake_signed],
        U256::ZERO,
        0,
        vec![],
        hash_data(&committee),
    );
    prepare_block(&mut b2, storage.clone());
    let b2_hash = hash_data(&b2);

    alice.on_proposal(b2.clone()).unwrap();
    let vote_fin_2 = ockham::types::Vote {
        view: 2,
        block_hash: b2_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: alice_pk.clone(),
        signature: sign(&alice_sk, &b2_hash.0),
    };
    alice.on_vote(vote_fin_2).unwrap();

    // Check Staking Success
    {
        let mut db = state_manager.lock().unwrap();
        let acc = db.basic(bob_addr).unwrap().unwrap();
        println!("Bob Balance after Stake: {}", acc.balance);
        assert_eq!(acc.balance, U256::from(3000u64));
    }
    {
        let state = storage.get_consensus_state().unwrap().unwrap();
        assert_eq!(state.pending_validators.len(), 1);
        assert_eq!(state.pending_validators[0].0, bob_pk);
        // Activation View = Current View (2) + 10 = 12
        assert_eq!(state.pending_validators[0].1, 12);
        println!("Bob Pending until view 12");

        let stake = state.stakes.get(&bob_addr).cloned().unwrap_or_default();
        println!("DEBUG: Bob Stake in Storage: {}", stake);
        assert_eq!(stake, U256::from(2000u64));
    }

    // -------------------------------------------------------------
    // STAGE 2: ACTIVATE (Block 12)
    // -------------------------------------------------------------
    // Propose B12 extending B2.
    let sig2 = sign(&alice_sk, &b2_hash.0);
    let qc2 = QuorumCertificate {
        view: 2,
        block_hash: b2_hash,
        signature: sig2,
        signers: vec![alice_pk.clone()],
    };

    let mut b12 = Block::new(
        alice_pk.clone(),
        12,
        b2_hash,
        qc2,
        Hash::default(),
        Hash::default(),
        vec![],
        U256::ZERO,
        0,
        vec![],
        hash_data(&committee),
    );
    prepare_block(&mut b12, storage.clone());
    let b12_hash = hash_data(&b12);

    alice.on_proposal(b12.clone()).unwrap();
    let vote_fin_12 = ockham::types::Vote {
        view: 12,
        block_hash: b12_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: alice_pk.clone(),
        signature: sign(&alice_sk, &b12_hash.0),
    };
    alice.on_vote(vote_fin_12).unwrap();

    {
        let state = storage.get_consensus_state().unwrap().unwrap();
        assert!(state.committee.contains(&bob_pk));
        assert!(state.pending_validators.is_empty());
        println!("Bob Active");

        let stake = state.stakes.get(&bob_addr).cloned().unwrap_or_default();
        println!("DEBUG: Bob Stake after Activation: {}", stake);
        assert_eq!(stake, U256::from(2000u64));
    }

    // -------------------------------------------------------------
    // STAGE 3: UNSTAKE (Block 13)
    // -------------------------------------------------------------
    println!("--- Bob Unstaking ---");
    let unstake_call = hex::decode("2e17de78").unwrap();
    let mut tx_unstake = Transaction {
        chain_id: 1,
        nonce: 1,
        max_priority_fee_per_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        gas_limit: 100_000,
        to: Some(Address::from_slice(
            &hex::decode("0000000000000000000000000000000000001000").unwrap(),
        )),
        value: U256::ZERO,
        data: unstake_call.into(),
        access_list: vec![],
        public_key: bob_pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    tx_unstake.signature = sign(&bob_sk, &tx_unstake.sighash().0);

    // New committee for validation?
    // Wait, B13 must be signed by committee. Committee is now [Alice, Bob].
    let mut new_committee = committee.clone();
    new_committee.push(bob_pk.clone());
    // Note: order matters for hash.
    // In vm logic: `state.committee.push(pk)`. So appended at end.
    // So [Alice, Bob].

    // Agg QC for B12 (Alice+Bob? No, B12 was signed by Alice alone because Bob wasn't active at V2 QC creation)
    // Wait, B12 executed and activated Bob.
    // B13 qc certifies B12.
    // At B12 finalization, Bob became active.
    // So QC for B12 MUST include Bob?
    // No, QC signs the *Previous* state authority?
    // View 12 QC signs B12.
    // B12 was proposed by Alice.
    // Validators for View 12 are defined by parent (B2) state? Or B12 state?
    // Usually validator set for View V is active set at start of V.
    // B12 start: Bob active? No, Bob activated *during* B12 execution.
    // So QC for B12 (created AFTER execution) should arguably be signed by new committee?
    // Or old committee?
    // HotStuff/Simplex: QC verifies block B. Signers are committee at B.view.
    // At B.view (12), Bob was pending (activated at end).
    // So Alice is the only signer for B12 QC.
    // B13 extends B12.
    // B13 (View 13).
    // QC for B12 needs Alice signature.

    let sig12 = sign(&alice_sk, &b12_hash.0);
    // Sig12 needs to be Aggregate format if using verify_aggregate?
    // Alice is 1/1 (Bob not active yet in QC view).
    let qc12 = QuorumCertificate {
        view: 12,
        block_hash: b12_hash,
        signature: sig12,
        signers: vec![alice_pk.clone()],
    };

    // But B13 Block Committee Hash?
    // Use new committee [Alice, Bob].
    let mut b13 = Block::new(
        alice_pk.clone(),
        13,
        b12_hash,
        qc12,
        Hash::default(),
        Hash::default(),
        vec![tx_unstake],
        U256::ZERO,
        0,
        vec![],
        hash_data(&new_committee),
    );
    prepare_block(&mut b13, storage.clone());
    let b13_hash = hash_data(&b13);

    alice.on_proposal(b13.clone()).unwrap();
    // Finalize B13. Now Bob IS active. So voting needs Bob?
    // alice.on_vote triggers check based on *Loaded* committee.
    // After B12 finalization, committee updated to [Alice, Bob].
    // So B13 finalization needs 2/3 of 2 = 2 votes.
    // Alice + Bob.

    let vote_fin_13_a = ockham::types::Vote {
        view: 13,
        block_hash: b13_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: alice_pk.clone(),
        signature: sign(&alice_sk, &b13_hash.0),
    };
    alice.on_vote(vote_fin_13_a).unwrap();

    let vote_fin_13_b = ockham::types::Vote {
        view: 13,
        block_hash: b13_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: bob_pk.clone(),
        signature: sign(&bob_sk, &b13_hash.0),
    };
    alice.on_vote(vote_fin_13_b).unwrap(); // Should trigger finalize

    {
        let state = storage.get_consensus_state().unwrap().unwrap();
        assert_eq!(state.exiting_validators.len(), 1);
        // Exiting view = 13 + 10 = 23.
        assert_eq!(state.exiting_validators[0].1, 23);
        println!("Bob Exiting until 23");

        let stake = state.stakes.get(&bob_addr).cloned().unwrap_or_default();
        println!("DEBUG: Bob Stake after Unstake: {}", stake);
        assert_eq!(stake, U256::from(2000u64));
    }

    // -------------------------------------------------------------
    // STAGE 4: REMOVE & WITHDRAW (Block 23)
    // -------------------------------------------------------------

    // Propose B23 extending B13.
    // QC for B13 needs Alice+Bob.
    let s13_a = sign(&alice_sk, &b13_hash.0);
    let s13_b = sign(&bob_sk, &b13_hash.0);
    let agg13 = ockham::crypto::aggregate(&[s13_a, s13_b]).unwrap();
    let qc13 = QuorumCertificate {
        view: 13,
        block_hash: b13_hash,
        signature: agg13,
        signers: vec![alice_pk.clone(), bob_pk.clone()],
    }; // Order? Sorted usually.

    let mut b23 = Block::new(
        alice_pk.clone(),
        23,
        b13_hash,
        qc13,
        Hash::default(),
        Hash::default(),
        vec![],
        U256::ZERO,
        0,
        vec![],
        hash_data(&new_committee),
    );
    prepare_block(&mut b23, storage.clone());
    let b23_hash = hash_data(&b23);

    alice.on_proposal(b23.clone()).unwrap();
    // Finalize B23. Needs Alice+Bob.
    let v23a = ockham::types::Vote {
        view: 23,
        block_hash: b23_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: alice_pk.clone(),
        signature: sign(&alice_sk, &b23_hash.0),
    };
    alice.on_vote(v23a).unwrap();
    let v23b = ockham::types::Vote {
        view: 23,
        block_hash: b23_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: bob_pk.clone(),
        signature: sign(&bob_sk, &b23_hash.0),
    };
    alice.on_vote(v23b).unwrap();

    {
        let state = storage.get_consensus_state().unwrap().unwrap();
        assert!(!state.committee.contains(&bob_pk));
        println!("Bob Removed");

        // Stake should still be there
        let stake = state.stakes.get(&bob_addr).cloned().unwrap_or_default();
        println!("DEBUG: Bob Stake after Removal: {}", stake);
        assert_eq!(stake, U256::from(2000u64));
    }

    // Withdraw (Block 24)
    println!("--- Bob Withdrawing ---");
    let withdraw_call = hex::decode("3ccfd60b").unwrap();
    let mut tx_withdraw = Transaction {
        chain_id: 1,
        nonce: 2,
        max_priority_fee_per_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        gas_limit: 100_000,
        to: Some(Address::from_slice(
            &hex::decode("0000000000000000000000000000000000001000").unwrap(),
        )),
        value: U256::ZERO,
        data: withdraw_call.into(),
        access_list: vec![],
        public_key: bob_pk.clone(),
        signature: ockham::crypto::Signature::default(),
    };
    tx_withdraw.signature = sign(&bob_sk, &tx_withdraw.sighash().0);

    // B24. Committee is just Alice again.
    // QC for B23 (Alice+Bob).
    let s23a = sign(&alice_sk, &b23_hash.0);
    let s23b = sign(&bob_sk, &b23_hash.0);
    let agg23 = ockham::crypto::aggregate(&[s23a, s23b]).unwrap();
    let qc23 = QuorumCertificate {
        view: 23,
        block_hash: b23_hash,
        signature: agg23,
        signers: vec![alice_pk.clone(), bob_pk.clone()],
    };

    let mut b24 = Block::new(
        alice_pk.clone(),
        24,
        b23_hash,
        qc23,
        Hash::default(),
        Hash::default(),
        vec![tx_withdraw],
        U256::ZERO,
        0,
        vec![],
        hash_data(&committee),
    );
    prepare_block(&mut b24, storage.clone());
    let b24_hash = hash_data(&b24);

    alice.on_proposal(b24.clone()).unwrap();
    // Finalize B24. Just Alice needed (committee shrank).
    let v24 = ockham::types::Vote {
        view: 24,
        block_hash: b24_hash,
        vote_type: ockham::types::VoteType::Finalize,
        author: alice_pk.clone(),
        signature: sign(&alice_sk, &b24_hash.0),
    };
    alice.on_vote(v24).unwrap();

    {
        let mut db = state_manager.lock().unwrap();
        let acc = db.basic(bob_addr).unwrap().unwrap();
        assert_eq!(acc.balance, U256::from(5000u64));
        println!("Bob Withdrew Successfully. Balance: {}", acc.balance);
    }
}
