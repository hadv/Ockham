use crate::crypto::{
    Hash, PrivateKey, PublicKey, aggregate, hash_data, sign, verify, verify_aggregate,
};
use crate::storage::{ConsensusState, Storage};
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
    // Sync Actions
    BroadcastRequest(Hash),
    SendBlock(Block, String), // Respond to a specific peer (String is PeerId)
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

    // Storage (Abstracted)
    pub storage: Box<dyn Storage>,

    // Vote Aggregation
    // Vote Aggregation (split by type roughly, or just filter)
    pub votes_received: HashMap<View, HashMap<PublicKey, Vote>>,
    // Track Finalize votes separately for easier counting
    pub finalize_votes_received: HashMap<View, HashMap<PublicKey, Vote>>,

    // Sync: Orphan Buffer
    // Map: ParentHash -> List of Orphan Blocks waiting for that parent
    pub orphans: HashMap<Hash, Vec<Block>>,
}

impl SimplexState {
    pub fn new(
        my_id: PublicKey,
        my_key: PrivateKey,
        committee: Vec<PublicKey>,
        storage: Box<dyn Storage>,
    ) -> Self {
        // Attempt to load existing state
        if let Ok(Some(saved_state)) = storage.get_consensus_state() {
            log::info!(
                "Loaded persistent state: View {}, Finalized {}, Preferred View {}",
                saved_state.view,
                saved_state.finalized_height,
                saved_state.preferred_view
            );
            return Self {
                my_id,
                my_key,
                committee,
                current_view: saved_state.view,
                finalized_height: saved_state.finalized_height,
                preferred_block: saved_state.preferred_block,
                preferred_view: saved_state.preferred_view,
                storage,
                votes_received: HashMap::new(),
                finalize_votes_received: HashMap::new(),
                orphans: HashMap::new(),
            };
        }

        // Initialize Genesis
        let genesis_qc = QuorumCertificate::default();
        let genesis_block = Block::new(
            crate::crypto::generate_keypair_from_id(0).0,
            0,
            Hash::default(),
            genesis_qc.clone(),
            vec![],
        );
        let genesis_hash = hash_data(&genesis_block);

        // Save Genesis
        storage.save_block(&genesis_block).unwrap();
        // Dummy block for timeouts (mapped to genesis for simplicity or just empty)
        // In this implementation, we might not strictly need to save dummy explicitly if code handles it,
        // but let's save genesis as the "default" block.
        storage.save_qc(&genesis_qc).unwrap();

        let initial_state = ConsensusState {
            view: 1,
            finalized_height: 0,
            preferred_block: genesis_hash,
            preferred_view: 0,
        };
        storage.save_consensus_state(&initial_state).unwrap();

        Self {
            my_id,
            my_key,
            committee,
            current_view: initial_state.view,
            finalized_height: initial_state.finalized_height,
            preferred_block: initial_state.preferred_block,
            preferred_view: initial_state.preferred_view,
            storage,
            votes_received: HashMap::new(),
            finalize_votes_received: HashMap::new(),
            orphans: HashMap::new(),
        }
    }

    /// Triggered on start or view change to check if we should propose.
    pub fn try_propose(&mut self) -> Result<Vec<ConsensusAction>, ConsensusError> {
        if self.is_leader(self.current_view) {
            let prev_view = self.current_view - 1;
            if let Ok(Some(qc)) = self.storage.get_qc(prev_view) {
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

    /// Shared logic for validating and storing a block (Proposal or Sync).
    /// Returns true if the block was successfully stored (or already existed).
    /// Returns Actions (RequestBlock) if Orphan.
    fn validate_and_store_block(
        &mut self,
        block: Block,
    ) -> Result<(bool, Vec<ConsensusAction>), ConsensusError> {
        // 1. Check Parent (Simplex Lineage)
        if block.parent_hash != Hash::default()
            && self
                .storage
                .get_block(&block.parent_hash)
                .unwrap()
                .is_none()
        {
            // Orphan Logic: Buffer and Request Parent
            log::info!(
                "Received Orphan Block View {}. Parent {:?} missing. Buffering and Requesting...",
                block.view,
                block.parent_hash
            );
            self.orphans
                .entry(block.parent_hash)
                .or_default()
                .push(block.clone());
            return Ok((
                false,
                vec![ConsensusAction::BroadcastRequest(block.parent_hash)],
            ));
        }

        // 2. Verify QC
        self.verify_qc(&block.justify)?;

        // 3. Update preferred chain if this QC justifies a better block
        self.update_preferred_chain(&block.justify);

        // 4. Update state (store block)
        self.storage.save_block(&block).unwrap();

        Ok((true, vec![]))
    }

    /// Handle a new proposal.
    pub fn on_proposal(&mut self, block: Block) -> Result<Vec<ConsensusAction>, ConsensusError> {
        // 1. View Check (Strict for proposals)
        if block.view < self.current_view {
            // For live proposals, late blocks are irrelevant
            return Err(ConsensusError::InvalidView);
        }

        // 2. Common Validation & Storage
        let (stored, mut actions) = self.validate_and_store_block(block.clone())?;
        if !stored {
            return Ok(actions); // It was an orphan, request sent
        }

        // 3. Update view if needed (fast forward)
        if block.view >= self.current_view {
            self.current_view = block.view;
            self.persist_state();
        }

        // 4. Generate Vote
        let block_hash = hash_data(&block);
        let vote = self.create_vote(block.view, block_hash, VoteType::Notarize);
        actions.push(ConsensusAction::BroadcastVote(vote));

        // 5. Check if we should broadcast Finalize
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
        let mut signers = Vec::new();

        // Simple aggregation: check how many votes for this specific block_hash
        for v in view_votes.values() {
            if v.block_hash == vote.block_hash {
                count_for_block += 1;
                signatures.push(v.signature.clone());
                signers.push(v.author.clone());
            }
        }

        if count_for_block >= threshold {
            // QC Formed!
            // In a real system we'd handle failure better, but here we expect strictly valid signatures
            let aggregated_signature =
                aggregate(&signatures).expect("Failed to aggregate signatures");

            let qc = QuorumCertificate {
                view: vote.view,
                block_hash: vote.block_hash,
                signature: aggregated_signature,
                signers,
            };

            // Check if we haven't already processed this QC to avoid dupes?
            if self.storage.get_qc(vote.view).unwrap().is_none() {
                log::info!("QC Formed for View {}", vote.view);
                self.storage.save_qc(&qc).unwrap();
                self.update_preferred_chain(&qc);

                let next_view = vote.view + 1;

                // Broadcast Finalize for this View (since it is now notarized!)
                let finalize_vote =
                    self.create_vote(vote.view, vote.block_hash, VoteType::Finalize);
                let mut actions = vec![ConsensusAction::BroadcastVote(finalize_vote)];
                if next_view > self.current_view {
                    self.current_view = next_view;
                    self.persist_state();
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
                self.persist_state();
                // In real impl, we would commit transactions here.
            }
        }
        Ok(vec![])
    }

    fn verify_qc(&self, qc: &QuorumCertificate) -> Result<(), ConsensusError> {
        if qc.view == 0 {
            return Ok(());
        }
        if !verify_aggregate(&qc.signers, &qc.block_hash.0, &qc.signature) {
            return Err(ConsensusError::InvalidQC);
        }
        Ok(())
    }

    fn update_preferred_chain(&mut self, qc: &QuorumCertificate) {
        // If the QC certifies a real block (not dummy), and it's higher than what we have, update.
        if qc.block_hash != Hash::default() && qc.view >= self.preferred_view {
            self.preferred_view = qc.view;
            self.preferred_block = qc.block_hash;
            self.persist_state();
        }
    }

    fn persist_state(&self) {
        let state = ConsensusState {
            view: self.current_view,
            finalized_height: self.finalized_height,
            preferred_block: self.preferred_block,
            preferred_view: self.preferred_view,
        };
        if let Err(e) = self.storage.save_consensus_state(&state) {
            log::error!("Failed to persist state: {:?}", e);
        }
    }

    /// Handle a Block Request from a peer.
    pub fn on_block_request(
        &self,
        block_hash: Hash,
        peer_id: String,
    ) -> Result<Vec<ConsensusAction>, ConsensusError> {
        if let Ok(Some(block)) = self.storage.get_block(&block_hash) {
            log::info!("Serving Block Request for {:?}", block_hash);
            return Ok(vec![ConsensusAction::SendBlock(block, peer_id)]);
        }
        Ok(vec![])
    }

    /// Handle a Block Response (Synced Block).
    pub fn on_block_response(
        &mut self,
        block: Block,
    ) -> Result<Vec<ConsensusAction>, ConsensusError> {
        log::info!("Received Synced Block View {}", block.view);

        // Use shared validation logic (allows old blocks!)
        let (stored, mut actions) = self.validate_and_store_block(block.clone())?;

        if !stored {
            // It was an orphan, request sent via actions
            return Ok(actions);
        }

        // Fast-forward view if we synced a newer block
        if block.view >= self.current_view {
            self.current_view = block.view;
            self.persist_state();
        }

        // Check if this block fills any gaps (is a parent for orphans)
        let block_hash = hash_data(&block);
        if let Some(orphans) = self.orphans.remove(&block_hash) {
            log::info!(
                "Processed Orphan Parent. Re-processing {} orphans...",
                orphans.len()
            );
            for orphan in orphans {
                // Recursively process orphans
                if let Ok(orphan_actions) = self.on_block_response(orphan) {
                    actions.extend(orphan_actions);
                }
            }
        }

        Ok(actions)
    }
}
