use crate::crypto::{Hash, PublicKey, Signature};
use serde::{Deserialize, Serialize};

/// The View number definition (u64).
pub type View = u64;

/// A Block in the Simplex chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub author: PublicKey,
    pub view: View,
    pub parent_hash: Hash,
    pub justify: QuorumCertificate, // The QC that justifies this block (usually for parent)
    pub payload: Vec<u8>,           // Transactions (empty if dummy)
    pub is_dummy: bool,             // Simplex specific: Dummy blocks for timeout
}

impl Block {
    pub fn new(
        author: PublicKey,
        view: View,
        parent_hash: Hash,
        justify: QuorumCertificate,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            author,
            view,
            parent_hash,
            justify,
            payload,
            is_dummy: false,
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
            payload: vec![],
            is_dummy: true,
        }
    }
}

/// A Vote from a validator for a specific block (Notarization) or view (Finalization/Timeout).
/// In Simplex, a timeout creates a vote for a "Dummy Block" which effectively
/// allows moving to the next view.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Vote {
    pub view: View,
    pub block_hash: Hash, // The block being voted for (or ZeroHash/DummyHash)
    pub author: PublicKey,
    pub signature: Signature,
}

/// A Quorum Certificate (QC) proves that 2f+1 validators voted for a block.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct QuorumCertificate {
    pub view: View,
    pub block_hash: Hash,
    pub signatures: Vec<(PublicKey, Signature)>, // In real impl, this would be an aggregated sig + bitfield
}
