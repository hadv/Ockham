use blst::min_sig::{
    AggregateSignature, PublicKey as BlstPublicKey, SecretKey, Signature as BlstSignature,
};
use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;

/// A Hash type (32 bytes), typically SHA-256.
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

// -----------------------------------------------------------------------------
// BLS Cryptography Implementation (using blst::min_sig)
// min_sig: Signatures in G1 (48 bytes), Public Keys in G2 (96 bytes).
// This is preferred for smaller signatures which are transmitted more frequently.
// -----------------------------------------------------------------------------

/// BLS Public Key (96 bytes).
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey(pub BlstPublicKey);

impl std::hash::Hash for PublicKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bytes().hash(state);
    }
}

impl Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes = self.0.to_bytes();
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        let pk = BlstPublicKey::from_bytes(&bytes)
            .map_err(|e| serde::de::Error::custom(format!("{:?}", e)))?;
        Ok(PublicKey(pk))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pub({})", hex::encode(self.0.to_bytes()))
    }
}

impl PartialOrd for PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PublicKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.to_bytes().cmp(&other.0.to_bytes())
    }
}

/// BLS Private Key.
#[derive(Clone)]
pub struct PrivateKey(pub SecretKey);

impl PrivateKey {
    /// Generate a new random Private Key.
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let mut ikm = [0u8; 32];
        rng.fill_bytes(&mut ikm);
        let sk = SecretKey::key_gen(&ikm, &[]).unwrap();
        PrivateKey(sk)
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.sk_to_pk())
    }
}

/// BLS Signature (48 bytes).
#[derive(Clone, PartialEq, Eq)]
pub struct Signature(pub BlstSignature);

impl std::hash::Hash for Signature {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bytes().hash(state);
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes = self.0.to_bytes();
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        let sig = BlstSignature::from_bytes(&bytes)
            .map_err(|e| serde::de::Error::custom(format!("{:?}", e)))?;
        Ok(Signature(sig))
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sig({})", hex::encode(self.0.to_bytes()))
    }
}

impl Default for Signature {
    fn default() -> Self {
        // Technically pure zero bytes isn't a valid BLS signature usually,
        // but for Default trait we might need something.
        // Let's adhere to "infinity" point if possible, or just a zeroed structure if blst supports it.
        // BlstSignature::default() doesn't exist.
        // We will panic if accessed, or create a dummy one.
        // Ideally we shouldn't use default signatures in logic.
        // For now, let's use an all-zero byte array which will likely fail verification but satisfy the type.
        // Actually blst doesn't expose a raw constructor easily without validation.
        // Let's leave it out or implement a dummy.
        // SAFE: just parsing empty bytes will fail.
        // Let's create a signature of a dummy message with a dummy key.
        let sk = SecretKey::key_gen(&[0u8; 32], &[]).unwrap();
        Signature(sk.sign(&[], &[], &[]))
    }
}

/// Signs a message (bytes) using the private key.
/// Domain separation tag (DST) is important for security.
const DST: &[u8] = b"BLS_SIG_BLS12381G1_XMD:SHA-256_SSWU_RO_POP_";

pub fn sign(priv_key: &PrivateKey, message: &[u8]) -> Signature {
    Signature(priv_key.0.sign(message, DST, &[]))
}

/// Verifies a signature.
pub fn verify(pub_key: &PublicKey, message: &[u8], signature: &Signature) -> bool {
    let err = signature
        .0
        .verify(true, message, DST, &[], &pub_key.0, true);
    err == blst::BLST_ERROR::BLST_SUCCESS
}

/// Helper to hash any serializable object
pub fn hash_data<T: Serialize>(data: &T) -> Hash {
    let serialized = serde_json::to_vec(data).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized);
    let result = hasher.finalize();
    Hash(result.into())
}

/// Generate a KeyPair (Public, Private).
pub fn generate_keypair() -> (PublicKey, PrivateKey) {
    let sk = PrivateKey::generate();
    let pk = sk.public_key();
    (pk, sk)
}

// -----------------------------------------------------------------------------
// VRF (Verifiable Random Function) using BLS
//
// Proof = BLS Signature on the input (seed).
// Output = Hash(Proof).
// -----------------------------------------------------------------------------

pub struct VRFProof(pub Signature);

impl VRFProof {
    pub fn to_hash(&self) -> Hash {
        // Hash the signature bytes to get the VRF output
        let bytes = self.0.0.to_bytes();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Hash(hasher.finalize().into())
    }
}

pub fn vrf_prove(priv_key: &PrivateKey, seed: &[u8]) -> VRFProof {
    // Deterministic signature on the seed
    let sig = sign(priv_key, seed);
    VRFProof(sig)
}

pub fn vrf_verify(pub_key: &PublicKey, seed: &[u8], proof: &VRFProof) -> bool {
    verify(pub_key, seed, &proof.0)
}

/// Aggregates multiple signatures into a single signature.
pub fn aggregate(signatures: &[Signature]) -> Option<Signature> {
    if signatures.is_empty() {
        return None;
    }
    let sig_refs: Vec<&BlstSignature> = signatures.iter().map(|s| &s.0).collect();
    match AggregateSignature::aggregate(&sig_refs, true) {
        Ok(agg) => Some(Signature(agg.to_signature())),
        Err(_) => None,
    }
}

/// Verifies an aggregated signature against a list of public keys for a single message.
/// This uses FastAggregateVerify optimization (all signers signed the same message).
pub fn verify_aggregate(pub_keys: &[PublicKey], message: &[u8], signature: &Signature) -> bool {
    if pub_keys.is_empty() {
        return false;
    }
    let pk_refs: Vec<&BlstPublicKey> = pub_keys.iter().map(|pk| &pk.0).collect();
    let err = signature
        .0
        .fast_aggregate_verify(true, message, DST, &pk_refs);
    err == blst::BLST_ERROR::BLST_SUCCESS
}

/// Generate a KeyPair from a u64 ID (deterministic).
/// Useful for static committees where keys are derived from IDs.
pub fn generate_keypair_from_id(id: u64) -> (PublicKey, PrivateKey) {
    let mut ikm = [0u8; 32];
    ikm[24..32].copy_from_slice(&id.to_be_bytes());
    // We use the ID as the Input Key Material (IKM)
    let sk = SecretKey::key_gen(&ikm, &[]).unwrap();
    let pk = sk.sk_to_pk();
    (PublicKey(pk), PrivateKey(sk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vrf() {
        let (pk, sk) = generate_keypair();
        let seed = b"test_seed";
        let proof = vrf_prove(&sk, seed);
        assert!(vrf_verify(&pk, seed, &proof));

        let output = proof.to_hash();
        println!("VRF Output: {:?}", output);

        // Check uniqueness (deterministic)
        let proof2 = vrf_prove(&sk, seed);
        assert_eq!(proof.0, proof2.0);

        // Check verification failure with wrong key
        let (pk2, _) = generate_keypair();
        assert!(!vrf_verify(&pk2, seed, &proof));

        // Check verification failure with wrong seed
        assert!(!vrf_verify(&pk, b"wrong_seed", &proof));
    }

    #[test]
    fn test_aggregation() {
        let message = b"consensus_vote";
        let mut sigs = Vec::new();
        let mut pub_keys = Vec::new();

        // 1. Generate 3 keypairs and sign the same message
        for _ in 0..3 {
            let (pk, sk) = generate_keypair();
            let sig = sign(&sk, message);
            sigs.push(sig);
            pub_keys.push(pk);
        }

        // 2. Aggregate
        let agg_sig = aggregate(&sigs).expect("Aggregation failed");

        // 3. Verify
        assert!(
            verify_aggregate(&pub_keys, message, &agg_sig),
            "Aggregate verification failed"
        );

        // 4. Negative test: wrong message
        assert!(
            !verify_aggregate(&pub_keys, b"wrong_msg", &agg_sig),
            "Verified wrong message"
        );

        // 5. Negative test: missing public key
        let mut partial_pks = pub_keys.clone();
        partial_pks.pop();
        assert!(
            !verify_aggregate(&partial_pks, message, &agg_sig),
            "Verified with missing pubkey"
        );
    }
}
