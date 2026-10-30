use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, BTreeSet};

pub type Hash = [u8; 32];
pub type ClaimNullifier = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestTemplate {
    pub quest_id: u32,
    pub title: String,
    pub target_count: u32,
    pub reward_marks: u64,
    pub reward_sovereigns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerQuestProgress {
    pub quest_id: u32,
    pub current_count: u32,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestError {
    QuestNotFound,
    NotCompleted,
    RewardAlreadyClaimed,
}

/// Compute consumed-once reward claim nullifier (V4 §10.4, P05).
pub fn compute_quest_claim_nullifier(actor_id: u64, quest_id: u32, step: u32) -> ClaimNullifier {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVITERRA_QUEST_CLAIM_V1");
    hasher.update(actor_id.to_be_bytes());
    hasher.update(quest_id.to_be_bytes());
    hasher.update(step.to_be_bytes());
    hasher.finalize().into()
}

/// Idempotent Canonical Quest & Reward Engine.
pub struct QuestEngine {
    pub templates: BTreeMap<u32, QuestTemplate>,
    pub player_progress: BTreeMap<(u64, u32), PlayerQuestProgress>,
    pub consumed_claims: BTreeSet<ClaimNullifier>,
}

impl Default for QuestEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestEngine {
    pub fn new() -> Self {
        QuestEngine {
            templates: BTreeMap::new(),
            player_progress: BTreeMap::new(),
            consumed_claims: BTreeSet::new(),
        }
    }

    pub fn record_kill(&mut self, actor_id: u64, quest_id: u32) {
        let tmpl = match self.templates.get(&quest_id) {
            Some(t) => t,
            None => return,
        };

        let progress = self.player_progress.entry((actor_id, quest_id)).or_insert(PlayerQuestProgress {
            quest_id,
            current_count: 0,
            completed: false,
        });

        progress.current_count = progress.current_count.saturating_add(1);
        if progress.current_count >= tmpl.target_count {
            progress.completed = true;
        }
    }

    /// Claim quest reward for step 1. Enforces single-use nullifier to prevent double-claim / double-loot.
    pub fn claim_reward(&mut self, actor_id: u64, quest_id: u32) -> Result<(u64, u64), QuestError> {
        self.claim_reward_step(actor_id, quest_id, 1)
    }

    /// Claim quest reward for a specific multi-stage quest step.
    pub fn claim_reward_step(&mut self, actor_id: u64, quest_id: u32, step: u32) -> Result<(u64, u64), QuestError> {
        if step == 0 {
            return Err(QuestError::NotCompleted);
        }
        let progress = self.player_progress.get(&(actor_id, quest_id)).ok_or(QuestError::QuestNotFound)?;
        let tmpl = self.templates.get(&quest_id).ok_or(QuestError::QuestNotFound)?;
        let required_count = tmpl.target_count.saturating_mul(step);
        if !progress.completed || progress.current_count < required_count {
            return Err(QuestError::NotCompleted);
        }

        let nullifier = compute_quest_claim_nullifier(actor_id, quest_id, step);
        if self.consumed_claims.contains(&nullifier) {
            return Err(QuestError::RewardAlreadyClaimed);
        }

        let rewards = (tmpl.reward_marks, tmpl.reward_sovereigns);

        self.consumed_claims.insert(nullifier);
        Ok(rewards)
    }
}
