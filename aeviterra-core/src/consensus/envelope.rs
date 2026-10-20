use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize, Serializer, Deserializer};
use std::collections::{BTreeMap, BTreeSet};

pub type Hash = [u8; 32];
pub const MAX_ACTION_PAYLOAD_BYTES: usize = 16_384; // 16 KB canonical intent payload cap (F16)

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Signature(pub [u8; 64]);

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SigVisitor;
        impl<'de> serde::de::Visitor<'de> for SigVisitor {
            type Value = Signature;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a 64-byte signature")
            }
            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v.len() == 64 {
                    let mut arr = [0u8; 64];
                    arr.copy_from_slice(v);
                    Ok(Signature(arr))
                } else {
                    Err(E::custom(format!("expected 64 bytes, got {}", v.len())))
                }
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut arr = [0u8; 64];
                for byte in &mut arr {
                    *byte = seq.next_element()?
                        .ok_or_else(|| serde::de::Error::custom("signature sequence too short"))?;
                }
                Ok(Signature(arr))
            }
        }
        deserializer.deserialize_bytes(SigVisitor)
    }
}

pub const DOMAIN_INTENT: &[u8; 8] = b"AEVI_ACT";
pub const DOMAIN_PROPOSAL: &[u8; 8] = b"AEVI_PRP";
pub const DOMAIN_VOTE: &[u8; 8] = b"AEVI_VOT";

/// Canonical Signed Action Intent Envelope.
/// Enforces complete domain separation and replay defense (F17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalActionEnvelope {
    pub world_id: Hash,
    pub protocol_epoch: u32,
    pub authority_shard: u32,
    pub actor_id: u64,
    pub session_id: u64,
    pub sequence_num: u64,
    pub target_tick: u64,
    pub action_type: u16,
    pub payload: Vec<u8>,
    pub signature: Signature,
}

impl CanonicalActionEnvelope {
    /// Compute the cryptographic digest for signing and replay defense.
    /// Includes explicit 32-bit payload length framing to prevent canonical boundary ambiguity (F16, F17).
    pub fn compute_signing_digest(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_INTENT);
        hasher.update(self.world_id);
        hasher.update(self.protocol_epoch.to_be_bytes());
        hasher.update(self.authority_shard.to_be_bytes());
        hasher.update(self.actor_id.to_be_bytes());
        hasher.update(self.session_id.to_be_bytes());
        hasher.update(self.sequence_num.to_be_bytes());
        hasher.update(self.target_tick.to_be_bytes());
        hasher.update(self.action_type.to_be_bytes());
        hasher.update((self.payload.len() as u32).to_be_bytes());
        hasher.update(&self.payload);
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionAdmissionError {
    WorldMismatch,
    EpochMismatch,
    ShardMismatch,
    PayloadTooLarge(usize),
    StaleOrReplayedSequence { expected_min: u64, got: u64 },
    TickWindowExpired,
}

/// Replay & Sequence Deduplication Guard for Signed Action Envelopes (F17, T03, T08).
/// Retains per-(actor_id, session_id) high-water marks so pruned tombstones never make old actions valid again.
#[derive(Debug, Clone)]
pub struct ActionAdmissionVerifier {
    pub world_id: Hash,
    pub protocol_epoch: u32,
    pub authority_shard: u32,
    pub max_tick_drift: u64,
    pub session_high_water_seq: BTreeMap<(u64, u64), u64>,
}

impl ActionAdmissionVerifier {
    pub fn new(world_id: Hash, protocol_epoch: u32, authority_shard: u32, max_tick_drift: u64) -> Self {
        ActionAdmissionVerifier {
            world_id,
            protocol_epoch,
            authority_shard,
            max_tick_drift,
            session_high_water_seq: BTreeMap::new(),
        }
    }

    pub fn verify_and_record(
        &mut self,
        env: &CanonicalActionEnvelope,
        current_tick: u64,
    ) -> Result<Hash, ActionAdmissionError> {
        if env.world_id != self.world_id {
            return Err(ActionAdmissionError::WorldMismatch);
        }
        if env.protocol_epoch != self.protocol_epoch {
            return Err(ActionAdmissionError::EpochMismatch);
        }
        if env.authority_shard != self.authority_shard {
            return Err(ActionAdmissionError::ShardMismatch);
        }
        if env.payload.len() > MAX_ACTION_PAYLOAD_BYTES {
            return Err(ActionAdmissionError::PayloadTooLarge(env.payload.len()));
        }
        if env.target_tick < current_tick.saturating_sub(self.max_tick_drift)
            || env.target_tick > current_tick.saturating_add(self.max_tick_drift)
        {
            return Err(ActionAdmissionError::TickWindowExpired);
        }

        let session_key = (env.actor_id, env.session_id);
        let last_seq = self.session_high_water_seq.get(&session_key).copied().unwrap_or(0);
        if env.sequence_num <= last_seq {
            return Err(ActionAdmissionError::StaleOrReplayedSequence {
                expected_min: last_seq.saturating_add(1),
                got: env.sequence_num,
            });
        }

        self.session_high_water_seq.insert(session_key, env.sequence_num);
        Ok(env.compute_signing_digest())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BftPhase {
    Prepare,
    PreCommit,
    Commit,
    ViewChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BftProposal {
    pub world_id: Hash,
    pub shard_id: u32,
    pub committee_epoch: u64,
    pub view: u64,
    pub height: u64,
    pub parent_hash: Hash,
    pub state_root: Hash,
    pub batch_digest: Hash,
    pub proposer_id: u64,
}

impl BftProposal {
    pub fn compute_hash(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_PROPOSAL);
        hasher.update(self.world_id);
        hasher.update(self.shard_id.to_be_bytes());
        hasher.update(self.committee_epoch.to_be_bytes());
        hasher.update(self.view.to_be_bytes());
        hasher.update(self.height.to_be_bytes());
        hasher.update(self.parent_hash);
        hasher.update(self.state_root);
        hasher.update(self.batch_digest);
        hasher.update(self.proposer_id.to_be_bytes());
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BftVote {
    pub phase: BftPhase,
    pub proposal_hash: Hash,
    pub view: u64,
    pub height: u64,
    pub validator_id: u64,
    pub signature: Signature,
}

impl BftVote {
    pub fn compute_digest(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_VOTE);
        hasher.update([self.phase as u8]);
        hasher.update(self.proposal_hash);
        hasher.update(self.view.to_be_bytes());
        hasher.update(self.height.to_be_bytes());
        hasher.update(self.validator_id.to_be_bytes());
        hasher.finalize().into()
    }
}

/// Durable BFT Commit Certificate with Quorum Proof.
/// Satisfies q >= 2f + 1 for an admitted committee of size n = 3f + 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BftCommitCertificate {
    pub proposal_hash: Hash,
    pub view: u64,
    pub height: u64,
    pub signers: Vec<u64>,
    pub signatures: Vec<Signature>,
}

impl BftCommitCertificate {
    /// Verifies that the certificate contains at least q = 2f + 1 DISTINCT signers
    /// for a committee of size n = 3f + 1, with 1-to-1 matching signatures (F02, T02).
    pub fn verify_quorum(&self, committee_size: usize) -> bool {
        if committee_size == 0 || self.signers.len() != self.signatures.len() {
            return false;
        }
        let unique_signers: BTreeSet<u64> = self.signers.iter().copied().collect();
        if unique_signers.len() != self.signers.len() {
            return false; // Reject duplicate signer IDs (T02)
        }
        let f = (committee_size - 1) / 3;
        let quorum_threshold = 2 * f + 1;
        unique_signers.len() >= quorum_threshold
    }

    /// Verifies quorum and also checks that every signer belongs to the admitted validator set (F01, T01).
    pub fn verify_quorum_with_committee(&self, admitted_committee: &BTreeSet<u64>) -> bool {
        if !self.verify_quorum(admitted_committee.len()) {
            return false;
        }
        self.signers.iter().all(|s| admitted_committee.contains(s))
    }
}
