use crate::crypto::{
    Hash, PrivateKey, PublicKey, aggregate, hash_data, sign, verify, verify_aggregate,
};

use crate::evidence_pool::EvidencePool;
use crate::storage::{ConsensusState, StateOverlay, Storage};
use crate::tx_pool::TxPool;
use crate::types::{
    Block, EquivocationEvidence, INITIAL_BASE_FEE, QuorumCertificate, U256, View, Vote, VoteType,
};
use crate::vm::Executor;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Invalid view for operation")]
    InvalidView,
    #[error("Invalid parent hash")]
    InvalidParent,
    #[error("Invalid QC")]
    InvalidQC,
    #[error("Invalid Block")]
    InvalidBlock,
    #[error("Unknown author")]
    UnknownAuthor,
    #[error("Invalid State Root")]
    InvalidStateRoot,
    #[error("Invalid Receipts Root")]
    InvalidReceiptsRoot,
    #[error("Invalid Signature")]
    InvalidSignature,
}

/// Abstract actions emitted by the consensus state machine.
/// This decouples logic from side-effects (networking, timer, validation).
#[derive(Debug, Clone)]
pub enum ConsensusAction {
    BroadcastVote(Vote),
    BroadcastEvidence(EquivocationEvidence),
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
    pub last_voted_view: View,
    pub block_gas_limit: u64,

    // Storage (Abstracted)
    pub storage: std::sync::Arc<dyn Storage>,

    // Vote Aggregation
    // Vote Aggregation (split by type roughly, or just filter)
    pub votes_received: HashMap<View, HashMap<PublicKey, Vote>>,
    // Track Finalize votes separately for easier counting
    pub finalize_votes_received: HashMap<View, HashMap<PublicKey, Vote>>,

    // Sync: Orphan Buffer
    // Map: ParentHash -> List of Orphan Blocks waiting for that parent
    pub orphans: HashMap<Hash, Vec<Block>>,

    // Slashing
    pub evidence_pool: EvidencePool,

    // Execution & P2P
    pub tx_pool: Arc<TxPool>,
    pub executor: Executor,
}

impl SimplexState {
    pub fn new(
        my_id: PublicKey,
        my_key: PrivateKey,
        committee: Vec<PublicKey>,
        storage: std::sync::Arc<dyn Storage>,
        tx_pool: Arc<TxPool>,
        executor: Executor,
        block_gas_limit: u64,
    ) -> Self {
        // Attempt to load existing state
        if let Ok(Some(saved_state)) = storage.get_consensus_state() {
            log::info!(
                "Loaded persistent state: View {}, Finalized {}, Preferred View {}, Last Voted View {}",
                saved_state.view,
                saved_state.finalized_height,
                saved_state.preferred_view,
                saved_state.last_voted_view
            );
            if saved_state.committee != committee {
                log::warn!("Loaded committee differs from argument. Using persisted committee.");
            }
            let effective_committee = saved_state.committee.clone();

            return Self {
                my_id,
                my_key,
                committee: effective_committee,
                current_view: saved_state.view,
                finalized_height: saved_state.finalized_height,
                preferred_block: saved_state.preferred_block,
                preferred_view: saved_state.preferred_view,
                last_voted_view: saved_state.last_voted_view,
                storage,
                votes_received: HashMap::new(),
                finalize_votes_received: HashMap::new(),
                orphans: HashMap::new(),
                evidence_pool: EvidencePool::new(),
                tx_pool,
                executor,
                block_gas_limit: crate::types::DEFAULT_BLOCK_GAS_LIMIT,
            };
        }

        // Initialize Genesis
        let genesis_qc = QuorumCertificate::default();
        let genesis_block = Block::new(
            crate::crypto::generate_keypair_from_id(0).0,
            0,
            Hash::default(),
            genesis_qc.clone(),
            Hash::default(), // state_root
            Hash::default(), // receipts_root
            vec![],
            U256::from(INITIAL_BASE_FEE), // Genesis Base Fee
            0,
            vec![],          // Evidence
            Hash::default(), // Committee Hash
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
            last_voted_view: 0,
            committee: committee.clone(),
            pending_validators: vec![],
            exiting_validators: vec![],
            stakes: HashMap::new(),
        };
        storage.save_consensus_state(&initial_state).unwrap();

        // Allocating funds to Node 0 (Genesis Account)
        let (pk0, _) = crate::crypto::generate_keypair_from_id(0);
        let pk_bytes = pk0.0.to_bytes();
        let hash = crate::types::keccak256(pk_bytes);
        let address = crate::types::Address::from_slice(&hash[12..]);

        // Save account with max balance
        let account = crate::storage::AccountInfo {
            nonce: 0,
            balance: crate::types::U256::MAX,
            code_hash: crate::crypto::Hash(crate::types::keccak256([]).into()),
            code: None,
        };
        storage.save_account(&address, &account).unwrap();

        Self {
            my_id,
            my_key,
            committee,
            current_view: initial_state.view,
            finalized_height: initial_state.finalized_height,
            preferred_block: initial_state.preferred_block,
            preferred_view: initial_state.preferred_view,
            last_voted_view: initial_state.last_voted_view,
            storage,
            votes_received: HashMap::new(),
            finalize_votes_received: HashMap::new(),
            orphans: HashMap::new(),
            evidence_pool: EvidencePool::new(),
            tx_pool,
            executor,
            block_gas_limit,
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
                let mut block = self.create_proposal(self.current_view, qc.clone(), parent_hash)?;

                // Executor: Execute block to update state_root/receipts_root and validate transactions
                // USE EPHEMERAL OVERLAY for execution (do not commit to DB)
                let overlay = Arc::new(StateOverlay::new(self.storage.clone()));

                // Fork state from Parent Root
                let parent_root = if parent_hash == Hash::default() {
                    Hash::default()
                } else {
                    self.storage
                        .get_block(&parent_hash)
                        .ok()
                        .flatten()
                        .map(|b| b.state_root)
                        .unwrap_or_default()
                };

                let state_manager = Arc::new(Mutex::new(
                    self.executor
                        .state
                        .lock()
                        .unwrap()
                        .fork(parent_root, overlay),
                ));

                let executor = Executor::new(state_manager, self.block_gas_limit);

                executor
                    .execute_block(&mut block)
                    .map_err(|_e| ConsensusError::InvalidParent)?; // Map error appropriately

                log::info!(
                    "Proposal Executed (View {}): Root {:?}, Gas {}",
                    block.view,
                    block.state_root,
                    block.gas_used
                );

                // Clean up transactions from pool immediately
                self.tx_pool.remove_transactions(&block.payload);

                // SAVE the block immediately (Leader trusts own execution)
                // Note: StateOverlay ensures only block data is saved, not state changes.
                // Wait, we are calling self.storage.save_block directly here, so it IS saved.
                // This is correct. We want Block Data in DB, just not Account State.
                self.storage.save_block(&block).unwrap();

                // Remove included evidence from pool
                let evidence_in_block = block.evidence.clone();
                self.evidence_pool.remove_evidence(&evidence_in_block);

                let mut actions = vec![ConsensusAction::BroadcastBlock(block.clone())];

                // Generate Vote (Leader votes for own proposal)
                let block_hash = hash_data(&block);
                let vote = self.create_vote(block.view, block_hash, VoteType::Notarize);
                actions.push(ConsensusAction::BroadcastVote(vote));

                // Check Finalize (if QC justifies previous view)
                let qc_view = block.justify.view;
                if qc_view > 0 {
                    let finalize_vote =
                        self.create_vote(qc_view, block.justify.block_hash, VoteType::Finalize);
                    actions.push(ConsensusAction::BroadcastVote(finalize_vote));
                }

                return Ok(actions);
            }
        }
        Ok(vec![])
    }

    // Helper to cleanup tx pool after proposing
    pub fn cleanup_proposed_txs(&self, block: &Block) {
        self.tx_pool.remove_transactions(&block.payload);
    }

    /// Shared logic for validating and storing a block (Proposal or Sync).
    /// Returns true if the block was successfully stored (or already existed).
    /// Returns Actions (RequestBlock) if Orphan.
    fn validate_and_store_block(
        &mut self,
        block: Block,
    ) -> Result<(bool, Vec<ConsensusAction>), ConsensusError> {
        let block_hash = hash_data(&block);
        if self
            .storage
            .get_block(&block_hash)
            .unwrap_or(None)
            .is_some()
        {
            return Ok((true, vec![]));
        }
        // 1. Check Parent (Simplex Lineage)
        if block.parent_hash != Hash::default()
            && self
                .storage
                .get_block(&block.parent_hash)
                .unwrap()
                .is_none()
        {
            // Orphan Logic: Buffer and Request Parent
            println!(
                "DEBUG: Orphan Detected. Parent not found: {:?}",
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

        // 1.1 Committee Hash Check
        let expected_committee_hash = hash_data(&self.committee);
        if block.committee_hash != expected_committee_hash {
            log::warn!(
                "Invalid Committee Hash: Expected {:?}, Got {:?}",
                expected_committee_hash,
                block.committee_hash
            );
            return Err(ConsensusError::InvalidBlock); // Or specific error
        }

        // 1.2 Fork/Lineage Check
        // 1.2 Fork/Lineage Check
        // Disabled because SMT Root in blocks (ephemeral) differs from Local SMT Root (persistent) in current implementation.
        // if let Ok(Some(parent)) = self.storage.get_block(&block.parent_hash) {
        //     let current_root = self.executor.state.lock().unwrap().root();
        //     if parent.state_root != current_root {
        //         println!("DEBUG: Fork Detected! Parent Root {:?} != Local Root {:?}", parent.state_root, current_root);
        //         // Let's drop it to silence the error.
        //         return Ok((false, vec![]));
        //     }
        // }

        // 1.5 Execute Block (Validation)
        // We must re-execute to verify state_root and receipts_root matches.
        let overlay = Arc::new(StateOverlay::new(self.storage.clone()));

        // Fork state from Parent Root
        let parent_root = if block.parent_hash == Hash::default() {
            Hash::default()
        } else {
            self.storage
                .get_block(&block.parent_hash)
                .ok()
                .flatten()
                .map(|b| b.state_root)
                .unwrap_or_default()
        };

        let state_manager = Arc::new(Mutex::new(
            self.executor
                .state
                .lock()
                .unwrap()
                .fork(parent_root, overlay),
        ));

        let executor = Executor::new(state_manager, self.block_gas_limit);

        let mut executed_block = block.clone();
        // Clear gas used/roots to verify execution recreation
        executed_block.gas_used = 0;
        // executed_block.state_root = Hash::default(); // Keep original to compare? No, executor overwrites it.

        executor.execute_block(&mut executed_block).map_err(|e| {
            log::error!("Block Execution Failed: {:?}", e);
            ConsensusError::InvalidBlock
        })?;

        if block.state_root != executed_block.state_root {
            log::error!(
                "Invalid State Root: expected {:?}, got {:?}",
                block.state_root,
                executed_block.state_root
            );
            return Err(ConsensusError::InvalidStateRoot);
        }

        if executed_block.receipts_root != block.receipts_root {
            log::error!(
                "Invalid Receipts Root: expected {:?}, got {:?}",
                block.receipts_root,
                executed_block.receipts_root
            );
            return Err(ConsensusError::InvalidReceiptsRoot);
        }

        // 2. Verify QC
        self.verify_qc(&block.justify)?;

        // 3. Update preferred chain if this QC justifies a better block
        self.update_preferred_chain(&block.justify);

        // 4. Update state (store block)
        self.storage.save_block(&block).unwrap();

        // 5. Clean up TxPool
        // Remove transactions included in this valid block from our pool
        self.tx_pool.remove_transactions(&block.payload);

        // Remove included evidence from pool (if any)
        self.evidence_pool.remove_evidence(&block.evidence);

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

        // 4. Generate Vote (Strict Check)
        if block.view <= self.last_voted_view {
            // We already voted for this view (or a higher one). Do not vote again.
            // Parallel Chain Prevention: Honest nodes MUST NOT equivocate.
            log::warn!(
                "Double Voting Attempt Rejected: View {}, Last Voted {}",
                block.view,
                self.last_voted_view
            );
            return Ok(actions);
        }

        // UPDATE AND PERSIST STATE BEFORE VOTING
        self.last_voted_view = block.view;
        self.persist_state(); // Critical: Persist the fact that we voted.

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
        // Verify signature
        if !verify(&vote.author, &vote.block_hash.0, &vote.signature) {
            log::warn!("Invalid signature from author {:?}", vote.author);
            return Err(ConsensusError::InvalidSignature);
        }

        if vote.vote_type == VoteType::Finalize {
            return self.on_finalize_vote(vote);
        }

        let view_votes = self.votes_received.entry(vote.view).or_default();

        // 0. Equivocation Check
        // 0. Equivocation Check
        if let Some(existing_vote) = view_votes.get(&vote.author)
            && existing_vote.block_hash != vote.block_hash
        {
            log::warn!(
                "Equivocation Detected from {:?} in View {}",
                vote.author,
                vote.view
            );
            let evidence = EquivocationEvidence {
                vote_a: existing_vote.clone(),
                vote_b: vote.clone(),
            };
            // Add to pool and broadcast
            if self.evidence_pool.add_evidence(evidence.clone()) {
                return Ok(vec![ConsensusAction::BroadcastEvidence(evidence)]);
            } else {
                return Ok(vec![]);
            }
        }

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
                    log::info!(
                        "I am the leader for View {}! Proposing block (Chain)...",
                        next_view
                    );
                    // FIX: If QC (from vote) is for a dummy block, extend preferred_block
                    let parent_hash = if vote.block_hash == Hash::default() {
                        self.preferred_block
                    } else {
                        vote.block_hash
                    };

                    if let Ok(mut block) = self.create_proposal(next_view, qc, parent_hash) {
                        // Full Proposal Lifecycle (Ephemeral Execution)
                        let overlay = Arc::new(StateOverlay::new(self.storage.clone()));
                        let parent_root = if parent_hash == Hash::default() {
                            Hash::default()
                        } else {
                            self.storage
                                .get_block(&parent_hash)
                                .ok()
                                .flatten()
                                .map(|b| b.state_root)
                                .unwrap_or(Hash::default())
                        };

                        let state_manager = Arc::new(Mutex::new(
                            self.executor
                                .state
                                .lock()
                                .unwrap()
                                .fork(parent_root, overlay),
                        ));
                        let executor = Executor::new(state_manager, self.block_gas_limit);

                        if executor.execute_block(&mut block).is_ok() {
                            log::info!(
                                "Proposal Executed (Chain). View: {}, Root: {:?}, Gas: {}",
                                block.view,
                                block.state_root,
                                block.gas_used
                            );

                            self.tx_pool.remove_transactions(&block.payload);
                            self.storage.save_block(&block).unwrap();

                            actions.push(ConsensusAction::BroadcastBlock(block.clone()));

                            // Vote for own block
                            let block_hash = hash_data(&block);
                            let vote = self.create_vote(block.view, block_hash, VoteType::Notarize);
                            actions.push(ConsensusAction::BroadcastVote(vote));

                            // Finalize Vote if justified
                            let qc_view = block.justify.view;
                            if qc_view > 0 {
                                let finalize_vote = self.create_vote(
                                    qc_view,
                                    block.justify.block_hash,
                                    VoteType::Finalize,
                                );
                                actions.push(ConsensusAction::BroadcastVote(finalize_vote));
                            }
                        } else {
                            log::error!("Failed to execute chained proposal View {}", next_view);
                        }
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
        // Calculate Next Base Fee based on Parent
        // We need to fetch the parent block to know its gas_used and base_fee.
        // We know 'parent' hash.
        let base_fee = if let Ok(Some(parent_block)) = self.storage.get_block(&parent) {
            self.calculate_next_base_fee(&parent_block)
        } else {
            // FIX: If we can't find the parent, we can't safely propose because:
            // 1. We don't know the base fee.
            // 2. We haven't executed the parent, so our DB state is likely stale.
            // 3. We might re-include transactions that were already in the parent.
            // (Unless it's Genesis, but Genesis handling should ensure it's saved).
            log::warn!(
                "Parent block {:?} not found. Dropping proposal opportunity.",
                parent
            );
            // We should ideally request sync here too.
            return Err(ConsensusError::InvalidParent);
        };

        // Filter transactions by base_fee
        // Note: get_transactions_for_block should now assume sorted by priority fee and filter by base_fee
        let payload = self
            .tx_pool
            .get_transactions_for_block(self.block_gas_limit, base_fee);

        // Note: We don't know gas_used yet, only at execution.
        // But Block::new requires it?
        // Actually, for a PROPOSAL, gas_used is 0 (unexecuted) or predicted?
        // In this architecture, we execute IMMEDIATELY after creation in try_propose.
        // So we can initialize with 0, and executor updates it.

        let block = Block::new(
            self.my_id.clone(),
            view,
            parent, // Parent of new block is the block certified by QC
            qc,
            Hash::default(), // state_root (Calculated later in execute_block)
            Hash::default(), // receipts_root
            payload,
            base_fee,
            0,                            // gas_used initialized to 0, updated by executor
            self.evidence_pool.get_all(), // Include all pending evidence
            hash_data(&self.committee),   // Committee Hash
        );
        Ok(block)
    }

    /// EIP-1559 Base Fee Calculation
    fn calculate_next_base_fee(&self, parent: &Block) -> U256 {
        let elasticity_multiplier = 2;
        let base_fee_max_change_denominator = 8;
        let target_gas = self.block_gas_limit / elasticity_multiplier;

        let parent_gas_used = parent.gas_used;
        let parent_base_fee = parent.base_fee_per_gas;

        if parent_gas_used == target_gas {
            parent_base_fee
        } else if parent_gas_used > target_gas {
            let gas_used_delta = parent_gas_used - target_gas;
            let base_fee_increase = parent_base_fee * U256::from(gas_used_delta)
                / U256::from(target_gas)
                / U256::from(base_fee_max_change_denominator);
            parent_base_fee + base_fee_increase
        } else {
            let gas_used_delta = target_gas - parent_gas_used;
            let base_fee_decrease = parent_base_fee * U256::from(gas_used_delta)
                / U256::from(target_gas)
                / U256::from(base_fee_max_change_denominator);
            parent_base_fee.saturating_sub(base_fee_decrease)
        }
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

                // Check for Dummy Block (Timeout)
                if vote.block_hash == Hash::default() {
                    log::info!(
                        "Finalized Dummy Block (Timeout) for View {}. Skipping state commit.",
                        vote.view
                    );
                    return Ok(vec![]);
                }

                // COMMIT STATE (Re-execute against persistent storage)
                match self.storage.get_block(&vote.block_hash) {
                    Ok(Some(mut block)) => {
                        log::info!("Committing Finalized Block View {}", block.view);
                        // Use self.executor which points to REAL storage
                        if let Err(e) = self.executor.execute_block(&mut block) {
                            log::error!("CRITICAL: Failed to commit finalized block: {:?}", e);
                        } else {
                            log::info!("State Committed for View {}", block.view);

                            // RELOAD COMMITTEE from System Contract (Storage)
                            let db = self.executor.state.lock().unwrap();
                            if let Ok(Some(state)) = db.get_consensus_state() {
                                // Update local view of committee
                                self.committee = state.committee;
                                log::info!("Updated Validator Set. Size: {}", self.committee.len());
                            }
                        }
                    }
                    Ok(None) => {
                        log::warn!(
                            "Finalized block not found in storage: {:?}",
                            vote.block_hash
                        );
                        // We might need to request it?
                    }
                    Err(e) => {
                        log::error!("Storage error fetching finalized block: {:?}", e);
                    }
                }
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
        // Read-Modify-Write to preserve pending/exiting/stakes which we don't track in memory
        let mut state = self
            .storage
            .get_consensus_state()
            .unwrap()
            .unwrap_or(ConsensusState {
                view: self.current_view,
                finalized_height: self.finalized_height,
                preferred_block: self.preferred_block,
                preferred_view: self.preferred_view,
                last_voted_view: self.last_voted_view,
                committee: self.committee.clone(),
                pending_validators: vec![],
                exiting_validators: vec![],
                stakes: HashMap::new(),
            });

        // Update fields we manage
        state.view = self.current_view;
        state.finalized_height = self.finalized_height;
        state.preferred_block = self.preferred_block;
        state.preferred_view = self.preferred_view;
        state.last_voted_view = self.last_voted_view;
        state.committee = self.committee.clone();

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
