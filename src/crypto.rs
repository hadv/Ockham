use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// A Mock Hash type (32 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Hash(pub [u8; 32]);

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl From<[u8; 32]> for Hash {
    fn from(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A Mock Signature type.
/// For simplicity in Phase 1, we just wrap the message hash.
#[derive(Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Signature(pub Vec<u8>);

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sig({})", hex::encode(&self.0))
    }
}

/// A Mock Public Key.
/// Identification is just a u64 ID for Phase 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PublicKey(pub u64);

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pub({})", self.0)
    }
}

/// A Mock Private Key.
#[derive(Clone, Copy)]
pub struct PrivateKey(pub u64);

/// Signs a message (bytes) using the private key.
/// In this mock, we just hash the message and append the key ID.
pub fn sign(priv_key: &PrivateKey, message: &[u8]) -> Signature {
    let mut hasher = Sha256::new();
    hasher.update(message);
    hasher.update(priv_key.0.to_be_bytes()); // "Signed" by this ID
    let result = hasher.finalize();
    Signature(result.to_vec())
}

/// Verifies a signature.
/// Checks if Hash(message || pub_key) == signature.
pub fn verify(pub_key: &PublicKey, message: &[u8], signature: &Signature) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(message);
    hasher.update(pub_key.0.to_be_bytes());
    let result = hasher.finalize();
    signature.0 == result.as_slice()
}

/// Helper to hash any serializable object
pub fn hash_data<T: Serialize>(data: &T) -> Hash {
    let serialized = serde_json::to_vec(data).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized);
    let result = hasher.finalize();
    Hash(result.into())
}
