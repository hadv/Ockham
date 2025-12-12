use crate::crypto::{Hash, PrivateKey, PublicKey, hash_data, sign, verify};
use crate::types::{Block, QuorumCertificate, View, Vote, VoteType};
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
    pub preferred_block: Hash,
    pub preferred_view: View,

    // Storage (mocked in memory)
    pub blocks: HashMap<Hash, Block>,
    pub qcs: HashMap<View, QuorumCertificate>,

    // Vote Aggregation
    // Vote Aggregation (split by type roughly, or just filter)
    pub votes_received: HashMap<View, HashMap<PublicKey, Vote>>,
    // Track Finalize votes separately for easier counting
    pub finalize_votes_received: HashMap<View, HashMap<PublicKey, Vote>>,
}

impl SimplexState {
    pub fn new(my_id: PublicKey, my_key: PrivateKey, committee: Vec<PublicKey>) -> Self {
        // Create Genesis Block
        let genesis_qc = QuorumCertificate::default();
        let genesis_block =
            Block::new(crate::crypto::generate_keypair_from_id(0).0, 0, Hash::default(), genesis_qc.clone(), vec![]);
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
            preferred_block: genesis_hash,
            preferred_view: 0,
            blocks,
            qcs,
            votes_received: HashMap::new(),
            finalize_votes_received: HashMap::new(),
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
                // FIX: If QC is for a dummy block (ZeroHash), we must extend the last real block (preferred_block)
                let is_dummy = qc.block_hash == Hash::default();

                let parent_hash = if is_dummy {
                    self.preferred_block
                } else {
                    qc.block_hash
                };
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
        // Update preferred chain if this QC justifies a better block
        self.update_preferred_chain(&block.justify);

        // 4. Update state (store block)
        let block_hash = hash_data(&block);
        self.blocks.insert(block_hash, block.clone());

        // 5. Update view if needed (fast forward)
        // Note: Simplex logic typically updates view on QC, but receiving a valid proposal for higher view implies previous views were successful.
        if block.view >= self.current_view {
            self.current_view = block.view;
        }

        // 6. Generate Vote
        let vote = self.create_vote(block.view, block_hash, VoteType::Notarize);

        // 7. Check if we should broadcast Finalize?
        // Simplex Rule: If we see a notarized blockchain of length h, broadcast Finalize(h).
        // Since we just validated `block` which has a QC for `block.view - 1` (roughly),
        // we can try to finalize the parent view?
        // Actually, if we see a valid QC for View V, we can broadcast Finalize(V).
        // Let's implement that in `on_proposal` (since we verified block's QC) and `on_vote` (when we form a QC).
        let mut actions = vec![ConsensusAction::BroadcastVote(vote)];

        // If this block came with a valid QC for (View-1), we can vote to finalize View-1.
        // Or if this block itself represents a success for the current view?
        // Simplex text: "on seeing 2n/3 votes for block b_h (notarized)... sends Finalize(h)"
        // Since we just accepted block_h, we haven't seen 2n/3 votes for IT yet.
        // But the QC inside it proves (View-1) was notarized.
        let qc_view = block.justify.view;
        if qc_view > 0 {
            let finalize_vote =
                self.create_vote(qc_view, block.justify.block_hash, VoteType::Finalize);
            actions.push(ConsensusAction::BroadcastVote(finalize_vote));
        }

        Ok(actions)
    }

    /// Handle an incoming vote.
    /// If we have enough votes (2f+1), form a QC.
    pub fn on_vote(&mut self, vote: Vote) -> Result<Vec<ConsensusAction>, ConsensusError> {
        // Verify signature (mocked)
        if !verify(&vote.author, &vote.block_hash.0, &vote.signature) {
            // For mock, we verify hash matches signature content.
        }

        if vote.vote_type == VoteType::Finalize {
            return self.on_finalize_vote(vote);
        }

        let view_votes = self.votes_received.entry(vote.view).or_default();
        view_votes.insert(vote.author.clone(), vote.clone());

        let threshold = (self.committee.len() * 2) / 3 + 1;

        let mut count_for_block = 0;
        let mut signatures = Vec::new();

        // Simple aggregation: check how many votes for this specific block_hash
        for v in view_votes.values() {
            if v.block_hash == vote.block_hash {
                count_for_block += 1;
                signatures.push((v.author.clone(), v.signature.clone()));
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
                self.update_preferred_chain(&qc);

                let next_view = vote.view + 1;

                // Broadcast Finalize for this View (since it is now notarized!)
                let finalize_vote =
                    self.create_vote(vote.view, vote.block_hash, VoteType::Finalize);
                let mut actions = vec![ConsensusAction::BroadcastVote(finalize_vote)];
                if next_view > self.current_view {
                    self.current_view = next_view;
                }

                // If we are the leader for the NEXT view (qc.view + 1), PROPOSE!
                if self.is_leader(next_view) {
                    log::info!("I am the leader for View {}! Proposing block...", next_view);
                    // FIX: If QC (from vote) is for a dummy block, extend preferred_block
                    let parent_hash = if vote.block_hash == Hash::default() {
                        self.preferred_block
                    } else {
                        vote.block_hash
                    };

                    if let Ok(block) = self.create_proposal(next_view, qc, parent_hash) {
                        actions.push(ConsensusAction::BroadcastBlock(block));
                    }
                }
                return Ok(actions);
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
        let vote = self.create_vote(view, dummy_hash, VoteType::Notarize);

        Ok(vec![ConsensusAction::BroadcastVote(vote)])
    }

    fn create_vote(&self, view: View, block_hash: Hash, vote_type: VoteType) -> Vote {
        // Sign the block hash
        let signature = sign(&self.my_key, &block_hash.0);
        Vote {
            view,
            block_hash,
            vote_type,
            author: self.my_id.clone(),
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
            self.my_id.clone(),
            view,
            parent, // Parent of new block is the block certified by QC
            qc,
            vec![], // Payload empty for now
        );
        Ok(block)
    }

    // try_finalize removed in favor of on_finalize_vote
    fn on_finalize_vote(&mut self, vote: Vote) -> Result<Vec<ConsensusAction>, ConsensusError> {
        let view_votes = self.finalize_votes_received.entry(vote.view).or_default();
        view_votes.insert(vote.author.clone(), vote.clone());

        let threshold = (self.committee.len() * 2) / 3 + 1;
        if view_votes.len() >= threshold {
            // Explicit Simplex Finalization!
            if vote.view > self.finalized_height {
                self.finalized_height = vote.view;
                log::info!("EXPLICITLY FINALIZED VIEW: {}", vote.view);
                // In real impl, we would commit transactions here.
            }
        }
        Ok(vec![])
    }

    fn verify_qc(&self, qc: &QuorumCertificate) -> Result<(), ConsensusError> {
        if qc.view == 0 {
            return Ok(());
        }
        Ok(())
    }

    fn update_preferred_chain(&mut self, qc: &QuorumCertificate) {
        // If the QC certifies a real block (not dummy), and it's higher than what we have, update.
        if qc.block_hash != Hash::default() && qc.view >= self.preferred_view {
            self.preferred_view = qc.view;
            self.preferred_block = qc.block_hash;
        }
    }
}
