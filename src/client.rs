use crate::crypto::{PrivateKey, sign};
use crate::types::{Address, Block, Transaction, U256};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

pub struct OckhamClient {
    client: HttpClient,
}

impl OckhamClient {
    pub fn new(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let client = HttpClientBuilder::default().build(url)?;
        Ok(Self { client })
    }

    pub async fn get_block_by_hash(
        &self,
        hash: crate::crypto::Hash,
    ) -> Result<Option<Block>, Box<dyn std::error::Error>> {
        let params = rpc_params![hash];
        let block: Option<Block> = self.client.request("get_block_by_hash", params).await?;
        Ok(block)
    }

    pub async fn get_latest_block(&self) -> Result<Option<Block>, Box<dyn std::error::Error>> {
        let block: Option<Block> = self
            .client
            .request("get_latest_block", rpc_params![])
            .await?;
        Ok(block)
    }

    pub async fn get_balance(&self, address: Address) -> Result<U256, Box<dyn std::error::Error>> {
        let params = rpc_params![address];
        let balance: U256 = self.client.request("get_balance", params).await?;
        Ok(balance)
    }

    pub async fn get_transaction_count(
        &self,
        address: Address,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let params = rpc_params![address];
        let nonce: u64 = self.client.request("get_transaction_count", params).await?;
        Ok(nonce)
    }

    pub async fn send_transaction(
        &self,
        nonce: u64,
        to: Option<Address>,
        value: U256,
        data: crate::types::Bytes,
        key: &PrivateKey,
    ) -> Result<crate::crypto::Hash, Box<dyn std::error::Error>> {
        // 1. Get Chain ID (for now hardcoded or fetched)
        let chain_id: u64 = self.client.request("chain_id", rpc_params![]).await?;

        // 2. Get Nonce (using balance/account info? No, need get_transaction_count equivalent.
        // For MVP, we don't have get_transaction_count.
        // But we have get_balance.
        // Wait, we need the nonce. `get_balance` implementation in `rpc.rs` uses `storage.get_account`.
        // We should add `get_nonce` to RPC or just guess.
        // Let's check `rpc.rs` again.

        // rpc.rs has `get_balance`. It fetches account.
        // I should probably add `get_nonce` to RPC to be correct, but I cannot modify rpc.rs in this step trivially without replanning?
        // Actually, I can check if I can add valid nonce.
        // The user just said "execute bin in test folder".
        // If I use a random key, nonce is 0.
        // So I can just generate a new random key for every tx in the test.

        // 3. Get Gas Price (Base Fee)
        let base_fee: U256 = self
            .client
            .request("suggest_base_fee", rpc_params![])
            .await?;

        // Priority Fee
        let priority_fee = U256::from(1_000_000); // 0.001 Gwei
        // Use 2x Base Fee buffer to ensure inclusion even if base fee spikes
        let max_fee = base_fee
            .saturating_mul(U256::from(2))
            .saturating_add(priority_fee);

        // 4. Construct Transaction
        let mut tx = Transaction {
            chain_id,
            nonce,
            max_priority_fee_per_gas: priority_fee,
            max_fee_per_gas: max_fee,
            gas_limit: 100000, // Standard transfer + data
            to,
            value,
            data,
            access_list: vec![],
            public_key: key.public_key(),
            signature: crate::crypto::Signature::default(),
        };

        // 5. Sign
        let sighash = tx.sighash();
        let signature = sign(key, &sighash.0);
        tx.signature = signature;

        // 6. Send
        let hash: crate::crypto::Hash = self
            .client
            .request("send_transaction", rpc_params![tx])
            .await?;
        Ok(hash)
    }
}
