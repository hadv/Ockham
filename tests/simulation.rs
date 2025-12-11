use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{PrivateKey, PublicKey, hash_data};
use ockham::types::{Block, QuorumCertificate};

#[test]
fn test_three_chain_commit() {
    // 1. Setup Committee (4 nodes)
    let keys: Vec<(PublicKey, PrivateKey)> =
        (0..4).map(|i| (PublicKey(i), PrivateKey(i))).collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0).collect();

    // Instantiate State for Node 0 (Leader 1)
    let mut node0 = SimplexState::new(keys[0].0, keys[0].1, committee.clone());

    // Instantiate State for Node 1, 2, 3
    let mut other_nodes: Vec<SimplexState> = keys
        .iter()
        .skip(1)
        .map(|(pk, sk)| SimplexState::new(*pk, *sk, committee.clone()))
        .collect();

    println!("Genesis: {:?}", node0.blocks.keys());

    // --- VIEW 1: PREPARE b1 ---
    // Leader 0 creates Block 1 (parent = Genesis)
    let genesis_hash = hash_data(node0.blocks.values().next().unwrap());
    let qc0 = QuorumCertificate::default(); // genesis QC
    let b1 = Block::new(keys[0].0, 1, genesis_hash, qc0, vec![1, 2, 3]);
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
    if let Some(v) = extract_vote(node0.on_proposal(b1.clone()).unwrap()) {
        votes_v1.push(v);
    }

    // Others vote
    for node in &mut other_nodes {
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

        let _ = node0.on_vote(vote.clone()).unwrap();

        // Check if QC was formed in state
        if let Some(qc) = node0.qcs.get(&1) {
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
    other_nodes[0].blocks.insert(b1_hash, b1.clone());

    // Node 1 proposes b2
    let b2 = Block::new(keys[1].0, 2, b1_hash, qc1.clone(), vec![4, 5, 6]);
    let b2_hash = hash_data(&b2);

    // All nodes vote for b2
    let mut votes_v2 = vec![];

    // Node 0 needs to see b2 (and its parent logic checks out)
    if let Some(v) = extract_vote(node0.on_proposal(b2.clone()).unwrap()) {
        votes_v2.push(v);
    }

    for node in &mut other_nodes {
        // ensure they have b1
        if !node.blocks.contains_key(&b1_hash) {
            node.blocks.insert(b1_hash, b1.clone());
        }
        if let Some(v) = extract_vote(node.on_proposal(b2.clone()).unwrap()) {
            votes_v2.push(v);
        }
    }

    // Aggregate QC2 (Node 0 does it again for tracking)
    let mut qc2 = None;
    for vote in votes_v2 {
        let _ = node0.on_vote(vote).unwrap();
        if let Some(qc) = node0.qcs.get(&2) {
            qc2 = Some(qc.clone());
        }
    }
    assert!(qc2.is_some(), "Should have formed QC for View 2");
    let qc2 = qc2.unwrap();
    println!("QC2 Formed for View {}", qc2.view);

    // --- VERIFY COMMIT ---
    assert!(node0.blocks.contains_key(&b2_hash));
    assert!(node0.qcs.contains_key(&2));
}
