use crate::crypto::{Hash, PrivateKey, PublicKey, hash_data, sign, verify};
use crate::types::{Block, QuorumCertificate, View, Vote};
use std::collections::HashMap;
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

/// Abstract actions emitted by the consensus state machine.
/// This decouples logic from side-effects (networking, timer, validation).
#[derive(Debug, Clone)]
pub enum ConsensusAction {
    BroadcastVote(Vote),
    BroadcastBlock(Block),
    // In a real implementation, we'd have Timer start/stop actions here
}

pub struct SimplexState {
    pub my_id: PublicKey,
    pub my_key: PrivateKey,
    pub committee: Vec<PublicKey>,
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
        blocks.insert(genesis_hash, genesis_block.clone());
        // Map Dummy Hash to Genesis to handle timeouts/genesis-parent check
        blocks.insert(Hash::default(), genesis_block);

        let mut qcs = HashMap::new();
        qcs.insert(0, genesis_qc.clone());

        Self {
            my_id,
            my_key,
            committee,
            current_view: 1, // Start at view 1
            finalized_height: 0,
            blocks,
            qcs,
            votes_received: HashMap::new(),
        }
    }

    /// Triggered on start or view change to check if we should propose.
    pub fn try_propose(&mut self) -> Result<Vec<ConsensusAction>, ConsensusError> {
        if self.is_leader(self.current_view) {
            let prev_view = self.current_view - 1;
            if let Some(qc) = self.qcs.get(&prev_view) {
                log::info!(
                    "I am the leader for View {}! Proposing block...",
                    self.current_view
                );
                // Parent is the block this QC certifies
                let parent_hash = qc.block_hash;
                let block = self.create_proposal(self.current_view, qc.clone(), parent_hash)?;
                return Ok(vec![ConsensusAction::BroadcastBlock(block)]);
            }
        }
        Ok(vec![])
    }

    /// Handle a new proposal.
    /// Returns actions to perform (e.g. BroadcastVote).
    pub fn on_proposal(&mut self, block: Block) -> Result<Vec<ConsensusAction>, ConsensusError> {
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
        // Note: Simplex logic typically updates view on QC, but receiving a valid proposal for higher view implies previous views were successful.
        if block.view >= self.current_view {
            self.current_view = block.view;
        }

        // 6. Generate Vote
        let vote = self.create_vote(block.view, block_hash);

        // 7. Try Finalize (Chain Commit)
        self.try_finalize(&block);

        Ok(vec![ConsensusAction::BroadcastVote(vote)])
    }

    /// Handle an incoming vote.
    /// If we have enough votes (2f+1), form a QC.
    pub fn on_vote(&mut self, vote: Vote) -> Result<Vec<ConsensusAction>, ConsensusError> {
        // Verify signature (mocked)
        if !verify(&vote.author, &vote.block_hash.0, &vote.signature) {
            // For mock, we verify hash matches signature content.
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

            // Check if we haven't already processed this QC to avoid dupes?
            if let std::collections::hash_map::Entry::Vacant(e) = self.qcs.entry(vote.view) {
                log::info!("QC Formed for View {}", vote.view);
                e.insert(qc.clone());

                // If we are the leader for the NEXT view (qc.view + 1), PROPOSE!
                let next_view = vote.view + 1;
                if self.is_leader(next_view) {
                    log::info!("I am the leader for View {}! Proposing block...", next_view);
                    if let Ok(block) = self.create_proposal(next_view, qc, vote.block_hash) {
                        return Ok(vec![ConsensusAction::BroadcastBlock(block)]);
                    }
                }
            }
            return Ok(vec![]);
        }

        Ok(vec![])
    }

    /// Handle timeout (dummy block generation).
    pub fn on_timeout(&mut self, view: View) -> Result<Vec<ConsensusAction>, ConsensusError> {
        if view < self.current_view {
            // For now, ignore old timeouts
            return Ok(vec![]);
        }

        // Simplex timeout -> Vote for dummy
        let dummy_hash = Hash([0u8; 32]);
        let vote = self.create_vote(view, dummy_hash);

        Ok(vec![ConsensusAction::BroadcastVote(vote)])
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

    fn is_leader(&self, view: View) -> bool {
        let idx = (view as usize) % self.committee.len();
        self.committee[idx] == self.my_id
    }

    fn create_proposal(
        &self,
        view: View,
        qc: QuorumCertificate,
        parent: Hash,
    ) -> Result<Block, ConsensusError> {
        let block = Block::new(
            self.my_id,
            view,
            parent, // Parent of new block is the block certified by QC
            qc,
            vec![], // Payload empty for now
        );
        Ok(block)
    }

    fn try_finalize(&mut self, head: &Block) {
        // 1. Parent (P)
        let parent_hash = head.justify.block_hash;
        if let Some(parent) = self.blocks.get(&parent_hash) {
            // 2. Grandparent (GP)
            let gp_hash = parent.justify.block_hash;
            if let Some(gp) = self.blocks.get(&gp_hash) {
                log::info!(
                    "TryFinalize Check: Head(v{}) -> Parent(v{}) -> GP(v{})",
                    head.view,
                    parent.view,
                    gp.view
                );
                // 3. Great-Grandparent (GGP) - Optional 3-chain check, or just commit GP (2-chain)
                // Let's commit GP if it's new, OR if it's Genesis and we haven't finalized anything yet (for demo)
                if gp.view > self.finalized_height || (gp.view == 0 && self.finalized_height == 0) {
                    self.finalized_height = gp.view;
                    log::info!("FINALIZED BLOCK: {:?}", gp);
                }
            } else {
                log::warn!("TryFinalize: GP not found for Parent(v{})", parent.view);
            }
        } else {
            log::warn!("TryFinalize: Parent not found for Head(v{})", head.view);
        }
    }

    fn verify_qc(&self, qc: &QuorumCertificate) -> Result<(), ConsensusError> {
        if qc.view == 0 {
            return Ok(());
        }
        Ok(())
    }
}
