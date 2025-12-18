use crate::types::EquivocationEvidence;
use std::collections::HashMap;

/// simple pool to manage collected evidence.
#[derive(Default, Debug)]
pub struct EvidencePool {
    // Map: Author -> List of Evidence (could be multiple views)
    evidences: HashMap<crate::crypto::PublicKey, Vec<EquivocationEvidence>>,
}

impl EvidencePool {
    pub fn new() -> Self {
        Self {
            evidences: HashMap::new(),
        }
    }

    /// Add evidence if valid and not already present.
    pub fn add_evidence(&mut self, evidence: EquivocationEvidence) -> bool {
        let author = evidence.vote_a.author.clone();

        let existing = self.evidences.entry(author).or_default();
        if existing.contains(&evidence) {
            return false;
        }

        // Basic sanity checks
        if evidence.vote_a.author != evidence.vote_b.author {
            return false;
        }
        if evidence.vote_a.view != evidence.vote_b.view {
            return false;
        }
        if evidence.vote_a.block_hash == evidence.vote_b.block_hash {
            return false; // Not equivocation if same block
        }

        // Signature verification is assumed to be done by caller or consensus before adding here
        // But for safety we could re-verify. For now, assume honest usage from consensus.

        existing.push(evidence);
        true
    }

    /// Get all pending evidence for inclusion in a block.
    pub fn get_all(&self) -> Vec<EquivocationEvidence> {
        self.evidences.values().flatten().cloned().collect()
    }

    /// Remove evidence that has been included in a block/processed.
    pub fn remove_evidence(&mut self, evidence: &[EquivocationEvidence]) {
        for e in evidence {
            if let Some(list) = self.evidences.get_mut(&e.vote_a.author)
                && let Some(pos) = list.iter().position(|x| x == e)
            {
                list.remove(pos);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.evidences.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
