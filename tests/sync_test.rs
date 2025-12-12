use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{Hash, generate_keypair_from_id, hash_data};
use ockham::storage::MemStorage;
use ockham::types::{Block, QuorumCertificate};

/// Helper to create a signed block
fn create_block(author_id: u64, view: u64, parent_hash: Hash, justify: QuorumCertificate) -> Block {
    let (pk, _) = generate_keypair_from_id(author_id);
    Block::new(pk, view, parent_hash, justify, vec![])
}

#[test]
fn test_sync_orphan_processing() {
    let (alice_pk, _) = generate_keypair_from_id(0);
    let (bob_pk, bob_sk) = generate_keypair_from_id(1);
    let committee = vec![alice_pk.clone(), bob_pk.clone()];

    // Initialize Bob (Sync Node)
    let storage = Box::new(MemStorage::new());
    let mut bob = SimplexState::new(bob_pk, bob_sk, committee, storage);

    // Create a chain of blocks (Geneis -> B1 -> B2 -> B3)
    let genesis_hash = bob.preferred_block;
    let genesis_qc = QuorumCertificate::default(); // Simplified for test

    // Block 1 (View 1)
    let b1 = create_block(0, 1, genesis_hash, genesis_qc.clone());
    let b1_hash = hash_data(&b1);
    // Assume QC for B1 exists (simplification: we just need a valid QC for the next block)
    // For this test, valid QCs are not strictly checked unless we call on_proposal fully.
    // SimplexState::on_proposal calls verify_qc, which checks simple things.
    let qc1 = QuorumCertificate {
        view: 1,
        block_hash: b1_hash,
        signatures: vec![],
    };

    // Block 2 (View 2)
    let b2 = create_block(0, 2, b1_hash, qc1.clone());
    let b2_hash = hash_data(&b2);
    let qc2 = QuorumCertificate {
        view: 2,
        block_hash: b2_hash,
        signatures: vec![],
    };

    // Block 3 (View 3)
    let b3 = create_block(0, 3, b2_hash, qc2.clone());

    // --- SCENARIO: Bob receives B3 first (gap) ---
    println!("Feeding Block 3 to Bob (Orphan)...");
    let result = bob.on_proposal(b3.clone());

    // Should trigger Request(B2)
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ConsensusAction::BroadcastRequest(h) => assert_eq!(*h, b2_hash),
        _ => panic!("Expected BroadcastRequest"),
    }
    // Verify B3 is orphaned
    assert!(bob.orphans.contains_key(&b2_hash)); // B3 is waiting for B2

    // --- SCENARIO: Bob receives B2 (gap) ---
    println!("Feeding Block 2 to Bob (Orphan)...");
    let result = bob.on_proposal(b2.clone());

    // Should trigger Request(B1)
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ConsensusAction::BroadcastRequest(h) => assert_eq!(*h, b1_hash),
        _ => panic!("Expected BroadcastRequest"),
    }
    // Verify B2 is orphaned
    assert!(bob.orphans.contains_key(&b1_hash)); // B2 is waiting for B1

    // --- SCENARIO: Bob receives B1 (Sync Complete) ---
    println!("Feeding Block 1 to Bob (Missing Link)...");
    // We use on_proposal for the first block, then on_block_response logic would handle orphans.
    // However, on_block_response invokes on_proposal.
    // Let's simulate receiving B1 via Sync Response logic logic.
    let result = bob.on_block_response(b1.clone());

    // Should trigger: Orphan processing for B2, then Orphan processing for B3.
    // Total actions should include Votes for B1, B2, B3.
    let actions = result.unwrap();
    println!("Actions generated: {:?}", actions);

    // Verify Bob has persisted all blocks
    assert!(bob.storage.get_block(&b1_hash).unwrap().is_some());
    assert!(bob.storage.get_block(&b2_hash).unwrap().is_some());
    let b3_hash = hash_data(&b3);
    assert!(bob.storage.get_block(&b3_hash).unwrap().is_some());

    // Verify Bob's view advanced
    assert!(bob.current_view >= 3);
}

#[test]
fn test_sync_block_serving() {
    let (alice_pk, alice_sk) = generate_keypair_from_id(0);
    let committee = vec![alice_pk.clone()];

    // Initialize Alice
    let storage = Box::new(MemStorage::new());
    let alice = SimplexState::new(alice_pk.clone(), alice_sk, committee, storage);

    // Create a block and save it
    let genesis_qc = QuorumCertificate::default();
    let b1 = create_block(0, 1, alice.preferred_block, genesis_qc);
    let b1_hash = hash_data(&b1);
    alice.storage.save_block(&b1).unwrap();

    // Request the block
    let peer_id = "PeerB".to_string();
    let result = alice.on_block_request(b1_hash, peer_id.clone());

    // Should return SendBlock action
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ConsensusAction::SendBlock(block, pid) => {
            assert_eq!(block.view, 1);
            assert_eq!(hash_data(block), b1_hash);
            assert_eq!(pid, &peer_id);
        }
        _ => panic!("Expected SendBlock"),
    }
}
