use crate::crypto::{Hash, PublicKey, Signature};
pub use alloy_primitives::{Address, Bytes, FixedBytes, U256, keccak256};
use serde::{Deserialize, Serialize};

/// The View number definition (u64).
pub type View = u64;

pub const DEFAULT_BLOCK_GAS_LIMIT: u64 = 30_000_000;
pub const MAX_TX_GAS_LIMIT: u64 = 16_777_216; // 2^24 (Fusaka EIP-7825)
pub const INITIAL_BASE_FEE: u64 = 10_000_000; // 0.01 Gwei

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessListItem {
    pub address: Address,
    pub storage_keys: Vec<U256>,
}

/// EIP-1559 style Legacy Transaction
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyTransaction {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: U256,
    pub max_fee_per_gas: U256,
    pub gas_limit: u64,
    pub to: Option<Address>, // None for contract creation
    pub value: U256,
    pub data: Bytes,
    pub access_list: Vec<AccessListItem>,
    pub public_key: PublicKey,
    pub signature: Signature,
}

/// Native Account Abstraction Transaction (EIP-7701 / RIP-7560)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AATransaction {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: U256,
    pub max_fee_per_gas: U256,
    pub gas_limit: u64,
    pub sender: Address, // Smart Contract Account
    pub data: Bytes,     // Execution calldata
    pub paymaster: Option<Address>,
    pub signature: Bytes, // Flexible auth data
    pub builder_fee: U256,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Transaction {
    Legacy(Box<LegacyTransaction>),
    AA(Box<AATransaction>),
}

impl Transaction {
    pub fn sender(&self) -> Address {
        match self {
            Transaction::Legacy(tx) => {
                let pk_bytes = tx.public_key.0.to_bytes();
                let hash = keccak256(pk_bytes);
                Address::from_slice(&hash[12..])
            }
            Transaction::AA(tx) => tx.sender,
        }
    }

    pub fn nonce(&self) -> u64 {
        match self {
            Transaction::Legacy(tx) => tx.nonce,
            Transaction::AA(tx) => tx.nonce,
        }
    }

    pub fn gas_limit(&self) -> u64 {
        match self {
            Transaction::Legacy(tx) => tx.gas_limit,
            Transaction::AA(tx) => tx.gas_limit,
        }
    }

    pub fn max_fee_per_gas(&self) -> U256 {
        match self {
            Transaction::Legacy(tx) => tx.max_fee_per_gas,
            Transaction::AA(tx) => tx.max_fee_per_gas,
        }
    }

    pub fn max_priority_fee_per_gas(&self) -> U256 {
        match self {
            Transaction::Legacy(tx) => tx.max_priority_fee_per_gas,
            Transaction::AA(tx) => tx.max_priority_fee_per_gas,
        }
    }

    pub fn value(&self) -> U256 {
        match self {
            Transaction::Legacy(tx) => tx.value,
            Transaction::AA(_) => U256::ZERO, // AA txs usually call with value 0, value transfer is internal
        }
    }

    pub fn data(&self) -> &Bytes {
        match self {
            Transaction::Legacy(tx) => &tx.data,
            Transaction::AA(tx) => &tx.data,
        }
    }

    pub fn to(&self) -> Option<Address> {
        match self {
            Transaction::Legacy(tx) => tx.to,
            Transaction::AA(tx) => Some(tx.sender), // AA tx "to" is the account itself for loopback/auth
        }
    }

    // Helper for direct access to check if it is contract creation
    pub fn is_create(&self) -> bool {
        match self {
            Transaction::Legacy(tx) => tx.to.is_none(),
            Transaction::AA(_) => false, // AA cannot directly deploy via "to=null" (usually via factory)
        }
    }

    // Helper for VM execution target
    pub fn target(&self) -> Option<Address> {
        self.to()
    }

    pub fn sighash(&self) -> Hash {
        match self {
            Transaction::Legacy(tx) => {
                let data = (
                    tx.chain_id,
                    tx.nonce,
                    &tx.max_priority_fee_per_gas,
                    &tx.max_fee_per_gas,
                    tx.gas_limit,
                    &tx.to,
                    &tx.value,
                    &tx.data,
                    &tx.access_list,
                );
                crate::crypto::hash_data(&data)
            }
            Transaction::AA(tx) => {
                // AA SigHash excludes signature
                let data = (
                    tx.chain_id,
                    tx.nonce,
                    &tx.max_priority_fee_per_gas,
                    &tx.max_fee_per_gas,
                    tx.gas_limit,
                    &tx.sender,
                    &tx.data,
                    &tx.paymaster,
                    &tx.builder_fee,
                );
                crate::crypto::hash_data(&data)
            }
        }
    }
}

/// A Block in the Simplex chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub author: PublicKey,
    pub view: View,
    pub parent_hash: Hash,
    pub justify: QuorumCertificate, // The QC that justifies this block (usually for parent)
    pub state_root: Hash,           // Global State Root after execution
    pub receipts_root: Hash,        // Merkle root of transaction receipts
    pub payload: Vec<Transaction>,  // Transactions
    pub is_dummy: bool,             // Simplex specific: Dummy blocks for timeout

    // EIP-1559
    pub base_fee_per_gas: U256,
    pub gas_used: u64,

    // On-Chain Committee
    pub evidence: Vec<EquivocationEvidence>,
    pub committee_hash: Hash, // Hash of the active committee for this view
}

impl Block {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        author: PublicKey,
        view: View,
        parent_hash: Hash,
        justify: QuorumCertificate,
        state_root: Hash,
        receipts_root: Hash,
        payload: Vec<Transaction>,
        base_fee_per_gas: U256,
        gas_used: u64,
        evidence: Vec<EquivocationEvidence>,
        committee_hash: Hash,
    ) -> Self {
        Self {
            author,
            view,
            parent_hash,
            justify,
            state_root,
            receipts_root,
            payload,
            is_dummy: false,
            base_fee_per_gas,
            gas_used,
            evidence,
            committee_hash,
        }
    }

    pub fn new_dummy(
        author: PublicKey,
        view: View,
        parent_hash: Hash,
        justify: QuorumCertificate,
    ) -> Self {
        Self {
            author,
            view,
            parent_hash,
            justify,
            state_root: Hash::default(),
            receipts_root: Hash::default(),
            payload: vec![],
            is_dummy: true,
            base_fee_per_gas: U256::from(INITIAL_BASE_FEE), // Default base fee for dummy
            gas_used: 0,
            evidence: vec![],
            committee_hash: Hash::default(),
        }
    }
}

/// Type of vote: Notarize (for block validity) or Finalize (for view completeness)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum VoteType {
    Notarize,
    Finalize,
}

/// Evidence of double-voting (Equivocation)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EquivocationEvidence {
    pub vote_a: Vote,
    pub vote_b: Vote,
}

/// A Vote from a validator for a specific block (Notarization) or view (Finalization/Timeout).
/// In Simplex, a timeout creates a vote for a "Dummy Block" (Notarize ZeroHash).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Vote {
    pub view: View,
    pub block_hash: Hash,    // The block being voted for (or ZeroHash/DummyHash)
    pub vote_type: VoteType, // Distinguish between Notarize and Finalize
    pub author: PublicKey,
    pub signature: Signature,
}

/// A Quorum Certificate (QC) proves that 2f+1 validators voted for a block.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct QuorumCertificate {
    pub view: View,
    pub block_hash: Hash,
    pub signature: Signature,    // Aggregated signature
    pub signers: Vec<PublicKey>, // Public keys of signers
}

/// Log entry from contract execution
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Log {
    pub address: Address,
    pub topics: Vec<Hash>,
    pub data: Bytes,
}

/// Transaction Receipt
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub status: u8, // 1 = Success, 0 = Revert
    pub cumulative_gas_used: u64,
    pub logs: Vec<Log>,
    // bloom ignored for simplicity in this iteration
}

/// Helper to calculate Merkle Root of receipts (Simplified)
/// In a real implementation this would use a Patricia Trie or proper Merkle Tree.
#[allow(clippy::manual_is_multiple_of)]
#[allow(clippy::clone_on_copy)]
pub fn calculate_receipts_root(receipts: &[Receipt]) -> Hash {
    if receipts.is_empty() {
        return Hash::default();
    }

    // Simple Merkle Tree Construction
    let mut leaves: Vec<Hash> = receipts.iter().map(crate::crypto::hash_data).collect();

    while leaves.len() > 1 {
        if leaves.len() % 2 != 0 {
            leaves.push(*leaves.last().unwrap());
        }
        let mut next_level = Vec::new();
        for chunk in leaves.chunks(2) {
            let left = &chunk[0];
            let right = &chunk[1];
            // Hash(left ++ right)
            let mut data = Vec::with_capacity(64);
            data.extend_from_slice(&left.0);
            data.extend_from_slice(&right.0);
            next_level.push(Hash(keccak256(&data).into()));
        }
        leaves = next_level;
    }
    leaves[0]
}

/// Messages used for Block Synchronization
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncMessage {
    RequestBlock(Hash),
    ResponseBlock(Box<Block>),
}
