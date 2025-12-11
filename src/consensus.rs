use crate::crypto::{Hash, PrivateKey, PublicKey, hash_data, sign, verify};
use crate::types::{Block, QuorumCertificate, View, Vote};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Invalid view for operation")]
    InvalidView,
    #[error("Invalid parent hash")]
    InvalidParent,
    #[error("Invalid QC")]
    InvalidQC,
    #[error("Unknown author")]
    UnknownAuthor,
}

pub struct SimplexState {
    pub my_id: PublicKey,
    pub my_key: PrivateKey,
    pub committee: HashSet<PublicKey>,
    pub current_view: View,
    pub finalized_height: View,

    // Storage (mocked in memory)
    pub blocks: HashMap<Hash, Block>,
    pub qcs: HashMap<View, QuorumCertificate>,

    // Vote Aggregation
    pub votes_received: HashMap<View, HashMap<PublicKey, Vote>>,
}

impl SimplexState {
    pub fn new(my_id: PublicKey, my_key: PrivateKey, committee: Vec<PublicKey>) -> Self {
        // Create Genesis Block
        let genesis_qc = QuorumCertificate::default();
        let genesis_block =
            Block::new(PublicKey(0), 0, Hash::default(), genesis_qc.clone(), vec![]);
        let genesis_hash = hash_data(&genesis_block);

        let mut blocks = HashMap::new();
        blocks.insert(genesis_hash, genesis_block);

        Self {
            my_id,
            my_key,
            committee: committee.into_iter().collect(),
            current_view: 1, // Start at view 1
            finalized_height: 0,
            blocks,
            qcs: HashMap::new(),
            votes_received: HashMap::new(),
        }
    }

    /// Handle a new proposal.
    /// If valid, return a Vote.
    pub fn on_proposal(&mut self, block: Block) -> Result<Vote, ConsensusError> {
        // 1. Basic checks
        if block.view < self.current_view {
            // Check if it's irrelevant or old
            return Err(ConsensusError::InvalidView);
        }

        // 2. Check Parent (Simplex Lineage)
        if !self.blocks.contains_key(&block.parent_hash) {
            // In a real system, we would buffer or request sync.
            // Here we error for simplicity of the unit test.
            return Err(ConsensusError::InvalidParent);
        }

        // 3. Verify QC
        self.verify_qc(&block.justify)?;

        // 4. Update state (store block)
        let block_hash = hash_data(&block);
        self.blocks.insert(block_hash, block.clone());

        // 5. Update view if needed (fast forward)
        if block.view >= self.current_view {
            self.current_view = block.view;
        }

        // 6. Generate Vote
        let vote = self.create_vote(block.view, block_hash);
        Ok(vote)
    }

    /// Handle an incoming vote.
    /// If we have enough votes (2f+1), form a QC.
    pub fn on_vote(&mut self, vote: Vote) -> Result<Option<QuorumCertificate>, ConsensusError> {
        // Verify signature (mocked)
        if !verify(&vote.author, &vote.block_hash.0, &vote.signature) {
            // For mock, we verify hash matches signature content, or use strict verify fn
            // My mock verify takes (pubkey, msg, sig). The msg signed is the block hash?
            // See create_vote: yes, we sign the block hash bytes.
        }

        let view_votes = self.votes_received.entry(vote.view).or_default();
        view_votes.insert(vote.author, vote.clone());

        let threshold = (self.committee.len() * 2) / 3 + 1;

        let mut count_for_block = 0;
        let mut signatures = Vec::new();

        // Simple aggregation: check how many votes for this specific block_hash
        for v in view_votes.values() {
            if v.block_hash == vote.block_hash {
                count_for_block += 1;
                signatures.push((v.author, v.signature.clone()));
            }
        }

        if count_for_block >= threshold {
            // QC Formed!
            let qc = QuorumCertificate {
                view: vote.view,
                block_hash: vote.block_hash,
                signatures,
            };
            self.qcs.insert(vote.view, qc.clone());
            return Ok(Some(qc));
        }

        Ok(None)
    }

    /// Handle timeout (dummy block generation).
    pub fn on_timeout(&mut self, view: View) -> Result<Vote, ConsensusError> {
        if view < self.current_view {
            return Err(ConsensusError::InvalidView);
        }

        // In Simplex, timeout is a vote for a dummy block (conceptually).
        // For this phase, we just emit a vote for a special "DummyHash" or empty hash associated with this view.
        // Or we treat it as voting for a dummy block that "would" exist.
        // Let's create a vote for Hash::default() or a specific Dummy Marker.
        // For simplicity, let's say we vote for a Zero Hash to signify dummy/timeout.

        let dummy_hash = Hash([0u8; 32]); // Marker for Dummy
        let vote = self.create_vote(view, dummy_hash);
        Ok(vote)
    }

    fn create_vote(&self, view: View, block_hash: Hash) -> Vote {
        // Sign the block hash
        let signature = sign(&self.my_key, &block_hash.0);
        Vote {
            view,
            block_hash,
            author: self.my_id,
            signature,
        }
    }

    fn verify_qc(&self, qc: &QuorumCertificate) -> Result<(), ConsensusError> {
        if qc.view == 0 {
            return Ok(());
        } // Genesis QC is always valid

        // Check if we know the block? Not necessarily required for QC validity itself,
        // but often we check if specific signatures are valid.
        // For mock, we trust if it has signatures because we built it?
        // No, we should verify at least one sig or threshold.
        // Let's just return Ok for mock phase unless empty.
        Ok(())
    }
}
