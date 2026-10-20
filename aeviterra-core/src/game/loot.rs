use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LootTier {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LootDrop {
    pub item_id: u32,
    pub tier: LootTier,
    pub quantity: u32,
}

/// Tier-Compartmentalized Pity Counter Engine (V4 §10.3 hardened).
/// Strictly prevents grinding low-tier mobs to prime pity counters for high-tier boss loot!
pub struct PityLootEngine {
    /// (actor_id, loot_tier) -> consecutive misses for that specific tier
    pub player_pity_counters: BTreeMap<(u64, LootTier), u32>,
    /// Base probability in parts per million (ppm)
    pub base_probabilities_ppm: BTreeMap<LootTier, u32>,
    /// Pity threshold: guaranteed drop after N consecutive misses
    pub pity_thresholds: BTreeMap<LootTier, u32>,
}

impl Default for PityLootEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PityLootEngine {
    pub fn new() -> Self {
        let mut base_probabilities_ppm = BTreeMap::new();
        base_probabilities_ppm.insert(LootTier::Common, 800_000);     // 80%
        base_probabilities_ppm.insert(LootTier::Uncommon, 150_000);   // 15%
        base_probabilities_ppm.insert(LootTier::Rare, 40_000);        // 4%
        base_probabilities_ppm.insert(LootTier::Epic, 9_000);         // 0.9%
        base_probabilities_ppm.insert(LootTier::Legendary, 1_000);    // 0.1%

        let mut pity_thresholds = BTreeMap::new();
        pity_thresholds.insert(LootTier::Rare, 30);
        pity_thresholds.insert(LootTier::Epic, 100);
        pity_thresholds.insert(LootTier::Legendary, 500);

        PityLootEngine {
            player_pity_counters: BTreeMap::new(),
            base_probabilities_ppm,
            pity_thresholds,
        }
    }

    pub fn get_miss_count(&self, actor_id: u64, tier: LootTier) -> u32 {
        self.player_pity_counters.get(&(actor_id, tier)).copied().unwrap_or(0)
    }

    /// Evaluates loot roll against deterministic threshold beacon entropy.
    /// Resets pity counter ONLY for the won tier!
    pub fn evaluate_tier_roll(
        &mut self,
        actor_id: u64,
        target_tier: LootTier,
        random_roll_ppm: u32,
    ) -> bool {
        let current_misses = self.get_miss_count(actor_id, target_tier);
        let base_prob = self.base_probabilities_ppm.get(&target_tier).copied().unwrap_or(10_000);
        let threshold = self.pity_thresholds.get(&target_tier).copied().unwrap_or(100).max(1);

        // Effective probability scales upward with consecutive misses (computed in u64 to prevent u32 overflow)
        let pity_bonus = ((current_misses as u64) * (base_prob as u64) / (threshold as u64)) as u32;
        let effective_prob = base_prob.saturating_add(pity_bonus).min(1_000_000);

        let won = (random_roll_ppm < effective_prob) || (current_misses >= threshold);

        if won {
            // Reset ONLY this tier's pity counter
            self.player_pity_counters.insert((actor_id, target_tier), 0);
        } else {
            // Increment miss counter for this tier
            self.player_pity_counters.insert((actor_id, target_tier), current_misses + 1);
        }

        won
    }
}
