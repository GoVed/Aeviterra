use aeviterra_core::math::fixed::{Q64, Vec3Fixed, MathError};
use aeviterra_core::state::smt::{SparseMerkleTree, EntityRecord, Hash, hash_leaf};
use aeviterra_core::shard::halo::{ChunkCoord, DynamicHaloCalculator};
use aeviterra_core::economy::thermodynamics::{ThermodynamicLedger, AgentCapabilityEnvelope};
use aeviterra_core::game::combat::{CombatState, CombatActionType, WeaponFamily};
use aeviterra_core::game::loot::PityLootEngine;
use crate::terrain::{GenesisTerrainGenerator, ResourceNode};
use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerAvatar {
    pub entity_id: u64,
    pub name: String,
    pub position: Vec3Fixed,
    pub velocity: Vec3Fixed,
    pub combat: CombatState,
    pub inventory: BTreeMap<u32, u64>, // resource_id -> count
    pub marks: u64,                    // soft currency
    pub sovereigns: u64,               // hard currency
    #[serde(default)]
    pub equipped_weapon_item: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpcEntity {
    pub entity_id: u64,
    pub name: String,
    pub position: Vec3Fixed,
    pub combat: CombatState,
    pub capability: AgentCapabilityEnvelope,
    pub is_hostile: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerIntent {
    Move { dx: Q64, dz: Q64 },
    Attack(CombatActionType),
    SwitchWeapon(WeaponFamily),
    Harvest(u64), // resource node_id
    CraftSword,
    CraftBow,
    CraftShield,
    EquipItem(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    EntityMoved { id: u64, new_pos: Vec3Fixed },
    CombatActionExecuted { actor_id: u64, action: CombatActionType, stamina_cost: u32 },
    CombatDamageDealt { attacker: u64, target: u64, damage: u32, target_health: u32 },
    ResourceHarvested { actor_id: u64, resource_name: &'static str, amount: u64 },
    ItemCrafted { crafter_id: u64, item_name: &'static str },
    ItemEquipped { actor_id: u64, item_id: u32, item_name: &'static str },
    NpcSlain { killer: u64, target: u64, reward_marks: u64 },
    PlayerDied { entity_id: u64 },
    StateRootUpdated { tick: u64, root: Hash },
}

/// Autonomous Virtual World Simulation Engine.
/// Pure deterministic state transition machine operating at 20 ticks/sec (V4.1 Specification).
#[allow(dead_code)]
pub struct WorldSimulation {
    pub genesis_seed: [u8; 32],
    pub terrain: GenesisTerrainGenerator,
    pub current_tick: u64,
    pub player: PlayerAvatar,
    pub npcs: BTreeMap<u64, NpcEntity>,
    pub resource_nodes: BTreeMap<u64, ResourceNode>,
    pub smt: SparseMerkleTree,
    pub thermodynamic_ledger: ThermodynamicLedger,
    pub pity_engine: PityLootEngine,
    pub halo_calculator: DynamicHaloCalculator,
    pub last_state_root: Hash,
}

impl WorldSimulation {
    pub fn new(player_name: String) -> Result<Self, MathError> {
        let genesis_seed = *b"AEVITERRA_GENESIS_SEED_2026_100Y";
        let terrain = GenesisTerrainGenerator::new(genesis_seed);

        let spawn_x = Q64::from_int(64)?;
        let spawn_z = Q64::from_int(64)?;
        let spawn_y = terrain.elevation_at(spawn_x, spawn_z)?;
        let spawn_pos = Vec3Fixed::new(spawn_x, spawn_y, spawn_z);

        let mut inventory = BTreeMap::new();
        inventory.insert(1, 0); // Iron Ore
        inventory.insert(2, 0); // Oak Timber
        inventory.insert(3, 0); // Silver
        inventory.insert(4, 0); // Direwolf Pelt
        inventory.insert(5, 0); // Sharp Fang

        let player = PlayerAvatar {
            entity_id: 1001,
            name: player_name,
            position: spawn_pos,
            velocity: Vec3Fixed::ZERO,
            combat: CombatState::new(1001, WeaponFamily::Blade),
            inventory,
            marks: 50,
            sovereigns: 1,
            equipped_weapon_item: None,
        };

        // Spawn a wild training wolf agent nearby
        let wolf_x = Q64::from_int(70)?;
        let wolf_z = Q64::from_int(66)?;
        let wolf_y = terrain.elevation_at(wolf_x, wolf_z)?;
        let wolf_pos = Vec3Fixed::new(wolf_x, wolf_y, wolf_z);

        let wolf = NpcEntity {
            entity_id: 2001,
            name: "Direwolf Stalker".to_string(),
            position: wolf_pos,
            combat: CombatState::new(2001, WeaponFamily::Blade),
            capability: AgentCapabilityEnvelope {
                agent_id: 2001,
                max_spend_per_epoch: 100,
                current_spent_in_epoch: 0,
                allowed_action_mask: AgentCapabilityEnvelope::ACTION_MOVE | AgentCapabilityEnvelope::ACTION_HARVEST,
                valid_until_epoch: 1000,
                revoked: false,
            },
            is_hostile: true,
        };

        let mut npcs = BTreeMap::new();
        npcs.insert(wolf.entity_id, wolf);

        let mut sim = WorldSimulation {
            genesis_seed,
            terrain,
            current_tick: 1,
            player,
            npcs,
            resource_nodes: BTreeMap::new(),
            smt: SparseMerkleTree::new(),
            thermodynamic_ledger: ThermodynamicLedger::new(100_000_000_000), // 100 tonnes initial world mass
            pity_engine: PityLootEngine::new(),
            halo_calculator: DynamicHaloCalculator::new(
                Q64::from_int(2)?,
                Q64::from_int(1)?,
            ),
            last_state_root: [0u8; 32],
        };

        // Discover initial mineral vein in spawn chunk
        let spawn_chunk = ChunkCoord::from_world_pos(spawn_pos);
        if let Some(node) = sim.terrain.mineral_vein_for_chunk(spawn_chunk)? {
            sim.resource_nodes.insert(node.node_id, node);
        }

        // Commit initial state to SMT
        sim.recompute_merkle_root()?;
        Ok(sim)
    }

    /// Recompute canonical Sparse Merkle Tree (SMT) state root.
    /// Commits player state, deterministic inventory/currency digest, NPCs, and resource veins (R2 §11).
    pub fn recompute_merkle_root(&mut self) -> Result<Hash, MathError> {
        let mut inv_hasher = Sha256::new();
        inv_hasher.update(b"AEVITERRA_INVENTORY_V1");
        inv_hasher.update(self.player.marks.to_be_bytes());
        inv_hasher.update(self.player.sovereigns.to_be_bytes());
        for (&res_id, &count) in &self.player.inventory {
            inv_hasher.update(res_id.to_be_bytes());
            inv_hasher.update(count.to_be_bytes());
        }
        let player_inv_root: Hash = inv_hasher.finalize().into();

        // Update player leaf
        let p_record = EntityRecord {
            entity_id: self.player.entity_id,
            authority_shard: 1,
            ownership_version: self.current_tick,
            mass_microunits: 85_000_000, // 85kg
            position_x: self.player.position.x.to_raw(),
            position_y: self.player.position.y.to_raw(),
            position_z: self.player.position.z.to_raw(),
            velocity_x: self.player.velocity.x.to_raw(),
            velocity_y: self.player.velocity.y.to_raw(),
            velocity_z: self.player.velocity.z.to_raw(),
            inventory_root: player_inv_root,
            state_flags: self.player.combat.health,
        };
        self.smt.insert(self.player.entity_id, p_record.compute_leaf_hash());

        // Update NPC leaves
        for (&id, npc) in &self.npcs {
            let n_record = EntityRecord {
                entity_id: id,
                authority_shard: 1,
                ownership_version: self.current_tick,
                mass_microunits: 60_000_000,
                position_x: npc.position.x.to_raw(),
                position_y: npc.position.y.to_raw(),
                position_z: npc.position.z.to_raw(),
                velocity_x: 0,
                velocity_y: 0,
                velocity_z: 0,
                inventory_root: [0u8; 32],
                state_flags: npc.combat.health,
            };
            self.smt.insert(id, n_record.compute_leaf_hash());
        }

        // Commit resource vein depletion state into canonical SMT (F26)
        for (&node_id, node) in &self.resource_nodes {
            let mut node_bytes = Vec::with_capacity(28);
            node_bytes.extend_from_slice(&node.resource_id.to_be_bytes());
            node_bytes.extend_from_slice(&node.remaining_amount.to_be_bytes());
            node_bytes.extend_from_slice(&node.max_amount.to_be_bytes());
            node_bytes.extend_from_slice(&node.chunk.x.to_be_bytes());
            node_bytes.extend_from_slice(&node.chunk.z.to_be_bytes());
            self.smt.insert(node_id, hash_leaf(node_id, &node_bytes));
        }

        let root = self.smt.compute_root();
        self.last_state_root = root;
        Ok(root)
    }

    /// The Pure State Transition Function T(S_t, B_t, C_t) -> S_{t+1}.
    pub fn step(&mut self, intents: Vec<PlayerIntent>) -> Result<Vec<GameEvent>, MathError> {
        let mut events = Vec::new();
        self.current_tick += 1;

        // 1. Tick regeneration for combatants
        self.player.combat.tick_regeneration();
        for npc in self.npcs.values_mut() {
            if npc.combat.health > 0 {
                npc.combat.tick_regeneration();
            }
        }

        // Health check: dead players cannot move, attack, or craft (B01)
        if self.player.combat.health == 0 {
            events.push(GameEvent::PlayerDied { entity_id: self.player.entity_id });
            let root = self.recompute_merkle_root()?;
            events.push(GameEvent::StateRootUpdated {
                tick: self.current_tick,
                root,
            });
            return Ok(events);
        }

        // 2. Process player input intents
        // Multi-intent speed clamp per tick (B04)
        let mut total_dx = Q64::ZERO;
        let mut total_dz = Q64::ZERO;
        let mut has_move = false;

        for intent in &intents {
            if let PlayerIntent::Move { dx, dz } = intent {
                total_dx = total_dx.checked_add(*dx)?;
                total_dz = total_dz.checked_add(*dz)?;
                has_move = true;
            }
        }

        if has_move {
            let speed_limit = Q64::from_int(5)?; // Max 5 m/s per tick
            let clamped_dx = if total_dx > speed_limit { speed_limit } else if total_dx < -speed_limit { -speed_limit } else { total_dx };
            let clamped_dz = if total_dz > speed_limit { speed_limit } else if total_dz < -speed_limit { -speed_limit } else { total_dz };

            let new_x = self.player.position.x.checked_add(clamped_dx)?;
            let new_z = self.player.position.z.checked_add(clamped_dz)?;
            let new_y = self.terrain.elevation_at(new_x, new_z)?;

            self.player.position = Vec3Fixed::new(new_x, new_y, new_z);
            self.player.combat.is_blocking = false; // Moving cancels blocking (B03)

            events.push(GameEvent::EntityMoved {
                id: self.player.entity_id,
                new_pos: self.player.position,
            });

            // Check for newly discovered resource veins in new chunk
            let current_chunk = ChunkCoord::from_world_pos(self.player.position);
            if let Some(node) = self.terrain.mineral_vein_for_chunk(current_chunk)? {
                self.resource_nodes.entry(node.node_id).or_insert(node);
            }
        }

        for intent in intents {
            match intent {
                PlayerIntent::Move { .. } => {
                    // Handled above in batch-clamped movement
                }

                PlayerIntent::Attack(action) => {
                    if let Ok(stamina_cost) = self.player.combat.execute_action(action) {
                        events.push(GameEvent::CombatActionExecuted {
                            actor_id: self.player.entity_id,
                            action,
                            stamina_cost,
                        });

                        // Defensive actions (Block, DodgeRoll) do NOT emit hit sweep (B15)
                        if action != CombatActionType::Block && action != CombatActionType::DodgeRoll {
                            let attack_range = match self.player.combat.active_weapon {
                                WeaponFamily::Bow => Q64::from_int(18)?,
                                WeaponFamily::WandPrism => Q64::from_int(14)?,
                                WeaponFamily::Staff => Q64::from_int(8)?,
                                WeaponFamily::Spear => Q64::from_int(5)?,
                                _ => Q64::from_int(4)?,
                            };

                            let bonus_pct: u32 = match self.player.equipped_weapon_item {
                                Some(101) => 40, // Damascus Broadsword: +40%
                                Some(102) => 30, // Composite Bow: +30%
                                _ => 0,
                            };

                            let base_dmg = match action {
                                CombatActionType::LightAttack => match self.player.combat.active_weapon {
                                    WeaponFamily::Bow => 70,
                                    WeaponFamily::Staff => 60,
                                    _ => 80,
                                },
                                CombatActionType::HeavyAttack => match self.player.combat.active_weapon {
                                    WeaponFamily::Bow => 190,
                                    WeaponFamily::Staff => 175,
                                    _ => 220,
                                },
                                CombatActionType::SpecialSkill(_) => 160,
                                _ => 0,
                            };

                            let scaled_dmg = base_dmg + (base_dmg * bonus_pct / 100);

                            let mut hits = Vec::new();
                            for (&npc_id, npc) in &self.npcs {
                                if npc.combat.health > 0 {
                                    let dist = self.player.position.distance(npc.position)?;
                                    if dist <= attack_range {
                                        hits.push((npc_id, scaled_dmg));
                                    }
                                }
                            }

                            for (npc_id, dmg) in hits {
                                if let Some(npc) = self.npcs.get_mut(&npc_id) {
                                    let prev_hp = npc.combat.health;
                                    let effective_dmg = npc.combat.apply_incoming_damage(dmg);
                                    events.push(GameEvent::CombatDamageDealt {
                                        attacker: self.player.entity_id,
                                        target: npc_id,
                                        damage: effective_dmg,
                                        target_health: npc.combat.health,
                                    });

                                    // NPC slain: trigger pity engine drops & marks reward (B10)
                                    if prev_hp > 0 && npc.combat.health == 0 {
                                        self.player.marks += 25;
                                        events.push(GameEvent::NpcSlain {
                                            killer: self.player.entity_id,
                                            target: npc_id,
                                            reward_marks: 25,
                                        });

                                        // Organic loot drops via PityEngine
                                        let roll = (self.current_tick as u32 * 17) % 1_000_000;
                                        let _rare_drop = self.pity_engine.evaluate_tier_roll(
                                            self.player.entity_id,
                                            aeviterra_core::game::loot::LootTier::Rare,
                                            roll,
                                        );

                                        *self.player.inventory.entry(4).or_insert(0) += 2; // Direwolf Pelts
                                        *self.player.inventory.entry(5).or_insert(0) += 1; // Sharp Fang
                                        let current_pelts = self.thermodynamic_ledger.get_balance(self.player.entity_id, 4);
                                        self.thermodynamic_ledger.entity_balances.insert((self.player.entity_id, 4), current_pelts + 2);
                                        let current_fangs = self.thermodynamic_ledger.get_balance(self.player.entity_id, 5);
                                        self.thermodynamic_ledger.entity_balances.insert((self.player.entity_id, 5), current_fangs + 1);
                                    }
                                }
                            }
                        }
                    }
                }

                PlayerIntent::SwitchWeapon(family) => {
                    self.player.combat.active_weapon = family;
                }

                PlayerIntent::EquipItem(item_id) => {
                    let count = self.player.inventory.get(&item_id).copied().unwrap_or(0);
                    if count > 0 {
                        self.player.equipped_weapon_item = Some(item_id);
                        let name = match item_id {
                            101 => {
                                self.player.combat.active_weapon = WeaponFamily::Blade;
                                "Masterwork Damascus Broadsword"
                            }
                            102 => {
                                self.player.combat.active_weapon = WeaponFamily::Bow;
                                "Composite Recurve Bow"
                            }
                            103 => {
                                self.player.combat.active_weapon = WeaponFamily::Shield;
                                "Sturdy Kite Shield"
                            }
                            _ => "Unknown Gear",
                        };
                        events.push(GameEvent::ItemEquipped {
                            actor_id: self.player.entity_id,
                            item_id,
                            item_name: name,
                        });
                    }
                }

                PlayerIntent::Harvest(node_id) => {
                    self.player.combat.is_blocking = false;
                    if let Some(node) = self.resource_nodes.get_mut(&node_id) {
                        let dist = self.player.position.distance(node.world_pos)?;
                        let harvest_range = Q64::from_int(6)?;
                        if dist <= harvest_range && node.remaining_amount > 0 {
                            let harvest_amount = node.remaining_amount.min(10);
                            node.remaining_amount -= harvest_amount;
                            let count = self.player.inventory.entry(node.resource_id).or_insert(0);
                            *count += harvest_amount;

                            // Update thermodynamic mass ledger
                            let resource_name = match node.resource_id {
                                1 => "Iron Ore",
                                2 => "Oak Timber",
                                _ => "Silver",
                            };

                            let current_bal = self.thermodynamic_ledger.get_balance(self.player.entity_id, node.resource_id);
                            self.thermodynamic_ledger.entity_balances.insert(
                                (self.player.entity_id, node.resource_id),
                                current_bal + harvest_amount,
                            );

                            events.push(GameEvent::ResourceHarvested {
                                actor_id: self.player.entity_id,
                                resource_name,
                                amount: harvest_amount,
                            });
                        }
                    }
                }

                PlayerIntent::CraftSword => {
                    self.player.combat.is_blocking = false;
                    let iron_count = self.player.inventory.get(&1).copied().unwrap_or(0);
                    if iron_count >= 50 {
                        // Exact closed recipe: 50 Iron Ore -> 35 Damascus Steel Sword + 15 Slag
                        *self.player.inventory.get_mut(&1).unwrap() -= 50;
                        *self.player.inventory.entry(101).or_insert(0) += 1; // 101 = Damascus Broadsword

                        let _ = self.thermodynamic_ledger.execute_recipe(
                            self.player.entity_id,
                            1,  // input iron
                            50,
                            101, // output sword
                            35,
                            999, // residue slag
                            15,
                        );

                        events.push(GameEvent::ItemCrafted {
                            crafter_id: self.player.entity_id,
                            item_name: "Masterwork Damascus Broadsword",
                        });
                    }
                }

                PlayerIntent::CraftBow => {
                    self.player.combat.is_blocking = false;
                    let timber_count = self.player.inventory.get(&2).copied().unwrap_or(0);
                    if timber_count >= 30 {
                        // Closed recipe: 30 Oak Timber -> 20 Composite Bow + 10 Wood Shavings Slag
                        *self.player.inventory.get_mut(&2).unwrap() -= 30;
                        *self.player.inventory.entry(102).or_insert(0) += 1;

                        let _ = self.thermodynamic_ledger.execute_recipe(
                            self.player.entity_id,
                            2,  // input timber
                            30,
                            102, // output bow
                            20,
                            999, // residue slag
                            10,
                        );

                        events.push(GameEvent::ItemCrafted {
                            crafter_id: self.player.entity_id,
                            item_name: "Composite Recurve Bow",
                        });
                    }
                }

                PlayerIntent::CraftShield => {
                    self.player.combat.is_blocking = false;
                    let iron_count = self.player.inventory.get(&1).copied().unwrap_or(0);
                    let timber_count = self.player.inventory.get(&2).copied().unwrap_or(0);
                    if iron_count >= 20 && timber_count >= 20 {
                        *self.player.inventory.get_mut(&1).unwrap() -= 20;
                        *self.player.inventory.get_mut(&2).unwrap() -= 20;
                        *self.player.inventory.entry(103).or_insert(0) += 1;

                        let _ = self.thermodynamic_ledger.execute_recipe(
                            self.player.entity_id,
                            1,
                            20,
                            103,
                            15,
                            999,
                            5,
                        );
                        let _ = self.thermodynamic_ledger.execute_recipe(
                            self.player.entity_id,
                            2,
                            20,
                            103,
                            15,
                            999,
                            5,
                        );

                        events.push(GameEvent::ItemCrafted {
                            crafter_id: self.player.entity_id,
                            item_name: "Sturdy Kite Shield",
                        });
                    }
                }
            }
        }

        // 3. Simple AI Agent behavior for alive NPCs
        let player_pos = self.player.position;
        let player_is_alive = self.player.combat.health > 0;

        for npc in self.npcs.values_mut() {
            if npc.combat.health > 0 && npc.is_hostile {
                let dist = npc.position.distance(player_pos)?;
                let aggro_radius = Q64::from_int(12)?; // 12m aggro
                let attack_radius = Q64::from_int(3)?; // 3m bite

                if player_is_alive && dist <= attack_radius {
                    // Attack living player
                    let raw_dmg = 45;
                    let dmg = self.player.combat.apply_incoming_damage(raw_dmg);
                    events.push(GameEvent::CombatDamageDealt {
                        attacker: npc.entity_id,
                        target: self.player.entity_id,
                        damage: dmg,
                        target_health: self.player.combat.health,
                    });
                } else if player_is_alive && dist <= aggro_radius {
                    // Move toward living player
                    let dir = player_pos.checked_sub(npc.position)?.normalize()?;
                    let step_speed = Q64::from_int(2)?; // 2 m/s
                    let step_x = dir.x.checked_mul(step_speed)?;
                    let step_z = dir.z.checked_mul(step_speed)?;

                    let nx = npc.position.x.checked_add(step_x)?;
                    let nz = npc.position.z.checked_add(step_z)?;
                    let ny = self.terrain.elevation_at(nx, nz)?;
                    npc.position = Vec3Fixed::new(nx, ny, nz);
                }
            }
        }

        // 4. Update Merkle tree and emit new canonical root
        let root = self.recompute_merkle_root()?;
        events.push(GameEvent::StateRootUpdated {
            tick: self.current_tick,
            root,
        });

        Ok(events)
    }
}
