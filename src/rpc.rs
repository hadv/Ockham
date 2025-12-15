use crate::crypto::Hash;
use crate::storage::{ConsensusState, Storage};
use crate::tx_pool::TxPool;
use crate::types::{Address, Block, Transaction, U256};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use std::sync::Arc;

#[rpc(server)]
pub trait OckhamRpc {
    #[method(name = "get_block_by_hash")]
    fn get_block_by_hash(&self, hash: Hash) -> RpcResult<Option<Block>>;

    #[method(name = "get_latest_block")]
    fn get_latest_block(&self) -> RpcResult<Option<Block>>;

    #[method(name = "get_status")]
    fn get_status(&self) -> RpcResult<Option<ConsensusState>>;

    #[method(name = "send_transaction")]
    fn send_transaction(&self, tx: Transaction) -> RpcResult<Hash>;

    #[method(name = "get_balance")]
    fn get_balance(&self, address: Address) -> RpcResult<U256>;

    #[method(name = "chain_id")]
    fn chain_id(&self) -> RpcResult<u64>;

    #[method(name = "suggest_base_fee")]
    fn suggest_base_fee(&self) -> RpcResult<U256>;
}

pub struct OckhamRpcImpl {
    storage: Arc<dyn Storage>,
    tx_pool: Arc<TxPool>,
    block_gas_limit: u64,
}

impl OckhamRpcImpl {
    pub fn new(storage: Arc<dyn Storage>, tx_pool: Arc<TxPool>, block_gas_limit: u64) -> Self {
        Self {
            storage,
            tx_pool,
            block_gas_limit,
        }
    }
}

#[async_trait]
impl OckhamRpcServer for OckhamRpcImpl {
    fn get_block_by_hash(&self, hash: Hash) -> RpcResult<Option<Block>> {
        let block = self.storage.get_block(&hash).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage error: {:?}", e),
                None::<()>,
            )
        })?;
        Ok(block)
    }

    fn get_latest_block(&self) -> RpcResult<Option<Block>> {
        let state = self.storage.get_consensus_state().map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage error: {:?}", e),
                None::<()>,
            )
        })?;

        if let Some(s) = state {
            let block = self.storage.get_block(&s.preferred_block).map_err(|e| {
                jsonrpsee::types::ErrorObject::owned(
                    -32000,
                    format!("Storage error: {:?}", e),
                    None::<()>,
                )
            })?;
            Ok(block)
        } else {
            Ok(None)
        }
    }

    fn get_status(&self) -> RpcResult<Option<ConsensusState>> {
        let state = self.storage.get_consensus_state().map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage error: {:?}", e),
                None::<()>,
            )
        })?;
        Ok(state)
    }

    fn send_transaction(&self, tx: Transaction) -> RpcResult<Hash> {
        let hash = crate::crypto::hash_data(&tx);
        // Validate? (TxPool does some validation)
        self.tx_pool.add_transaction(tx).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("TxPool error: {:?}", e),
                None::<()>,
            )
        })?;
        Ok(hash)
    }

    fn get_balance(&self, address: Address) -> RpcResult<U256> {
        let account = self.storage.get_account(&address).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage error: {:?}", e),
                None::<()>,
            )
        })?;

        Ok(account.map(|a| a.balance).unwrap_or_default())
    }

    fn chain_id(&self) -> RpcResult<u64> {
        Ok(1337) // TODO: Config
    }

    fn suggest_base_fee(&self) -> RpcResult<U256> {
        // Get the latest block (preferred block in consensus)
        let state = self.storage.get_consensus_state().map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage error: {:?}", e),
                None::<()>,
            )
        })?;

        let Some(s) = state else {
            return Ok(U256::from(crate::types::INITIAL_BASE_FEE));
        };

        let block = match self.storage.get_block(&s.preferred_block) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(U256::from(crate::types::INITIAL_BASE_FEE)),
            Err(e) => {
                return Err(jsonrpsee::types::ErrorObject::owned(
                    -32000,
                    format!("Storage error: {:?}", e),
                    None::<()>,
                ));
            }
        };

        // Logic mirror from consensus.rs
        let elasticity_multiplier = 2;
        let base_fee_max_change_denominator = 8;
        let target_gas = self.block_gas_limit / elasticity_multiplier;

        let parent_gas_used = block.gas_used;
        let parent_base_fee = block.base_fee_per_gas;

        if parent_gas_used == target_gas {
            Ok(parent_base_fee)
        } else if parent_gas_used > target_gas {
            let gas_used_delta = parent_gas_used - target_gas;
            let base_fee_increase = parent_base_fee * U256::from(gas_used_delta)
                / U256::from(target_gas)
                / U256::from(base_fee_max_change_denominator);
            Ok(parent_base_fee + base_fee_increase)
        } else {
            let gas_used_delta = target_gas - parent_gas_used;
            let base_fee_decrease = parent_base_fee * U256::from(gas_used_delta)
                / U256::from(target_gas)
                / U256::from(base_fee_max_change_denominator);
            Ok(parent_base_fee.saturating_sub(base_fee_decrease))
        }
    }
}
