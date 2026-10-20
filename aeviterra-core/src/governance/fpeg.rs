use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::BTreeSet;

pub type Hash = [u8; 32];
pub type Signature = [u8; 64];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CryptoSuite {
    Suite1Ed25519Sha256,
    Suite2PostQuantumMlDsaBlake3,
    Suite3RecursiveZkSnarkPoseidon,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochMigrationProposal {
    pub target_epoch: u64,
    pub activation_height: u64,
    pub crypto_suite: CryptoSuite,
    pub new_wasm_kernel_hash: Hash,
    pub migration_snark_program_id: Hash,
    pub pre_announcement_timestamp: u64,
}

impl EpochMigrationProposal {
    pub fn compute_hash(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(b"AEVITERRA_FPEG_MIGRATION_V1");
        hasher.update(self.target_epoch.to_be_bytes());
        hasher.update(self.activation_height.to_be_bytes());
        hasher.update([self.crypto_suite as u8]);
        hasher.update(self.new_wasm_kernel_hash);
        hasher.update(self.migration_snark_program_id);
        hasher.update(self.pre_announcement_timestamp.to_be_bytes());
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochMigrationCertificate {
    pub proposal_hash: Hash,
    pub target_epoch: u64,
    pub activation_height: u64,
    pub signers: Vec<u64>,
}

impl EpochMigrationCertificate {
    pub fn verify_supermajority(&self, committee_size: usize) -> bool {
        if committee_size == 0 {
            return false;
        }
        let unique_signers: BTreeSet<u64> = self.signers.iter().copied().collect();
        if unique_signers.len() != self.signers.len() {
            return false; // Reject duplicate validator IDs
        }
        let f = (committee_size - 1) / 3;
        let quorum = 2 * f + 1;
        unique_signers.len() >= quorum
    }
}

/// Formalized Protocol Epoch Gate Manager (V4.1 §18, F09, F29, T12).
/// Replaces Kelvin 0K code freeze with verifiable epoch transitions and cryptographic agility.
pub struct EpochManager {
    pub current_epoch: u64,
    pub active_crypto_suite: CryptoSuite,
    pub active_kernel_hash: Hash,
    pub pending_migration: Option<EpochMigrationCertificate>,
    pub pending_proposal: Option<EpochMigrationProposal>,
}

impl EpochManager {
    pub fn new(initial_kernel_hash: Hash) -> Self {
        EpochManager {
            current_epoch: 1,
            active_crypto_suite: CryptoSuite::Suite1Ed25519Sha256,
            active_kernel_hash: initial_kernel_hash,
            pending_migration: None,
            pending_proposal: None,
        }
    }

    pub fn schedule_migration(&mut self, cert: EpochMigrationCertificate, committee_size: usize) -> Result<(), &'static str> {
        if !cert.verify_supermajority(committee_size) {
            return Err("Insufficient distinct validator signatures for epoch migration");
        }
        if cert.target_epoch != self.current_epoch + 1 {
            return Err("Invalid target epoch sequence");
        }
        self.pending_migration = Some(cert);
        Ok(())
    }

    pub fn schedule_verified_proposal(
        &mut self,
        proposal: EpochMigrationProposal,
        cert: EpochMigrationCertificate,
        committee_size: usize,
    ) -> Result<(), &'static str> {
        if proposal.compute_hash() != cert.proposal_hash {
            return Err("Certificate proposal_hash does not match EpochMigrationProposal");
        }
        if proposal.target_epoch != cert.target_epoch || proposal.activation_height != cert.activation_height {
            return Err("Proposal and certificate metadata mismatch");
        }
        self.schedule_migration(cert, committee_size)?;
        self.pending_proposal = Some(proposal);
        Ok(())
    }

    pub fn maybe_activate_migration(&mut self, current_height: u64, new_suite: CryptoSuite, new_kernel_hash: Hash) -> bool {
        if let Some(ref cert) = self.pending_migration
            && current_height >= cert.activation_height {
                if let Some(ref prop) = self.pending_proposal
                    && (prop.crypto_suite != new_suite || prop.new_wasm_kernel_hash != new_kernel_hash) {
                        return false;
                    }
                self.current_epoch = cert.target_epoch;
                self.active_crypto_suite = new_suite;
                self.active_kernel_hash = new_kernel_hash;
                self.pending_migration = None;
                self.pending_proposal = None;
                return true;
            }
        false
    }
}
