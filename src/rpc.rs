use crate::crypto::Hash;
use crate::storage::{ConsensusState, Storage};
use crate::tx_pool::TxPool;
use crate::types::{Address, Block, Transaction, U256};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CallRequest {
    pub from: Option<Address>,
    pub to: Option<Address>,
    pub gas: Option<u64>,
    pub gas_price: Option<U256>,
    pub value: Option<U256>,
    pub data: Option<crate::types::Bytes>,
}
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

    #[method(name = "get_transaction_count")]
    fn get_transaction_count(&self, address: Address) -> RpcResult<u64>;

    #[method(name = "chain_id")]
    fn chain_id(&self) -> RpcResult<u64>;

    #[method(name = "suggest_base_fee")]
    fn suggest_base_fee(&self) -> RpcResult<U256>;

    #[method(name = "call")]
    fn call(&self, request: CallRequest, _block: Option<String>) -> RpcResult<crate::types::Bytes>;

    #[method(name = "estimate_gas")]
    fn estimate_gas(&self, request: CallRequest, _block: Option<String>) -> RpcResult<u64>;

    #[method(name = "get_code")]
    fn get_code(&self, address: Address, _block: Option<String>) -> RpcResult<crate::types::Bytes>;

    #[method(name = "get_block_by_number")]
    fn get_block_by_number(&self, number: String) -> RpcResult<Option<Block>>;
}

pub struct OckhamRpcImpl {
    storage: Arc<dyn Storage>,
    tx_pool: Arc<TxPool>,
    executor: crate::vm::Executor,
    block_gas_limit: u64,
    broadcast_sender: tokio::sync::mpsc::Sender<Transaction>,
}

impl OckhamRpcImpl {
    pub fn new(
        storage: Arc<dyn Storage>,
        tx_pool: Arc<TxPool>,
        executor: crate::vm::Executor,
        block_gas_limit: u64,
        broadcast_sender: tokio::sync::mpsc::Sender<Transaction>,
    ) -> Self {
        Self {
            storage,
            tx_pool,
            executor,
            block_gas_limit,
            broadcast_sender,
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
        self.tx_pool.add_transaction(tx.clone()).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("TxPool error: {:?}", e),
                None::<()>,
            )
        })?;

        // Broadcast
        let sender = self.broadcast_sender.clone();
        tokio::spawn(async move {
            let _ = sender.send(tx).await;
        });

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

    fn get_transaction_count(&self, address: Address) -> RpcResult<u64> {
        let account = self.storage.get_account(&address).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage error: {:?}", e),
                None::<()>,
            )
        })?;

        Ok(account.map(|a| a.nonce).unwrap_or_default())
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

    fn call(&self, request: CallRequest, _block: Option<String>) -> RpcResult<crate::types::Bytes> {
        let caller = request.from.unwrap_or_default();
        let value = request.value.unwrap_or_default();
        let data = request.data.unwrap_or_default();
        let gas = request.gas.unwrap_or(self.block_gas_limit);

        let (_, output) = self
            .executor
            .execute_ephemeral(caller, request.to, value, data, gas, vec![])
            .map_err(|e| {
                jsonrpsee::types::ErrorObject::owned(
                    -32000,
                    format!("Execution Error: {:?}", e),
                    None::<()>,
                )
            })?;

        Ok(crate::types::Bytes::from(output))
    }

    fn estimate_gas(&self, request: CallRequest, _block: Option<String>) -> RpcResult<u64> {
        let caller = request.from.unwrap_or_default();
        let value = request.value.unwrap_or_default();
        let data = request.data.unwrap_or_default();
        let gas = request.gas.unwrap_or(self.block_gas_limit);

        let (gas_used, _) = self
            .executor
            .execute_ephemeral(caller, request.to, value, data, gas, vec![])
            .map_err(|e| {
                jsonrpsee::types::ErrorObject::owned(
                    -32000,
                    format!("Execution Error: {:?}", e),
                    None::<()>,
                )
            })?;

        Ok(gas_used)
    }

    fn get_code(&self, address: Address, _block: Option<String>) -> RpcResult<crate::types::Bytes> {
        let account = self.storage.get_account(&address).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(
                -32000,
                format!("Storage Error: {:?}", e),
                None::<()>,
            )
        })?;

        if let Some(info) = account {
            if let Some(code) = info.code {
                Ok(code)
            } else if info.code_hash != Hash::default() {
                let code = self
                    .storage
                    .get_code(&info.code_hash)
                    .map_err(|e| {
                        jsonrpsee::types::ErrorObject::owned(
                            -32000,
                            format!("Storage Error: {:?}", e),
                            None::<()>,
                        )
                    })?
                    .unwrap_or_default();
                Ok(code)
            } else {
                Ok(crate::types::Bytes::default())
            }
        } else {
            Ok(crate::types::Bytes::default())
        }
    }

    fn get_block_by_number(&self, number: String) -> RpcResult<Option<Block>> {
        let view = if number == "latest" {
            if let Some(state) = self.storage.get_consensus_state().unwrap_or(None) {
                state.preferred_view
            } else {
                return Ok(None);
            }
        } else if let Some(stripped) = number.strip_prefix("0x") {
            u64::from_str_radix(stripped, 16).unwrap_or(0)
        } else {
            number.parse::<u64>().unwrap_or(0)
        };

        if let Some(qc) = self.storage.get_qc(view).map_err(|e| {
            jsonrpsee::types::ErrorObject::owned(-32000, format!("{:?}", e), None::<()>)
        })? {
            let block = self.storage.get_block(&qc.block_hash).map_err(|e| {
                jsonrpsee::types::ErrorObject::owned(-32000, format!("{:?}", e), None::<()>)
            })?;
            Ok(block)
        } else {
            Ok(None)
        }
    }
}
