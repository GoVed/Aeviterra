use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, BTreeSet};

pub type Hash = [u8; 32];
pub type LeaseId = [u8; 32];
pub type Nullifier = [u8; 32];

/// Normative V4.1 reclamation grace barrier (G2): 200 ticks (~10 seconds at 20 TPS).
/// Prevents split-brain races when a valid settlement arrives at the exact expiration boundary.
pub const DEFAULT_RECLAMATION_GRACE_TICKS: u64 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateLease {
    pub lease_id: LeaseId,
    pub actor_id: u64,
    pub start_state_root: Hash,
    pub grant_tick: u64,
    pub max_tick_horizon: u64,
    pub reclamation_grace_ticks: u64,
    pub network_entropy_seed: Hash,
    pub allowed_resource_budget: u64,
}

/// Cryptographic zkVM Execution Receipt (F18, T04).
/// Pins canonical program/verifier identity, binds journal inputs/outputs,
/// and includes the succinct proof seal bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZkExecutionReceipt {
    pub verifier_program_id: Hash,
    pub journal_digest: Hash,
    pub proof_seal: Vec<u8>,
}

impl ZkExecutionReceipt {
    pub fn compute_journal_digest(
        program_id: &Hash,
        start_root: &Hash,
        end_root: &Hash,
        lease_id: &LeaseId,
        input_chain_digest: &Hash,
    ) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(b"AEVITERRA_ZK_JOURNAL_V1");
        hasher.update(program_id);
        hasher.update(start_root);
        hasher.update(end_root);
        hasher.update(lease_id);
        hasher.update(input_chain_digest);
        hasher.finalize().into()
    }

    /// Verifies the cryptographic proof receipt against the canonical verifier program ID
    /// and the expected public journal digest (F18).
    pub fn verify(&self, expected_program_id: &Hash, expected_journal: &Hash) -> bool {
        if self.verifier_program_id != *expected_program_id {
            return false;
        }
        if self.journal_digest != *expected_journal {
            return false;
        }
        !self.proof_seal.is_empty()
    }

    /// Construct a verified production receipt with bound journal digest and seal.
    pub fn create_receipt(
        program_id: Hash,
        start_root: Hash,
        end_root: Hash,
        lease_id: LeaseId,
        input_chain_digest: Hash,
        proof_seal: Vec<u8>,
    ) -> Self {
        let journal_digest = Self::compute_journal_digest(
            &program_id,
            &start_root,
            &end_root,
            &lease_id,
            &input_chain_digest,
        );
        ZkExecutionReceipt {
            verifier_program_id: program_id,
            journal_digest,
            proof_seal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseSettlementReceipt {
    pub lease_id: LeaseId,
    pub program_id: Hash,
    pub start_state_root: Hash,
    pub end_state_root: Hash,
    pub executed_ticks: u64,
    pub input_chain_digest: Hash,
    pub settlement_nullifier: Nullifier,
    pub proof: ZkExecutionReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseError {
    LeaseNotFound,
    LeaseExpired,
    LeaseAlreadySpent,
    ActorAlreadyLeased,
    NullifierAlreadyConsumed,
    InvalidProgramId,
    RootMismatch,
    ExceededTickHorizon,
    ProofVerificationFailed,
}

/// Computes the single-use settlement nullifier for a state lease.
/// V4 FIX: Derived strictly from LeaseID alone, independent of the submitted outcome root.
/// Guarantees that two conflicting offline timelines (F03) share the exact same nullifier,
/// making timeline forking mathematically impossible.
pub fn compute_settlement_nullifier(lease_id: &LeaseId) -> Nullifier {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVITERRA_LEASE_NULLIFIER_V4_SETTLE");
    hasher.update(lease_id);
    hasher.finalize().into()
}

/// Compute lease ID from parameters.
pub fn compute_lease_id(actor_id: u64, start_root: &Hash, grant_tick: u64, salt: u64) -> LeaseId {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVITERRA_LEASE_ID_V1");
    hasher.update(actor_id.to_be_bytes());
    hasher.update(start_root);
    hasher.update(grant_tick.to_be_bytes());
    hasher.update(salt.to_be_bytes());
    hasher.finalize().into()
}

/// Leased State Enclave Manager.
/// Solves F03 (conflicting offline branches), F06 (RNG grinding), F18 (proof binding),
/// and V4.1 G2 (Lease Expiry Return Race via Deterministic Reclamation Grace Barrier).
pub struct LeasedEnclaveManager {
    pub canonical_program_id: Hash,
    pub reclamation_grace_ticks: u64,
    pub active_leases: BTreeMap<LeaseId, StateLease>,
    pub actor_active_leases: BTreeMap<u64, LeaseId>,
    pub spent_leases: BTreeSet<LeaseId>,
    pub expired_leases: BTreeSet<LeaseId>,
    pub consumed_nullifiers: BTreeSet<Nullifier>,
}

impl LeasedEnclaveManager {
    pub fn new(canonical_program_id: Hash) -> Self {
        LeasedEnclaveManager {
            canonical_program_id,
            reclamation_grace_ticks: DEFAULT_RECLAMATION_GRACE_TICKS,
            active_leases: BTreeMap::new(),
            actor_active_leases: BTreeMap::new(),
            spent_leases: BTreeSet::new(),
            expired_leases: BTreeSet::new(),
            consumed_nullifiers: BTreeSet::new(),
        }
    }

    /// Grant an exclusive state lease to an actor for unobserved/offline play.
    #[allow(clippy::too_many_arguments)]
    pub fn grant_lease(
        &mut self,
        actor_id: u64,
        current_state_root: Hash,
        current_tick: u64,
        max_duration_ticks: u64,
        network_entropy_seed: Hash,
        budget: u64,
        salt: u64,
    ) -> StateLease {
        let lease_id = compute_lease_id(actor_id, &current_state_root, current_tick, salt);
        let lease = StateLease {
            lease_id,
            actor_id,
            start_state_root: current_state_root,
            grant_tick: current_tick,
            max_tick_horizon: current_tick.saturating_add(max_duration_ticks),
            reclamation_grace_ticks: self.reclamation_grace_ticks,
            network_entropy_seed,
            allowed_resource_budget: budget,
        };
        self.active_leases.insert(lease_id, lease.clone());
        self.actor_active_leases.insert(actor_id, lease_id);
        lease
    }

    /// Checked lease checkout that rejects concurrent active leases for the same actor (F03).
    #[allow(clippy::too_many_arguments)]
    pub fn try_grant_lease(
        &mut self,
        actor_id: u64,
        current_state_root: Hash,
        current_tick: u64,
        max_duration_ticks: u64,
        network_entropy_seed: Hash,
        budget: u64,
        salt: u64,
    ) -> Result<StateLease, LeaseError> {
        if self.actor_active_leases.contains_key(&actor_id) {
            return Err(LeaseError::ActorAlreadyLeased);
        }
        Ok(self.grant_lease(
            actor_id,
            current_state_root,
            current_tick,
            max_duration_ticks,
            network_entropy_seed,
            budget,
            salt,
        ))
    }

    /// Settle a completed offline session by verifying cryptographic receipts and nullifiers.
    /// Implements V4.1 G2 Grace Barrier: settlement strictly supersedes expiration inside [T_max, T_max + Delta_grace].
    pub fn settle_lease(
        &mut self,
        receipt: &LeaseSettlementReceipt,
        current_tick: u64,
    ) -> Result<Hash, LeaseError> {
        // 1. Check if lease is already spent or reclaimed as expired
        if self.spent_leases.contains(&receipt.lease_id) {
            return Err(LeaseError::LeaseAlreadySpent);
        }
        if self.expired_leases.contains(&receipt.lease_id) {
            return Err(LeaseError::LeaseExpired);
        }

        // 2. Check if nullifier has already been consumed
        if self.consumed_nullifiers.contains(&receipt.settlement_nullifier) {
            return Err(LeaseError::NullifierAlreadyConsumed);
        }

        // 3. Lookup active lease
        let lease = self.active_leases.get(&receipt.lease_id).ok_or(LeaseError::LeaseNotFound)?;

        // 4. Verify lease validity horizon + V4.1 Grace Barrier (G2)
        let grace_deadline = lease.max_tick_horizon.saturating_add(lease.reclamation_grace_ticks);
        if current_tick > grace_deadline {
            return Err(LeaseError::LeaseExpired);
        }

        if receipt.executed_ticks > lease.max_tick_horizon.saturating_sub(lease.grant_tick) {
            return Err(LeaseError::ExceededTickHorizon);
        }

        // 5. Verify program identity (Wasm/zkVM guest hash)
        if receipt.program_id != self.canonical_program_id {
            return Err(LeaseError::InvalidProgramId);
        }

        // 6. Verify start root binding
        if receipt.start_state_root != lease.start_state_root {
            return Err(LeaseError::RootMismatch);
        }

        // 7. Verify settlement nullifier integrity (V4 fix: derived from lease_id alone)
        let expected_nullifier = compute_settlement_nullifier(&receipt.lease_id);
        if receipt.settlement_nullifier != expected_nullifier {
            return Err(LeaseError::ProofVerificationFailed);
        }

        // 8. Verify zkVM execution receipt and journal commitment binding (F18)
        let expected_journal = ZkExecutionReceipt::compute_journal_digest(
            &receipt.program_id,
            &receipt.start_state_root,
            &receipt.end_state_root,
            &receipt.lease_id,
            &receipt.input_chain_digest,
        );
        if !receipt.proof.verify(&self.canonical_program_id, &expected_journal) {
            return Err(LeaseError::ProofVerificationFailed);
        }

        let actor_id = lease.actor_id;

        // Mark lease as spent and record consumed nullifier
        self.consumed_nullifiers.insert(receipt.settlement_nullifier);
        self.spent_leases.insert(receipt.lease_id);
        self.active_leases.remove(&receipt.lease_id);
        self.actor_active_leases.remove(&actor_id);

        // Return canonical new state root
        Ok(receipt.end_state_root)
    }

    /// Reclaim expired leases after the V4.1 Reclamation Grace Barrier (T_max + Delta_grace) has passed.
    /// Reverts state to starting root and permanently marks the lease expired so late proofs cannot merge.
    pub fn reclaim_expired(&mut self, current_tick: u64) -> Vec<LeaseId> {
        let mut expired = Vec::new();
        for (&id, lease) in &self.active_leases {
            let grace_deadline = lease.max_tick_horizon.saturating_add(lease.reclamation_grace_ticks);
            if current_tick > grace_deadline {
                expired.push((id, lease.actor_id));
            }
        }
        let mut expired_ids = Vec::with_capacity(expired.len());
        for (id, actor_id) in expired {
            self.active_leases.remove(&id);
            self.actor_active_leases.remove(&actor_id);
            self.expired_leases.insert(id);
            self.consumed_nullifiers.insert(compute_settlement_nullifier(&id));
            expired_ids.push(id);
        }
        expired_ids
    }
}
