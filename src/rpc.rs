use crate::crypto::Hash;
use crate::storage::{ConsensusState, Storage};
use crate::types::Block;
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
}

pub struct OckhamRpcImpl {
    storage: Arc<dyn Storage>,
}

impl OckhamRpcImpl {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
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
}
