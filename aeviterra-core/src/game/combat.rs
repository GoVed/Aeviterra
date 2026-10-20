use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaponFamily {
    Blade,
    Spear,
    Bow,
    WandPrism,
    Staff,
    Shield,
    Shadow,
    Song,
    Tinker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombatActionType {
    LightAttack,
    HeavyAttack,
    DodgeRoll,
    Block,
    SpecialSkill(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterAttributes {
    pub might: u32,
    pub finesse: u32,
    pub resolve: u32,
    pub wit: u32,
    pub spirit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatState {
    pub actor_id: u64,
    pub health: u32,
    pub max_health: u32,
    pub stamina: u32,
    pub max_stamina: u32,
    pub active_weapon: WeaponFamily,
    pub secondary_weapon: Option<WeaponFamily>,
    pub is_blocking: bool,
    pub iframe_remaining_ticks: u32,
    pub notoriety_score: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombatError {
    InsufficientStamina,
    Dead,
    InvalidTarget,
    TargetInvulnerable,
    OutOfRange,
}

impl CombatState {
    pub fn new(actor_id: u64, active_weapon: WeaponFamily) -> Self {
        CombatState {
            actor_id,
            health: 1000,
            max_health: 1000,
            stamina: 100,
            max_stamina: 100,
            active_weapon,
            secondary_weapon: None,
            is_blocking: false,
            iframe_remaining_ticks: 0,
            notoriety_score: 0,
        }
    }

    /// Process a combat action intent. Gated strictly by stamina rather than timers.
    pub fn execute_action(&mut self, action: CombatActionType) -> Result<u32, CombatError> {
        if self.health == 0 {
            return Err(CombatError::Dead);
        }

        let stamina_cost = match action {
            CombatActionType::LightAttack => 15,
            CombatActionType::HeavyAttack => 35,
            CombatActionType::DodgeRoll => 25,
            CombatActionType::Block => 10,
            CombatActionType::SpecialSkill(_) => 30,
        };

        if self.stamina < stamina_cost {
            return Err(CombatError::InsufficientStamina);
        }

        self.stamina -= stamina_cost;

        if action == CombatActionType::DodgeRoll {
            // Grants 10 ticks (~200ms) of invincible i-frames
            self.iframe_remaining_ticks = 10;
        }

        self.is_blocking = action == CombatActionType::Block;
        Ok(stamina_cost)
    }

    /// Take incoming damage. Applies blocking mitigation and i-frame avoidance.
    pub fn apply_incoming_damage(&mut self, raw_damage: u32) -> u32 {
        if self.iframe_remaining_ticks > 0 {
            return 0; // Dodged via i-frames
        }

        let effective_damage = if self.is_blocking {
            // Blocking reduces damage by 70% (u64 intermediate avoids u32 overflow)
            ((raw_damage as u64) * 30 / 100) as u32
        } else {
            raw_damage
        };

        if effective_damage >= self.health {
            self.health = 0;
        } else {
            self.health -= effective_damage;
        }

        effective_damage
    }

    /// Evaluate Crowd Control (Stun/Freeze/Stagger) with strict Diminishing Returns (V4.1 §2).
    /// 1st stack: 100% duration, 2nd stack: 50% duration, 3rd stack: 25% duration, >=4th stack: 0 (Immune).
    pub fn evaluate_crowd_control_duration(base_duration_ticks: u32, recent_cc_stacks: u32) -> u32 {
        match recent_cc_stacks {
            0 => base_duration_ticks,
            1 => base_duration_ticks / 2,
            2 => base_duration_ticks / 4,
            _ => 0, // Stun-lock immunity
        }
    }

    pub fn tick_regeneration(&mut self) {
        if self.iframe_remaining_ticks > 0 {
            self.iframe_remaining_ticks -= 1;
        }
        if self.stamina < self.max_stamina {
            self.stamina = (self.stamina + 5).min(self.max_stamina);
        }
    }
}
