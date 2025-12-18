use crate::crypto::Hash;
use crate::state::StateManager;
use crate::types::Block;
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
            let a_valid = crate::crypto::verify(&v1.author, &v1.block_hash.0, &v1.signature);
            let b_valid = crate::crypto::verify(&v2.author, &v2.block_hash.0, &v2.signature);

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

            let mut slashed_amount = U256::from(1000u64); // Fixed Slash Amount

            if let Some(mut info) = db.basic(address).unwrap() {
                if info.balance < slashed_amount {
                    slashed_amount = info.balance; // Burn all
                }
                info.balance -= slashed_amount;

                // Commit Balance Update
                let new_info = crate::storage::AccountInfo {
                    nonce: info.nonce,
                    balance: info.balance,
                    code_hash: Hash(info.code_hash.0), // Revm to Internal Hash
                    code: info.code.map(|c| c.original_bytes()),
                };
                db.commit_account(address, new_info).unwrap();
                log::warn!(
                    "Slashed Validator {:?} amount {:?}",
                    address,
                    slashed_amount
                );

                // 4. Remove from Committee if low balance (Force Remove)
                let min_stake = U256::from(2000u64);
                #[allow(clippy::collapsible_if)]
                if info.balance < min_stake {
                    if let Ok(Some(mut state)) = db.get_consensus_state() {
                        // Check Pending
                        if let Some(pos) = state
                            .pending_validators
                            .iter()
                            .position(|(pk, _)| *pk == offender)
                        {
                            state.pending_validators.remove(pos);
                            // Also refund stake if any?
                            // Logic: validator must maintain min_stake to stay pending.
                            log::warn!(
                                "Validator Removed from Pending (Low Stake): {:?}",
                                offender
                            );
                        }
                        // Check Active
                        if let Some(pos) = state.committee.iter().position(|x| *x == offender) {
                            // Trigger Exit?
                            // For simplicity, just remove from committee now?
                            // Ideally should be "Exiting" state.
                            state.committee.remove(pos);
                            log::warn!(
                                "Validator Removed from Committee (Low Stake): {:?}",
                                offender
                            );
                        }
                        // Check Exiting (Already leaving, but maybe accelerate?)
                        // No need, just let them exit.
                        db.save_consensus_state(&state).unwrap();
                    }
                }
            }
        }

        for tx in &block.payload {
            if tx.gas_limit > self.block_gas_limit {
                return Err(ExecutionError::Transaction(
                    "Tx exceeds block gas limit".into(),
                ));
            }
        }

        let mut receipts = Vec::with_capacity(block.payload.len());

        for (i, tx) in block.payload.iter().enumerate() {
            // 1. Validate signature (simple check here, or assume consensus did it?)
            if tx.sender() == Address::ZERO {
                return Err(ExecutionError::Transaction("Invalid sender".into()));
            }

            // 2. Setup EVM
            let mut evm = EVM::new();
            evm.database(&mut *db);

            // SYSTEM CONTRACT INTERCEPTION (Address 0x1000)
            let sys_contract = Address::from_slice(
                &hex::decode("0000000000000000000000000000000000001000").unwrap(),
            );

            if tx.to == Some(sys_contract) {
                // System Contract Call
                log::info!("System Contract Call detected from {:?}", tx.sender());

                // Simple Gas/Nonce deduction (Simulated for MVP)
                let sender_acc = db.basic(tx.sender()).unwrap().unwrap();
                if sender_acc.balance < tx.value {
                    // + fee in real impl
                    return Err(ExecutionError::Transaction("Insufficient Balance".into()));
                }

                // Decode Selector
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
                                let current_stake =
                                    *state.stakes.get(&tx.sender()).unwrap_or(&U256::ZERO);
                                state.stakes.insert(tx.sender(), current_stake + tx.value);

                                // 2. Add to Pending (if not already active/pending)
                                let is_active = state.committee.contains(&sender_pk);
                                let is_pending = state
                                    .pending_validators
                                    .iter()
                                    .any(|(pk, _)| *pk == sender_pk);

                                if !is_active && !is_pending {
                                    let activation_view = block.view + 10; // Delay 10
                                    state
                                        .pending_validators
                                        .push((sender_pk.clone(), activation_view));
                                    log::info!(
                                        "Validator Pending: {:?} until view {}",
                                        sender_pk,
                                        activation_view
                                    );
                                }
                                db.save_consensus_state(&state).unwrap();
                            }
                        }
                        // unstake() -> 0x2e17de78
                        [0x2e, 0x17, 0xde, 0x78] => {
                            if let Ok(Some(mut state)) = db.get_consensus_state() {
                                let sender_pk = tx.public_key.clone();

                                // Must be Active to Unstake
                                if state.committee.contains(&sender_pk) {
                                    // Schedule Exit
                                    let exit_view = block.view + 10; // Delay 10
                                    state
                                        .exiting_validators
                                        .push((sender_pk.clone(), exit_view));
                                    log::info!(
                                        "Validator Exiting: {:?} at view {}",
                                        sender_pk,
                                        exit_view
                                    );
                                    db.save_consensus_state(&state).unwrap();
                                }
                            }
                        }
                        // withdraw() -> 0x3ccfd60b
                        [0x3c, 0xcf, 0xd6, 0x0b] => {
                            if let Ok(Some(mut state)) = db.get_consensus_state() {
                                let sender_pk = tx.public_key.clone();
                                let sender_addr = tx.sender();

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
                                    if !is_active
                                        && !is_pending
                                        && !is_exiting
                                        && stake > U256::ZERO
                                    {
                                        // Refund
                                        state.stakes.insert(sender_addr, U256::ZERO);
                                        db.save_consensus_state(&state).unwrap();

                                        // Credit Balance
                                        let mut acc =
                                            db.basic(sender_addr).unwrap().unwrap_or_default();
                                        acc.balance += stake;

                                        let new_info = crate::storage::AccountInfo {
                                            nonce: acc.nonce,
                                            balance: acc.balance,
                                            code_hash: Hash(acc.code_hash.0),
                                            code: acc.code.map(|c| c.original_bytes()),
                                        };
                                        db.commit_account(sender_addr, new_info).unwrap();

                                        log::info!(
                                            "Withdrawn Stake: {:?} for {:?}",
                                            stake,
                                            sender_addr
                                        );
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
                // CRITICAL FIX: Reload account info because it might have been modified by the System Contract Logic (e.g. withdraw refund)
                let updated_acc = db.basic(tx.sender()).unwrap().unwrap_or_default();

                let new_info = crate::storage::AccountInfo {
                    nonce: updated_acc.nonce + 1,
                    balance: updated_acc.balance - tx.value,
                    code_hash: Hash(updated_acc.code_hash.0),
                    code: updated_acc.code.map(|c| c.original_bytes()),
                };
                db.commit_account(tx.sender(), new_info).unwrap();

                // Credit 0x1000? (Optional, burn is fine for now or lock)

                // Push Receipt
                receipts.push(crate::types::Receipt {
                    status: 1,
                    cumulative_gas_used,
                    logs: vec![],
                });

                continue; // Skip standard EVM
            }

            // Set Block Info
            evm.env.block.basefee = block.base_fee_per_gas;

            // 3. Populate TxEnv
            let tx_env = &mut evm.env.tx;
            tx_env.caller = tx.sender();
            tx_env.transact_to = if let Some(to) = tx.to {
                TransactTo::Call(to)
            } else {
                TransactTo::Create(CreateScheme::Create)
            };
            tx_env.data = tx.data.clone();
            tx_env.value = tx.value;
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = tx.max_fee_per_gas;
            tx_env.gas_priority_fee = Some(tx.max_priority_fee_per_gas);
            tx_env.nonce = Some(tx.nonce);

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
}
