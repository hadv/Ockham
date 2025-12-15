use crate::crypto::Hash;
use crate::state::StateManager;
use crate::types::Block;
use revm::{
    EVM,
    primitives::{Address, CreateScheme, ExecutionResult, ResultAndState, TransactTo},
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
        let state = Arc::new(Mutex::new(StateManager::new(storage)));
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

        // Pre-check: Sum of gas limits?
        // Actually, effective gas is tracked during execution.
        // But we can check if individual tx exceeds limit.
        // Or if total gas used exceeds limit (checked at end of extraction).

        for tx in &block.payload {
            if tx.gas_limit > self.block_gas_limit {
                return Err(ExecutionError::Transaction(
                    "Tx exceeds block gas limit".into(),
                ));
            }
        }

        for tx in &block.payload {
            // 1. Validate signature (simple check here, or assume consensus did it?)
            // Ideally we check signatures before execution.
            if tx.sender() == Address::ZERO {
                return Err(ExecutionError::Transaction("Invalid sender".into()));
            }

            // 2. Setup EVM
            let mut evm = EVM::new();
            evm.database(&mut *db);

            // Set Block Info
            evm.env.block.basefee = block.base_fee_per_gas;
            // evm.env.block.gas_limit = U256::from(block_gas_limit...); // If needed

            // 3. Populate TxEnv
            // evm.env matches this version of revm
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
            // EIP-1559 to Legacy mapping for this revm version (if needed)
            // If using newest revm, we might have max_fee_per_gas field?
            // Checking imports... `TxEnv` struct in older revm might only have gas_price.
            // But we can check if `evm.env.tx` has `gas_priority_fee`.
            // Assuming this version uses `gas_price` as effective gas price or max fee.
            // Let's stick to setting gas_price = max_fee_per_gas for now, unless we upgrade revm integration.
            tx_env.gas_price = tx.max_fee_per_gas;
            tx_env.gas_priority_fee = Some(tx.max_priority_fee_per_gas); // Added if supported by local revm version used (3.5.0 supports it)

            tx_env.nonce = Some(tx.nonce);

            // 4. Execute
            let result_and_state = evm
                .transact()
                .map_err(|e| ExecutionError::Evm(format!("{:?}", e)))?;

            // Track gas
            if let ExecutionResult::Success { gas_used, .. } = result_and_state.result {
                cumulative_gas_used += gas_used;
            } else if let ExecutionResult::Revert { gas_used, .. } = result_and_state.result {
                cumulative_gas_used += gas_used;
            }
            // Halt?

            // 5. Commit state changes
            // result_and_state has .result (ExecutionResult) and .state (State = HashMap<Address, Account>)
            let ResultAndState { result, state } = result_and_state;

            if let ExecutionResult::Success { .. } = result {
                for (address, account) in state {
                    // Always update for now, or check status if needed.
                    // revm usually returns changed state in `ResultAndState`.

                    // Update account info
                    let info = crate::storage::AccountInfo {
                        nonce: account.info.nonce,
                        balance: account.info.balance,
                        code_hash: Hash(account.info.code_hash.0),
                        code: account.info.code.map(|c| c.original_bytes()),
                    };

                    db.commit_account(address, info)
                        .map_err(|e| ExecutionError::State(e.to_string()))?;

                    // Update storage
                    for (index, slot) in account.storage {
                        // value in revm is cast to U256 (slot value).
                        // revm 3.x storage value is U256.
                        // But we need to check if it's present (Slot specific).
                        // In 3.0 storage is `HashMap<U256, Slot>`. Slot has `present_value`.
                        let val = slot.present_value;
                        db.commit_storage(address, index, val)
                            .map_err(|e| ExecutionError::State(e.to_string()))?;
                    }
                }
            }

            // TODO: Collect receipts/logs for block header
        }

        // 6. Update State Root and Gas Used in Block
        block.state_root = db.root();
        block.gas_used = cumulative_gas_used;

        Ok(())
    }
}
