use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{PrivateKey, PublicKey, hash_data};
use ockham::types::{Block, QuorumCertificate};

#[test]
fn test_three_chain_commit() {
    // 1. Setup Committee (4 nodes)
    let keys: Vec<(PublicKey, PrivateKey)> =
        (0..4).map(|_| ockham::crypto::generate_keypair()).collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    // Shared tx pool and executor not really needed for this simulation unless we execute.
    // We need to provide dummy ones.

    // Instantiate State for Node 0 (Leader 1)
    let mut nodes: Vec<SimplexState> = (0..4)
        .map(|i| {
            let storage = std::sync::Arc::new(ockham::storage::MemStorage::new());
            // Create individual tx pool and executor for each node
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
        })
        .collect();

    println!("Genesis: {:?}", nodes[0].preferred_block);

    // --- VIEW 1: PREPARE b1 ---
    // Leader 0 creates Block 1 (parent = Genesis)
    let genesis_hash = nodes[0].preferred_block;
    let qc0 = QuorumCertificate::default(); // genesis QC
    let b1 = Block::new(
        keys[0].0.clone(),
        1,
        genesis_hash,
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

    println!("Block 1 Hash: {:?}", b1_hash);

    // All nodes process b1
    let mut votes_v1 = vec![];

    // Helper to extract vote from actions
    let extract_vote = |actions: Vec<ConsensusAction>| -> Option<ockham::types::Vote> {
        for action in actions {
            if let ConsensusAction::BroadcastVote(v) = action {
                return Some(v);
            }
        }
        None
    };

    // Node 0 votes
    if let Some(v) = extract_vote(nodes[0].on_proposal(b1.clone()).unwrap()) {
        votes_v1.push(v);
    }

    // Others vote
    for node in nodes.iter_mut().skip(1) {
        if let Some(v) = extract_vote(node.on_proposal(b1.clone()).unwrap()) {
            votes_v1.push(v);
        }
    }

    // Checking votes
    assert_eq!(votes_v1.len(), 4);

    // Aggregate votes for View 1 (QC1)
    let mut qc1 = None;
    for vote in votes_v1 {
        // Feed vote back to Node 0.
        // Note: on_vote now returns Vec<Action>, which is empty for now unless we implement auto-proposal.
        // But the QC is formed internally in `node0.qcs`.

        let _ = nodes[0].on_vote(vote.clone()).unwrap();

        // Check if QC was formed in state
        if let Ok(Some(qc)) = nodes[0].storage.get_qc(1) {
            qc1 = Some(qc.clone());
            break;
        }
    }

    assert!(qc1.is_some(), "Should have formed QC for View 1");
    let qc1 = qc1.unwrap();
    println!("QC1 Formed for View {}", qc1.view);

    // --- VIEW 2: PREPARE b2 ---
    // Leader 1 (Node 1) proposes Block 2 (parent = b1)
    // First, Node 1 needs to know about b1 and QC1 (sync/gossip)
    // We manually update Node 1 state
    nodes[1].storage.save_block(&b1).unwrap();

    // Node 1 proposes b2
    // Node 1 proposes b2
    let b2 = Block::new(
        keys[1].0.clone(),
        2,
        b1_hash,
        qc1.clone(),
        ockham::crypto::Hash::default(),
        ockham::crypto::Hash::default(),
        vec![],
        ockham::types::U256::ZERO,
        0,
        vec![],
        hash_data(&committee),
    );
    let b2_hash = hash_data(&b2);

    // All nodes vote for b2
    let mut votes_v2 = vec![];

    // Node 0 needs to see b2 (and its parent logic checks out)
    if let Some(v) = extract_vote(nodes[0].on_proposal(b2.clone()).unwrap()) {
        votes_v2.push(v);
    }

    for node in nodes.iter_mut().skip(1) {
        // ensure they have b1
        node.storage.save_block(&b1).unwrap();
        if let Some(v) = extract_vote(node.on_proposal(b2.clone()).unwrap()) {
            votes_v2.push(v);
        }
    }

    // Aggregate QC2 (Node 0 does it again for tracking)
    let mut qc2 = None;
    for vote in votes_v2 {
        let _ = nodes[0].on_vote(vote).unwrap();
        if let Ok(Some(qc)) = nodes[0].storage.get_qc(2) {
            qc2 = Some(qc.clone());
        }
    }
    assert!(qc2.is_some(), "Should have formed QC for View 2");
    let qc2 = qc2.unwrap();
    println!("QC2 Formed for View {}", qc2.view);

    // --- VERIFY COMMIT ---
    assert!(nodes[0].storage.get_block(&b2_hash).unwrap().is_some());
    assert!(nodes[0].storage.get_qc(2).unwrap().is_some());
}
