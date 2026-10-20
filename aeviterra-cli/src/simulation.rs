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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    EntityMoved { id: u64, new_pos: Vec3Fixed },
    CombatActionExecuted { actor_id: u64, action: CombatActionType, stamina_cost: u32 },
    CombatDamageDealt { attacker: u64, target: u64, damage: u32, target_health: u32 },
    ResourceHarvested { actor_id: u64, resource_name: &'static str, amount: u64 },
    ItemCrafted { crafter_id: u64, item_name: &'static str },
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

        let player = PlayerAvatar {
            entity_id: 1001,
            name: player_name,
            position: spawn_pos,
            velocity: Vec3Fixed::ZERO,
            combat: CombatState::new(1001, WeaponFamily::Blade),
            inventory,
            marks: 50,
            sovereigns: 1,
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
            npc.combat.tick_regeneration();
        }

        // 2. Process player input intents
        for intent in intents {
            match intent {
                PlayerIntent::Move { dx, dz } => {
                    let speed_limit = Q64::from_int(5)?; // Max 5 m/s
                    let clamped_dx = if dx > speed_limit { speed_limit } else if dx < -speed_limit { -speed_limit } else { dx };
                    let clamped_dz = if dz > speed_limit { speed_limit } else if dz < -speed_limit { -speed_limit } else { dz };

                    let new_x = self.player.position.x.checked_add(clamped_dx)?;
                    let new_z = self.player.position.z.checked_add(clamped_dz)?;
                    let new_y = self.terrain.elevation_at(new_x, new_z)?;

                    self.player.position = Vec3Fixed::new(new_x, new_y, new_z);
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

                PlayerIntent::Attack(action) => {
                    if let Ok(stamina_cost) = self.player.combat.execute_action(action) {
                        events.push(GameEvent::CombatActionExecuted {
                            actor_id: self.player.entity_id,
                            action,
                            stamina_cost,
                        });

                        // Sweep check for hits against nearby hostile NPCs
                        let attack_range = Q64::from_int(4)?; // 4m melee range
                        let mut hits = Vec::new();

                        for (&npc_id, npc) in &self.npcs {
                            if npc.combat.health > 0 {
                                let dist = self.player.position.distance(npc.position)?;
                                if dist <= attack_range {
                                    let raw_damage = match action {
                                        CombatActionType::LightAttack => 80,
                                        CombatActionType::HeavyAttack => 220,
                                        CombatActionType::Block => 0,
                                        CombatActionType::DodgeRoll => 0,
                                        CombatActionType::SpecialSkill(_) => 150,
                                    };
                                    hits.push((npc_id, raw_damage));
                                }
                            }
                        }

                        for (npc_id, dmg) in hits {
                            if let Some(npc) = self.npcs.get_mut(&npc_id) {
                                let effective_dmg = npc.combat.apply_incoming_damage(dmg);
                                events.push(GameEvent::CombatDamageDealt {
                                    attacker: self.player.entity_id,
                                    target: npc_id,
                                    damage: effective_dmg,
                                    target_health: npc.combat.health,
                                });
                            }
                        }
                    }
                }

                PlayerIntent::SwitchWeapon(family) => {
                    self.player.combat.active_weapon = family;
                }

                PlayerIntent::Harvest(node_id) => {
                    if let Some(node) = self.resource_nodes.get_mut(&node_id) {
                        let dist = self.player.position.distance(node.world_pos)?;
                        let harvest_range = Q64::from_int(6)?;
                        if dist <= harvest_range && node.remaining_amount >= 10 {
                            node.remaining_amount -= 10;
                            let count = self.player.inventory.entry(node.resource_id).or_insert(0);
                            *count += 10;

                            // Update thermodynamic mass ledger
                            let resource_name = match node.resource_id {
                                1 => "Iron Ore",
                                2 => "Oak Timber",
                                _ => "Silver",
                            };

                            let current_bal = self.thermodynamic_ledger.get_balance(self.player.entity_id, node.resource_id);
                            self.thermodynamic_ledger.entity_balances.insert(
                                (self.player.entity_id, node.resource_id),
                                current_bal + 10,
                            );

                            events.push(GameEvent::ResourceHarvested {
                                actor_id: self.player.entity_id,
                                resource_name,
                                amount: 10,
                            });
                        }
                    }
                }

                PlayerIntent::CraftSword => {
                    let iron_count = self.player.inventory.get(&1).copied().unwrap_or(0);
                    if iron_count >= 50 {
                        // Exact closed recipe: 50 Iron Ore -> 35 Damascus Steel Sword + 15 Slag
                        *self.player.inventory.get_mut(&1).unwrap() -= 50;
                        *self.player.inventory.entry(101).or_insert(0) += 1; // 101 = Damascus Broadsword

                        // Execute on thermodynamic ledger (verifies zero mass leak!)
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
            }
        }

        // 3. Simple AI Agent behavior for alive NPCs
        let player_pos = self.player.position;
        for npc in self.npcs.values_mut() {
            if npc.combat.health > 0 && npc.is_hostile {
                let dist = npc.position.distance(player_pos)?;
                let aggro_radius = Q64::from_int(12)?; // 12m aggro
                let attack_radius = Q64::from_int(3)?; // 3m bite

                if dist <= attack_radius {
                    // Attack player
                    let raw_dmg = 45;
                    let dmg = self.player.combat.apply_incoming_damage(raw_dmg);
                    events.push(GameEvent::CombatDamageDealt {
                        attacker: npc.entity_id,
                        target: self.player.entity_id,
                        damage: dmg,
                        target_health: self.player.combat.health,
                    });
                } else if dist <= aggro_radius {
                    // Move toward player
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
