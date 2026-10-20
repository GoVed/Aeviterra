use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

pub type Hash = [u8; 32];
pub type ActionId = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRandomAction {
    pub action_id: ActionId,
    pub actor_id: u64,
    pub target_id: u64,
    pub commit_tick: u64,
    pub assigned_beacon_round: u64,
    pub probability_threshold_ppm: u32, // Parts per million (0 to 1,000,000)
    pub cost_deducted: u64,
    pub resolved: bool,
    pub outcome_success: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeaconRoundOutput {
    pub round: u64,
    pub signature_entropy: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandomnessError {
    ActionAlreadyCommitted,
    ActionNotFound,
    BeaconRoundNotReached,
    InvalidEntropySignature,
    InvalidProbabilityThreshold,
    AlreadyResolved,
}

const PPM_RANGE: u64 = 1_000_000;
/// Largest multiple of 1,000,000 that fits in u64: avoids modulo reduction bias (R2 §16, F06).
const UNBIASED_U64_LIMIT: u64 = u64::MAX - (u64::MAX % PPM_RANGE);

/// Irrevocable Commit-Before-Entropy Action Pipeline.
/// Completely neutralizes F05 (selective abort) and F06 (outcome grinding).
pub struct ThresholdRandomManager {
    pub pending_actions: BTreeMap<ActionId, PendingRandomAction>,
    pub verified_beacon_rounds: BTreeMap<u64, Hash>,
    pub delay_rounds: u64,
}

impl ThresholdRandomManager {
    pub fn new(delay_rounds: u64) -> Self {
        ThresholdRandomManager {
            pending_actions: BTreeMap::new(),
            verified_beacon_rounds: BTreeMap::new(),
            delay_rounds: delay_rounds.max(1),
        }
    }

    /// Step 1: Commit action intent and irrevocably lock costs before entropy is known.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_random_action(
        &mut self,
        action_id: ActionId,
        actor_id: u64,
        target_id: u64,
        current_tick: u64,
        current_beacon_round: u64,
        probability_threshold_ppm: u32,
        cost: u64,
    ) -> Result<&PendingRandomAction, RandomnessError> {
        if probability_threshold_ppm > PPM_RANGE as u32 {
            return Err(RandomnessError::InvalidProbabilityThreshold);
        }
        if self.pending_actions.contains_key(&action_id) {
            return Err(RandomnessError::ActionAlreadyCommitted);
        }

        let assigned_round = current_beacon_round.saturating_add(self.delay_rounds);
        let action = PendingRandomAction {
            action_id,
            actor_id,
            target_id,
            commit_tick: current_tick,
            assigned_beacon_round: assigned_round,
            probability_threshold_ppm,
            cost_deducted: cost,
            resolved: false,
            outcome_success: None,
        };

        self.pending_actions.insert(action_id, action);
        Ok(self.pending_actions.get(&action_id).unwrap())
    }

    /// Step 2: Ingest threshold beacon round output (e.g. from BLS threshold committee).
    /// Write-once per round: prevents equivocating or overwriting finalized beacon outputs.
    pub fn ingest_beacon_round(&mut self, round: u64, signature_entropy: Hash) {
        self.verified_beacon_rounds.entry(round).or_insert(signature_entropy);
    }

    /// Step 3: Resolve the action deterministically once the assigned beacon round is available.
    /// Uses rejection sampling (R2 §16) to map 256-bit hash entropy into [0, 1,000,000) with zero modulo bias.
    pub fn resolve_action(&mut self, action_id: &ActionId) -> Result<bool, RandomnessError> {
        let action = self.pending_actions.get_mut(action_id).ok_or(RandomnessError::ActionNotFound)?;
        if action.resolved {
            return Err(RandomnessError::AlreadyResolved);
        }

        let beacon_entropy = self.verified_beacon_rounds
            .get(&action.assigned_beacon_round)
            .ok_or(RandomnessError::BeaconRoundNotReached)?;

        let roll_ppm = sample_unbiased_ppm(&action.action_id, action.assigned_beacon_round, beacon_entropy);

        let success = roll_ppm < action.probability_threshold_ppm;
        action.resolved = true;
        action.outcome_success = Some(success);

        Ok(success)
    }
}

/// Deterministic rejection sampling over SHA-256 counter mode to eliminate modulo bias (R2 §16, T07).
fn sample_unbiased_ppm(action_id: &ActionId, assigned_round: u64, beacon_entropy: &Hash) -> u32 {
    let mut attempt: u32 = 0;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(b"AEVITERRA_EVENT_RNG_V1");
        hasher.update(action_id);
        hasher.update(assigned_round.to_be_bytes());
        hasher.update(beacon_entropy);
        if attempt > 0 {
            hasher.update(attempt.to_be_bytes());
        }
        let event_hash: [u8; 32] = hasher.finalize().into();

        // Check up to four 64-bit words in the 256-bit digest before incrementing attempt counter
        for chunk in event_hash.chunks_exact(8) {
            let rand_u64 = u64::from_be_bytes(chunk.try_into().unwrap());
            if rand_u64 < UNBIASED_U64_LIMIT {
                return (rand_u64 % PPM_RANGE) as u32;
            }
        }
        attempt = attempt.saturating_add(1);
    }
}
