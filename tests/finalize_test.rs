#![allow(clippy::collapsible_if)]
use ockham::consensus::{ConsensusAction, SimplexState};
use ockham::crypto::{PrivateKey, PublicKey, hash_data};
use ockham::types::{Block, QuorumCertificate, VoteType};

#[test]
fn test_explicit_finalization() {
    let _ = env_logger::builder().is_test(true).try_init();

    // 1. Setup Committee (4 nodes) -> f=1, threshold=3
    let keys: Vec<(PublicKey, PrivateKey)> =
        (0..4).map(|_| ockham::crypto::generate_keypair()).collect();
    let committee: Vec<PublicKey> = keys.iter().map(|k| k.0.clone()).collect();

    let mut node0 = SimplexState::new(keys[0].0.clone(), keys[0].1.clone(), committee.clone());

    // 2. Proposal for View 1
    let genesis_hash = hash_data(node0.blocks.values().next().unwrap());
    let qc0 = QuorumCertificate::default();
    let b1 = Block::new(keys[0].0.clone(), 1, genesis_hash, qc0, vec![]);

    // 3. Node 0 receives Block 1 -> Should Vote (Notarize)
    let actions = node0.on_proposal(b1.clone()).unwrap();
    let mut notarize_vote = None;
    for action in actions {
        if let ConsensusAction::BroadcastVote(v) = action {
            if v.vote_type == VoteType::Notarize {
                notarize_vote = Some(v);
                break;
            }
        }
    }
    assert!(
        notarize_vote.is_some(),
        "Node 0 should vote to notarize Block 1"
    );

    // 4. Simulate Aggregation: All 4 nodes vote Notarize for Block 1
    // We feed these votes into Node 0 to form a QC
    let b1_hash = hash_data(&b1);

    // Create votes manually for simplicity (or use other nodes)
    let votes: Vec<_> = keys
        .iter()
        .map(|(pk, sk)| {
            let sig = ockham::crypto::sign(sk, &b1_hash.0);
            ockham::types::Vote {
                view: 1,
                block_hash: b1_hash,
                vote_type: VoteType::Notarize,
                author: pk.clone(),
                signature: sig,
            }
        })
        .collect();

    // Feed votes to Node 0
    let mut finalize_votes = vec![];

    for vote in votes {
        // When QC is formed (on 3rd vote), Node 0 should broadcast Finalize(1)
        let actions = node0.on_vote(vote).unwrap();
        for action in actions {
            if let ConsensusAction::BroadcastVote(v) = action {
                if v.vote_type == VoteType::Finalize {
                    finalize_votes.push(v);
                }
            }
        }
    }

    // QC should be formed
    assert!(node0.qcs.contains_key(&1), "QC for View 1 should be formed");

    // 5. Verify Node 0 broadcasted a Finalize Vote
    assert!(
        !finalize_votes.is_empty(),
        "Node 0 should have broadcasted a Finalize vote upon forming QC"
    );

    // 6. Feed Finalize Votes back to Node 0
    // We need 3 Finalize votes to commit.
    // First, feed Node 0's own vote which it broadcasted
    let finalize_vote_0 = finalize_votes[0].clone();
    let _ = node0.on_vote(finalize_vote_0);

    // Fabricate finalize votes from Node 1, 2
    for (pk, sk) in keys.iter().skip(1).take(2) {
        let sig = ockham::crypto::sign(sk, &b1_hash.0);
        let fvote = ockham::types::Vote {
            view: 1,
            block_hash: b1_hash,
            vote_type: VoteType::Finalize,
            author: pk.clone(),
            signature: sig,
        };
        let _ = node0.on_vote(fvote);
    }

    // 7. Assert Finalization
    // Node 0 should have finalized View 1 immediately
    assert_eq!(
        node0.finalized_height, 1,
        "Node 0 should have finalized View 1 explicitly"
    );
    println!("SUCCESS: Explicit Finalization verified at Height 1");
}
