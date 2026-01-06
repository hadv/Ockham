use crate::crypto::{Hash, verify};
use crate::state::StateManager;
use crate::types::{Block, Transaction};
use revm::Database; // Import for .basic() method
use revm::{
    EVM,
    primitives::{Address, CreateScheme, ExecutionResult, ResultAndState, TransactTo, U256},
};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("EVM Error: {0}")]
    Evm(String),
    #[error("State Error: {0}")]
    State(String),
    #[error("Transaction Error: {0}")]
    Transaction(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemStorage;

    #[test]
    fn test_execute_block_gas_limit() {
        let storage = Arc::new(MemStorage::new());
        let state = Arc::new(Mutex::new(StateManager::new(storage, None)));
        let _executor = Executor::new(state, 10_000_000); // reduced limit

        // ...
    }
}

#[derive(Clone)]
pub struct Executor {
    pub state: Arc<Mutex<StateManager>>,
    pub block_gas_limit: u64,
}

impl Executor {
    pub fn new(state: Arc<Mutex<StateManager>>, block_gas_limit: u64) -> Self {
        Self {
            state,
            block_gas_limit,
        }
    }

    pub fn execute_block(&self, block: &mut Block) -> Result<(), ExecutionError> {
        // Validation: Ensure block gas limit is respected by consensus
        // Also consensus ensures parent hash linkage.

        let mut db = self.state.lock().unwrap();
        let mut cumulative_gas_used = 0u64;
        log::info!(
            "Executing block view {} with {} txs",
            block.view,
            block.payload.len()
        );

        // 0. Process Evidence (Slashing)
        self.process_equivocation_slashing(block, &mut db);

        // 0.5 Process Liveness (Leader Slashing)
        self.process_liveness_slashing(block, &mut db);

        // Pre-scan for block limits
        for tx in &block.payload {
            if tx.gas_limit() > crate::types::MAX_TX_GAS_LIMIT {
                return Err(ExecutionError::Transaction(
                    "Tx exceeds fixed tx gas limit (Fusaka)".into(),
                ));
            }
            if tx.gas_limit() > self.block_gas_limit {
                return Err(ExecutionError::Transaction(
                    "Tx exceeds block gas limit".into(),
                ));
            }
        }

        let mut receipts = Vec::with_capacity(block.payload.len());

        for (i, tx) in block.payload.iter().enumerate() {
            // 1. Validate Transaction
            if tx.sender() == Address::ZERO {
                return Err(ExecutionError::Transaction("Invalid sender".into()));
            }

            // AA Validation Phase (EIP-7701)
            if let Transaction::AA(aa_tx) = tx {
                log::info!("Validating AA Tx from {:?}", aa_tx.sender);
                // We must use a separate EVM context or carefully manage state.
                // For MVP, we run validation on the SAME db as execution.
                // If validation fails, the whole block is invalid (consensus rule).
                self.validate_aa_transaction(aa_tx, &mut db, block.base_fee_per_gas)?;
            }

            // SYSTEM CONTRACT INTERCEPTION (Address 0x1000)
            let sys_contract = Address::from_slice(
                &hex::decode("0000000000000000000000000000000000001000").unwrap(),
            );

            if tx.to() == Some(sys_contract) {
                // Only Legacy Transactions can interact with System Contract for Staking
                // because they have the PublicKey needed for consensus.
                if let Transaction::Legacy(legacy_tx) = tx {
                    self.process_system_contract(
                        legacy_tx,
                        &mut db,
                        &mut receipts,
                        cumulative_gas_used,
                        block.view,
                    )?;
                } else {
                    log::warn!(
                        "AA Transaction attempted to call System Contract (Staking). Ignored."
                    );
                    // We consume nonce? Yes to prevent replay loop.
                    // Charge base gas? Yes.
                    // Basic fee deduction
                    let _sender_acc = db.basic(tx.sender()).unwrap().unwrap_or_default();
                    let _cost = tx.gas_limit() as u128 * tx.max_fee_per_gas().to::<u128>(); // Simplified
                    // ... Just skip for now or treat as failed tx.
                    receipts.push(crate::types::Receipt {
                        status: 0,
                        cumulative_gas_used,
                        logs: vec![],
                    });
                }
                continue;
            }

            // 2. Setup EVM
            let mut evm = EVM::new();
            evm.database(&mut *db);

            // Set Block Info
            evm.env.block.basefee = block.base_fee_per_gas;

            // 3. Populate TxEnv
            let tx_env = &mut evm.env.tx;
            tx_env.caller = tx.sender();
            tx_env.transact_to = if let Some(to) = tx.to() {
                TransactTo::Call(to)
            } else {
                TransactTo::Create(CreateScheme::Create)
            };
            tx_env.data = tx.data().clone();
            tx_env.value = tx.value();
            tx_env.gas_limit = tx.gas_limit();
            tx_env.gas_price = tx.max_fee_per_gas();
            tx_env.gas_priority_fee = Some(tx.max_priority_fee_per_gas());
            tx_env.nonce = Some(tx.nonce());

            // 4. Execute
            let result_and_state = evm
                .transact()
                .map_err(|e| ExecutionError::Evm(format!("{:?}", e)))?;

            // 5. Commit state changes
            let ResultAndState { result, state } = result_and_state;

            // Track gas and extract logs
            let (gas_used, status, logs) = match result {
                ExecutionResult::Success { gas_used, logs, .. } => (gas_used, 1u8, logs),
                ExecutionResult::Revert { gas_used, output } => {
                    log::warn!("Tx Reverted! Gas: {}, Output: {:?}", gas_used, output);
                    (gas_used, 0u8, vec![])
                }
                ExecutionResult::Halt {
                    gas_used, reason, ..
                } => {
                    log::warn!("Tx Halted! Gas: {}, Reason: {:?}", gas_used, reason);
                    (gas_used, 0u8, vec![])
                }
            };
            cumulative_gas_used += gas_used;
            log::info!(
                "Tx {} executed. Gas used: {}. Cumulative: {}",
                i,
                gas_used,
                cumulative_gas_used
            );

            // Create Receipt
            let receipt_logs: Vec<crate::types::Log> = logs
                .into_iter()
                .map(|l| crate::types::Log {
                    address: l.address,
                    topics: l.topics.into_iter().map(|t| Hash(t.0)).collect(),
                    data: l.data,
                })
                .collect();

            receipts.push(crate::types::Receipt {
                status,
                cumulative_gas_used,
                logs: receipt_logs,
            });

            if status == 1 {
                // Success
                for (address, account) in state {
                    let info = crate::storage::AccountInfo {
                        nonce: account.info.nonce,
                        balance: account.info.balance,
                        code_hash: Hash(account.info.code_hash.0),
                        code: account.info.code.map(|c| c.original_bytes()),
                    };

                    db.commit_account(address, info)
                        .map_err(|e| ExecutionError::State(e.to_string()))?;

                    for (index, slot) in account.storage {
                        let val = slot.present_value;
                        db.commit_storage(address, index, val)
                            .map_err(|e| ExecutionError::State(e.to_string()))?;
                    }
                }
            }
        }

        // 6. Process Queues (End of Block)
        {
            // Use existing 'db' lock
            if let Ok(Some(mut state)) = db.get_consensus_state() {
                let current_view = block.view;
                let mut changed = false;

                // Process Pending -> Active
                // Using retain is tricky with moving items, so we'll use partition or just loop
                let (ready, not_ready): (Vec<_>, Vec<_>) = state
                    .pending_validators
                    .into_iter()
                    .partition(|(_, v)| *v <= current_view);
                state.pending_validators = not_ready;

                for (pk, _) in ready {
                    if !state.committee.contains(&pk) {
                        state.committee.push(pk);
                        changed = true;
                    }
                }

                // Process Exiting -> Removed
                let (exited, still_exiting): (Vec<_>, Vec<_>) = state
                    .exiting_validators
                    .into_iter()
                    .partition(|(_, v)| *v <= current_view);
                state.exiting_validators = still_exiting;

                for (pk, _) in exited {
                    if let Some(pos) = state.committee.iter().position(|x| *x == pk) {
                        state.committee.remove(pos);
                        changed = true;
                    }
                }

                if changed {
                    db.save_consensus_state(&state).unwrap();
                }

                // Refresh State Root if consensus state changed?
                // ConsensusState is in DB so root changes automatically.
            }
        }

        // No need to re-lock, 'db' is still valid
        block.state_root = db.root();
        block.receipts_root = crate::types::calculate_receipts_root(&receipts);
        block.gas_used = cumulative_gas_used;
        log::info!(
            "Block Execution Complete. State Root: {:?}, Receipts Root: {:?}, Gas Used: {}",
            block.state_root,
            block.receipts_root,
            block.gas_used
        );

        Ok(())
    }

    /// Validates an AA Transaction by calling the `validateTransaction` function on the sender contract.
    fn validate_aa_transaction(
        &self,
        tx: &crate::types::AATransaction,
        db: &mut StateManager,
        base_fee: U256,
    ) -> Result<(), ExecutionError> {
        let mut evm = EVM::new();
        evm.database(&mut *db);
        evm.env.block.basefee = base_fee;

        let tx_env = &mut evm.env.tx;
        tx_env.caller = Address::ZERO; // EntryPoint-like caller or generic?
        // EIP-7701/RIP-7560: Caller is the Protocol (0x0 or special address)
        // calling validateTransaction on 'sender'
        tx_env.transact_to = TransactTo::Call(tx.sender);

        // Function Selector: validateTransaction(bytes32,bytes)
        // Keccak256("validateTransaction(bytes32,bytes)")
        let selector = crate::types::keccak256(b"validateTransaction(bytes32,bytes)");
        let selector_bytes = &selector[0..4];

        // Prepare Arguments:
        // 1. txHash (32 bytes)
        // 2. signature (bytes) - Dynamic type

        let tx_enum = crate::types::Transaction::AA(Box::new(tx.clone()));
        let tx_hash = tx_enum.sighash();

        let mut data = Vec::new();
        data.extend_from_slice(selector_bytes);

        // -- Head --
        // Arg 1: txHash (32 bytes)
        data.extend_from_slice(&tx_hash.0);

        // Arg 2: Offset to signature (32 bytes)
        // Offset is calculated from the start of arguments (after selector).
        // Arg 1 is 32 bytes. Arg 2 (offset) is 32 bytes.
        // So dynamic data starts at 32 + 32 = 64 (0x40).
        data.extend_from_slice(&[0u8; 31]);
        data.push(0x40);

        // -- Tail --
        // Signature Length (32 bytes)
        let sig_len = U256::from(tx.signature.len());
        // U256 to bytes big endian
        data.extend_from_slice(&sig_len.to_be_bytes::<32>());

        // Signature Data
        data.extend_from_slice(&tx.signature);

        // Padding (round up to multiple of 32)
        let rem = tx.signature.len() % 32;
        if rem != 0 {
            let pad = 32 - rem;
            data.extend(std::iter::repeat_n(0, pad));
        }

        tx_env.data = crate::types::Bytes::from(data);

        tx_env.value = U256::ZERO;
        tx_env.gas_limit = 200_000; // Validation limit
        tx_env.gas_price = tx.max_fee_per_gas;
        tx_env.gas_priority_fee = Some(tx.max_priority_fee_per_gas);
        tx_env.nonce = Some(tx.nonce); // Validating THIS nonce

        let result_and_state = evm
            .transact()
            .map_err(|e| ExecutionError::Evm(format!("Validation Failed: {:?}", e)))?;

        match result_and_state.result {
            ExecutionResult::Success { .. } => Ok(()),
            ExecutionResult::Revert { output, .. } => Err(ExecutionError::Transaction(format!(
                "AA Validation Reverted: {:?}",
                output
            ))),
            ExecutionResult::Halt { reason, .. } => Err(ExecutionError::Transaction(format!(
                "AA Validation Halted: {:?}",
                reason
            ))),
        }
    }

    /// Execute a transaction ephemerally (no commit, for RPC 'call' and 'estimate_gas')
    pub fn execute_ephemeral(
        &self,
        caller: Address,
        to: Option<Address>,
        value: U256,
        data: crate::types::Bytes,
        gas_limit: u64,
        _access_list: Vec<crate::types::AccessListItem>, // Future proofing
    ) -> Result<(u64, Vec<u8>), ExecutionError> {
        let mut db = self.state.lock().unwrap();

        // Setup EVM
        let mut evm = EVM::new();
        evm.database(&mut *db);

        // Env setup
        let tx_env = &mut evm.env.tx;
        tx_env.caller = caller;
        tx_env.transact_to = if let Some(addr) = to {
            TransactTo::Call(addr)
        } else {
            TransactTo::Create(CreateScheme::Create)
        };
        tx_env.data = data;
        tx_env.value = value;
        tx_env.gas_limit = gas_limit;
        tx_env.gas_price = U256::ZERO; // Simulation usually 0 or free
        tx_env.gas_priority_fee = None;
        tx_env.nonce = None; // Ignore nonce for simulation

        // Execute
        let result_and_state = evm
            .transact()
            .map_err(|e| ExecutionError::Evm(format!("{:?}", e)))?;

        let result = result_and_state.result;

        match result {
            ExecutionResult::Success {
                gas_used, output, ..
            } => {
                let data = match output {
                    revm::primitives::Output::Call(b) => b.to_vec(),
                    revm::primitives::Output::Create(b, _) => b.to_vec(),
                };
                Ok((gas_used, data))
            }
            ExecutionResult::Revert { gas_used, output } => {
                // For 'call', we often want the revert data too.
                Ok((gas_used, output.to_vec()))
            }
            ExecutionResult::Halt { reason, .. } => {
                Err(ExecutionError::Evm(format!("Halted: {:?}", reason)))
            }
        }
    }
    fn process_equivocation_slashing(&self, block: &Block, db: &mut StateManager) {
        for evidence in &block.evidence {
            let v1 = &evidence.vote_a;
            let v2 = &evidence.vote_b;

            // 1. Verify Structure
            if v1.author != v2.author {
                log::warn!("Evidence Invalid: Different Authors");
                continue;
            }
            if v1.view != v2.view {
                log::warn!("Evidence Invalid: Different Views");
                continue;
            }
            if v1.block_hash == v2.block_hash {
                log::warn!("Evidence Invalid: Same Block Hash (Not equivocation)");
                continue;
            }

            // 2. Verify Signatures
            let a_valid = verify(&v1.author, &v1.block_hash.0, &v1.signature);
            let b_valid = verify(&v2.author, &v2.block_hash.0, &v2.signature);

            if !a_valid || !b_valid {
                log::warn!("Evidence Invalid: Bad Signatures");
                continue;
            }

            // 3. Slash!
            let offender = v1.author.clone();
            // Need Address from PublicKey
            let pk_bytes = offender.0.to_bytes();
            let hash = crate::types::keccak256(pk_bytes);
            let address = Address::from_slice(&hash[12..]);

            let slashed_amount = U256::from(1000u64); // Fixed Slash Amount

            if let Ok(Some(mut state)) = db.get_consensus_state() {
                if let Some(stake) = state.stakes.get_mut(&address) {
                    if *stake < slashed_amount {
                        *stake = U256::ZERO;
                    } else {
                        *stake -= slashed_amount;
                    }

                    log::warn!(
                        "Slashed Validator {:?} amount {:?}",
                        address,
                        slashed_amount
                    );

                    // 4. Remove from Committee if low stake
                    let min_stake = U256::from(2000u64);
                    if *stake < min_stake {
                        // Check Pending
                        if let Some(pos) = state
                            .pending_validators
                            .iter()
                            .position(|(pk, _)| *pk == offender)
                        {
                            state.pending_validators.remove(pos);
                            log::warn!(
                                "Validator Removed from Pending (Low Stake): {:?}",
                                offender
                            );
                        }
                        // Check Active
                        if let Some(pos) = state.committee.iter().position(|x| *x == offender) {
                            state.committee.remove(pos);
                            log::warn!(
                                "Validator Removed from Committee (Low Stake): {:?}",
                                offender
                            );
                        }
                    }
                    db.save_consensus_state(&state).unwrap();
                } else {
                    log::warn!(
                        "Validator {:?} has no stake entry found for address {:?}",
                        offender,
                        address
                    );
                }
            }
        }
    }

    fn process_liveness_slashing(&self, block: &Block, db: &mut StateManager) {
        if let Ok(Some(mut state)) = db.get_consensus_state() {
            let mut changed = false;

            // 1. Reward Current Leader (Author)
            if let Some(score) = state.inactivity_scores.get_mut(&block.author) {
                if *score > 0 {
                    *score -= 1;
                    changed = true;
                }
            } else {
                // Initialize if not present (optimization: only if we need to track?)
            }

            // 2. Penalize Failed Leader (if Timeout QC)
            let qc = &block.justify;
            if qc.block_hash == Hash::default() && qc.view > 0 {
                // Timeout detected for qc.view
                let committee_len = state.committee.len();
                if committee_len > 0 {
                    let failed_leader_idx = (qc.view as usize) % committee_len;
                    // Safety check index
                    if let Some(failed_leader) = state.committee.get(failed_leader_idx).cloned() {
                        log::warn!(
                            "Timeout QC for View {}. Penalizing Leader {:?}",
                            qc.view,
                            failed_leader
                        );

                        // Increment Score
                        let score = state
                            .inactivity_scores
                            .entry(failed_leader.clone())
                            .or_insert(0);
                        *score += 1;
                        let current_score = *score;
                        changed = true;

                        // Immediate Slash (Incremental)
                        let penalty = U256::from(10u64);
                        let pk_bytes = failed_leader.0.to_bytes();
                        let hash = crate::types::keccak256(pk_bytes);
                        let address = Address::from_slice(&hash[12..]);

                        if let Some(stake) = state.stakes.get_mut(&address) {
                            if *stake < penalty {
                                *stake = U256::ZERO;
                            } else {
                                *stake -= penalty;
                            }
                            changed = true;
                        } else {
                            log::warn!(
                                "Validator {:?} has no stake entry found for address {:?}",
                                failed_leader,
                                address
                            );
                        }

                        // Threshold Check
                        if current_score > 50 {
                            log::warn!(
                                "Validator {:?} exceeded inactivity threshold ({}). Removing from committee.",
                                failed_leader,
                                current_score
                            );
                            if let Some(pos) =
                                state.committee.iter().position(|x| *x == failed_leader)
                            {
                                state.committee.remove(pos);
                                // Reset score
                                state.inactivity_scores.remove(&failed_leader);
                                changed = true;
                            }
                        }
                    }
                }
            }

            if changed {
                db.save_consensus_state(&state).unwrap();
            }
        }
    }

    fn process_system_contract(
        &self,
        tx: &crate::types::LegacyTransaction,
        db: &mut StateManager,
        receipts: &mut Vec<crate::types::Receipt>,
        cumulative_gas_used: u64,
        view: u64,
    ) -> Result<(), ExecutionError> {
        // System Contract Call
        log::info!(
            "System Contract Call detected from {:?}",
            crate::types::Transaction::Legacy(Box::new(tx.clone())).sender()
        );

        // Simple Gas/Nonce deduction (Simulated for MVP)
        // Wraps in Enum to use helper or compute manually.
        let tx_enum = crate::types::Transaction::Legacy(Box::new(tx.clone()));
        let _sender_acc = db.basic(tx_enum.sender()).unwrap().unwrap();
        // if sender_acc.balance < tx.value {
        //     // + fee in real impl
        //     return Err(ExecutionError::Transaction("Insufficient Balance".into()));
        // }

        // Simulate cost deduction if needed, or remove.
        // For MVP we just log.
        let _cost = tx.max_fee_per_gas; // Field access for Legacy Tx struct
        if tx.data.len() >= 4 {
            let selector = &tx.data[0..4];
            match selector {
                // stake() -> 0x3a4b66f1
                [0x3a, 0x4b, 0x66, 0xf1] => {
                    let min_stake = U256::from(2000u64); // Threshold
                    if tx.value < min_stake {
                        log::error!("Stake too low: {:?}", tx.value);
                    } else if let Ok(Some(mut state)) = db.get_consensus_state() {
                        let sender_pk = tx.public_key.clone();

                        // 1. Lock Funds
                        let current_stake = *state
                            .stakes
                            .get(&crate::types::Transaction::Legacy(Box::new(tx.clone())).sender())
                            .unwrap_or(&U256::ZERO);
                        state.stakes.insert(
                            crate::types::Transaction::Legacy(Box::new(tx.clone())).sender(),
                            current_stake + tx.value,
                        );
                        // 2. Add to Pending
                        // Calculate activation view = current + 10 (epoch length)
                        let activation_view = view + 10;
                        state.pending_validators.push((sender_pk, activation_view));

                        db.save_consensus_state(&state).unwrap();
                        log::info!("Staked {:?} for validator", tx.value);
                    }
                }
                // unstake() -> 0x2e17de78
                [0x2e, 0x17, 0xde, 0x78] => {
                    if let Ok(Some(mut state)) = db.get_consensus_state() {
                        let sender_addr =
                            crate::types::Transaction::Legacy(Box::new(tx.clone())).sender();
                        if let Some(pos) = state.committee.iter().position(|pk| {
                            crate::types::keccak256(pk.0.to_bytes())[12..] == sender_addr.0
                        }) {
                            let pk = state.committee[pos].clone();
                            let exit_view = view + 10;
                            state.exiting_validators.push((pk, exit_view));
                            db.save_consensus_state(&state).unwrap();
                            log::info!("Validator Unstaked/Exiting: {:?}", sender_addr);
                        } else {
                            log::warn!("Unstake failed: Not in committee");
                        }
                    }
                }
                // withdraw() -> 0x3ccfd60b
                [0x3c, 0xcf, 0xd6, 0x0b] => {
                    if let Ok(Some(mut state)) = db.get_consensus_state() {
                        let sender_pk = tx.public_key.clone();
                        let sender_addr =
                            crate::types::Transaction::Legacy(Box::new(tx.clone())).sender();

                        let is_active = state.committee.contains(&sender_pk);
                        let is_pending = state
                            .pending_validators
                            .iter()
                            .any(|(pk, _)| *pk == sender_pk);
                        let is_exiting = state
                            .exiting_validators
                            .iter()
                            .any(|(pk, _)| *pk == sender_pk);

                        #[allow(clippy::collapsible_if)]
                        if let Some(stake) = state.stakes.get(&sender_addr).cloned() {
                            if !is_active && !is_pending && !is_exiting && stake > U256::ZERO {
                                // Refund
                                state.stakes.insert(sender_addr, U256::ZERO);
                                db.save_consensus_state(&state).unwrap();

                                // Credit Balance
                                let mut acc = db.basic(sender_addr).unwrap().unwrap_or_default();
                                acc.balance += stake;

                                let new_info = crate::storage::AccountInfo {
                                    nonce: acc.nonce,
                                    balance: acc.balance,
                                    code_hash: Hash(acc.code_hash.0),
                                    code: acc.code.map(|c| c.original_bytes()),
                                };
                                db.commit_account(sender_addr, new_info).unwrap();

                                log::info!("Withdrawn Stake: {:?} for {:?}", stake, sender_addr);
                            }
                        }
                    }
                }
                _ => {
                    log::warn!("Unknown System Contract Function");
                }
            }
        }

        // Skip EVM Execution for this Tx, but record receipt?
        // Deduct Balance manually
        let updated_acc = db
            .basic(crate::types::Transaction::Legacy(Box::new(tx.clone())).sender())
            .unwrap()
            .unwrap_or_default();

        let new_info = crate::storage::AccountInfo {
            nonce: updated_acc.nonce + 1,
            balance: updated_acc.balance - tx.value,
            code_hash: Hash(updated_acc.code_hash.0),
            code: updated_acc.code.map(|c| c.original_bytes()),
        };
        db.commit_account(
            crate::types::Transaction::Legacy(Box::new(tx.clone())).sender(),
            new_info,
        )
        .unwrap();

        // Push Receipt
        receipts.push(crate::types::Receipt {
            status: 1,
            cumulative_gas_used,
            logs: vec![],
        });

        Ok(())
    }
}
