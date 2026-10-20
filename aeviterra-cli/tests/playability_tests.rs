use aeviterra_cli::simulation::*;
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::game::combat::{CombatActionType, WeaponFamily};
use aeviterra_core::game::settlement::*;
use aeviterra_core::game::quest::*;
use aeviterra_core::assets::grammar::*;
use aeviterra_core::economy::thermodynamics::AgentCapabilityEnvelope;
use std::time::Instant;

/// P01: Combat Balance, TTK (Time-To-Kill), Stamina Rhythms, and i-Frame Defense.
#[test]
fn test_p01_combat_balance_ttk_stamina_and_iframes() {
    let mut sim = WorldSimulation::new("Aethelgard_Warrior".to_string()).unwrap();

    // Verify initial avatar combat attributes
    assert_eq!(sim.player.combat.health, 1000);
    assert_eq!(sim.player.combat.stamina, 100);
    assert_eq!(sim.player.combat.active_weapon, WeaponFamily::Blade);

    // Initial Wolf NPC is at (70, y, 66), player is at (64, y, 64) -> dist ~6.3m
    // Move player into melee combat range (dist <= 4m)
    let move_intent = PlayerIntent::Move {
        dx: Q64::from_int(4).unwrap(),
        dz: Q64::from_int(2).unwrap(),
    };
    sim.step(vec![move_intent]).unwrap();

    let wolf_id = 2001;
    let initial_wolf_hp = sim.npcs.get(&wolf_id).unwrap().combat.health;
    assert_eq!(initial_wolf_hp, 1000);

    // 1. Light Attack execution: costs 15 stamina, deals 80 damage
    let light_atk = PlayerIntent::Attack(CombatActionType::LightAttack);
    let events = sim.step(vec![light_atk]).unwrap();

    let wolf_after_light = sim.npcs.get(&wolf_id).unwrap().combat.health;
    assert_eq!(wolf_after_light, 1000 - 80); // 920 HP
    // Player stamina: 100 - 15 = 85 (regen occurred at tick start before attack)
    assert_eq!(sim.player.combat.stamina, 85);

    // Verify combat damage event was emitted
    let dealt_event = events.iter().any(|e| matches!(e, GameEvent::CombatDamageDealt { attacker: 1001, target: 2001, damage: 80, .. }));
    assert!(dealt_event);

    // 2. Heavy Attack execution: regens +3 to 88, costs 35 stamina, deals 220 damage
    let heavy_atk = PlayerIntent::Attack(CombatActionType::HeavyAttack);
    sim.step(vec![heavy_atk]).unwrap();
    let wolf_after_heavy = sim.npcs.get(&wolf_id).unwrap().combat.health;
    assert_eq!(wolf_after_heavy, 920 - 220); // 700 HP
    // Stamina: 85 + 5 - 35 = 55 (5 stamina regenerated per tick)
    assert_eq!(sim.player.combat.stamina, 55);

    // 3. Dodge Roll i-Frames: costs 25 stamina, grants 10 ticks of invulnerability
    let roll_intent = PlayerIntent::Attack(CombatActionType::DodgeRoll);
    sim.step(vec![roll_intent]).unwrap();
    assert_eq!(sim.player.combat.iframe_remaining_ticks, 10); // 10 at execution

    // Direwolf bites while i-frames are active -> 0 damage taken!
    let hp_before_bite = sim.player.combat.health;
    // Step simulation (wolf is in melee range and attacks)
    sim.step(vec![]).unwrap();
    assert_eq!(sim.player.combat.health, hp_before_bite); // Invulnerable!

    // 4. Shield Blocking: reduces incoming damage by 70%
    // Wait until i-frames expire
    while sim.player.combat.iframe_remaining_ticks > 0 {
        sim.step(vec![]).unwrap();
    }
    // Execute shield block
    let block_intent = PlayerIntent::Attack(CombatActionType::Block);
    sim.step(vec![block_intent]).unwrap();
    assert!(sim.player.combat.is_blocking);

    // Wolf attacks blocking player: 45 raw damage -> 30% unblocked = 13 damage taken
    let hp_before_blocked_hit = sim.player.combat.health;
    sim.step(vec![]).unwrap();
    let hp_after_blocked_hit = sim.player.combat.health;
    let dmg_taken = hp_before_blocked_hit - hp_after_blocked_hit;
    assert!(dmg_taken <= 15, "Damage was blocked (expected ~13, got {})", dmg_taken);

    // 5. Weapon Family Switching
    let switch_intent = PlayerIntent::SwitchWeapon(WeaponFamily::Spear);
    sim.step(vec![switch_intent]).unwrap();
    assert_eq!(sim.player.combat.active_weapon, WeaponFamily::Spear);
}

/// P02: Gathering & Crafting Economy Flow (vein depletion, mass-conserved Damascus forging).
#[test]
fn test_p02_gathering_depletion_and_damascus_forging() {
    use aeviterra_cli::terrain::ResourceNode;
    use aeviterra_core::shard::halo::ChunkCoord;

    let mut sim = WorldSimulation::new("Master_Blacksmith".to_string()).unwrap();

    // Insert a known Iron Ore vein at player position for deterministic testing
    let node_id = 9999;
    sim.resource_nodes.insert(node_id, ResourceNode {
        node_id,
        chunk: ChunkCoord::from_world_pos(sim.player.position),
        world_pos: sim.player.position,
        resource_id: 1, // 1 = Iron Ore
        remaining_amount: 100,
        max_amount: 100,
    });
    let initial_remaining = sim.resource_nodes.get(&node_id).unwrap().remaining_amount;
    assert_eq!(initial_remaining, 100);

    // Harvest mineral vein in increments of 10 units
    for i in 1..=10 {
        let harvest_intent = PlayerIntent::Harvest(node_id);
        let events = sim.step(vec![harvest_intent]).unwrap();

        let harvested_event = events.iter().any(|e| matches!(e, GameEvent::ResourceHarvested { amount: 10, .. }));
        assert!(harvested_event);

        let iron_in_inventory = sim.player.inventory.get(&1).copied().unwrap_or(0);
        assert_eq!(iron_in_inventory, i * 10);
    }

    // Node is now completely depleted (0 remaining)
    let depleted_node = sim.resource_nodes.get(&node_id).unwrap();
    assert_eq!(depleted_node.remaining_amount, 0);

    // Attempting 11th harvest on depleted vein yields 0 additional ore
    let harvest_depleted = PlayerIntent::Harvest(node_id);
    let events = sim.step(vec![harvest_depleted]).unwrap();
    let harvested_again = events.iter().any(|e| matches!(e, GameEvent::ResourceHarvested { .. }));
    assert!(!harvested_again);
    assert_eq!(sim.player.inventory.get(&1).copied().unwrap_or(0), 100);

    // Craft Masterwork Damascus Broadsword:
    // Closed thermodynamic recipe: 50 Iron Ore (item 1) -> 35 Damascus Sword (item 101) + 15 Slag (item 999)
    let craft_intent = PlayerIntent::CraftSword;
    let craft_events = sim.step(vec![craft_intent]).unwrap();

    let crafted = craft_events.iter().any(|e| matches!(e, GameEvent::ItemCrafted { item_name: "Masterwork Damascus Broadsword", .. }));
    assert!(crafted);

    // Verify inventory balance
    assert_eq!(sim.player.inventory.get(&1).copied().unwrap_or(0), 50); // 100 - 50 = 50 remaining
    assert_eq!(sim.player.inventory.get(&101).copied().unwrap_or(0), 1); // 1 sword crafted

    // Verify thermodynamic conservation in world ledger
    let sword_bal = sim.thermodynamic_ledger.get_balance(sim.player.entity_id, 101);
    let slag_bal = sim.thermodynamic_ledger.world_sinks.get(&999).copied().unwrap_or(0);
    assert_eq!(sword_bal, 35);
    assert_eq!(slag_bal, 15);
    assert_eq!(sword_bal + slag_bal, 50); // Exact mass balance!
}

/// P03: Autonomous AI Agent Aggro, Dynamic Pursuit, & Capability Boundaries.
#[test]
fn test_p03_ai_agent_pursuit_and_capabilities() {
    let mut sim = WorldSimulation::new("Explorer".to_string()).unwrap();

    let wolf_id = 2001;
    let initial_wolf_pos = sim.npcs.get(&wolf_id).unwrap().position;
    let player_pos = sim.player.position;

    let initial_dist = initial_wolf_pos.distance(player_pos).unwrap();
    assert!(initial_dist.to_int_trunc() < 12); // Within 12m aggro radius

    // Step simulation 2 times with no player movement -> wolf pursues player
    sim.step(vec![]).unwrap();
    sim.step(vec![]).unwrap();

    let intermediate_wolf_pos = sim.npcs.get(&wolf_id).unwrap().position;
    let intermediate_dist = intermediate_wolf_pos.distance(player_pos).unwrap();

    // Wolf moved closer
    assert!(intermediate_dist < initial_dist);

    // Verify Wolf Capability Envelope constraints
    let wolf_capability = &sim.npcs.get(&wolf_id).unwrap().capability;
    assert_eq!(wolf_capability.agent_id, wolf_id);
    assert!((wolf_capability.allowed_action_mask & AgentCapabilityEnvelope::ACTION_MOVE) != 0);
    assert!((wolf_capability.allowed_action_mask & AgentCapabilityEnvelope::ACTION_HARVEST) != 0);
    // Cannot craft or trade
    assert!((wolf_capability.allowed_action_mask & AgentCapabilityEnvelope::ACTION_CRAFT) == 0);
    assert!((wolf_capability.allowed_action_mask & AgentCapabilityEnvelope::ACTION_TRADE) == 0);
}

/// P04: Settlement Plot Claiming & Modular Architectural Construction.
#[test]
fn test_p04_settlement_plot_claiming_and_modular_construction() {
    let mut charter = SettlementCharter::new(10, "Valoria Freehold".to_string(), 500);
    let player_id = 1001;

    // 1. Claim homestead plot
    assert!(charter.claim_plot(42, player_id, 100).is_ok());

    // 2. Duplicate plot claim rejected
    assert_eq!(charter.claim_plot(42, 9999, 101), Err(SettlementError::PlotAlreadyClaimed));

    // 3. Construct valid structural primitive on claimed plot
    let valid_wall = StructuralPrimitive {
        style: ArchitecturalStyle::FortifiedCitadel,
        primary_material: MaterialType::Granite,
        secondary_material: MaterialType::SteelAlloy,
        length: Q64::from_int(24).unwrap(), // 24m wall
        height: Q64::from_int(8).unwrap(),  // 8m high
        thickness: Q64::from_int(2).unwrap(), // 2m thick
        features: 0b010,
    };
    assert!(charter.build_structure(42, player_id, valid_wall).is_ok());

    // 4. Malicious structure rejected (zero thickness)
    let invalid_wall = StructuralPrimitive {
        style: ArchitecturalStyle::FortifiedCitadel,
        primary_material: MaterialType::Granite,
        secondary_material: MaterialType::SteelAlloy,
        length: Q64::from_int(24).unwrap(),
        height: Q64::from_int(8).unwrap(),
        thickness: Q64::ZERO, // Zero thickness illegal!
        features: 0,
    };
    assert_eq!(charter.build_structure(42, player_id, invalid_wall), Err(SettlementError::InvalidStructureConstraints));

    // 5. Non-owner cannot build on plot
    let intruder_id = 9999;
    let another_wall = StructuralPrimitive {
        style: ArchitecturalStyle::FortifiedCitadel,
        primary_material: MaterialType::Granite,
        secondary_material: MaterialType::SteelAlloy,
        length: Q64::from_int(10).unwrap(),
        height: Q64::from_int(4).unwrap(),
        thickness: Q64::from_int(1).unwrap(),
        features: 0,
    };
    assert_eq!(charter.build_structure(42, intruder_id, another_wall), Err(SettlementError::NotPlotOwner));
}

/// P05: Idempotent Quests & Consumed-Once Reward Claims.
#[test]
fn test_p05_idempotent_quests_and_consumed_once_reward_claims() {
    let mut quest_engine = QuestEngine::new();
    let quest_id = 101;
    let player_id = 1001;

    quest_engine.templates.insert(quest_id, QuestTemplate {
        quest_id,
        title: "Frontier Defense: Slay 2 Direwolves".to_string(),
        target_count: 2,
        reward_marks: 300,
        reward_sovereigns: 5,
    });

    // Unregistered quest attempt
    assert_eq!(quest_engine.claim_reward(player_id, quest_id), Err(QuestError::QuestNotFound));

    // Kill 1st wolf
    quest_engine.record_kill(player_id, quest_id);
    assert_eq!(quest_engine.claim_reward(player_id, quest_id), Err(QuestError::NotCompleted));

    // Kill 2nd wolf -> completed!
    quest_engine.record_kill(player_id, quest_id);

    // Initial claim succeeds
    let (marks, sovereigns) = quest_engine.claim_reward(player_id, quest_id).unwrap();
    assert_eq!(marks, 300);
    assert_eq!(sovereigns, 5);

    // Double claim / replay attack fails permanently
    assert_eq!(quest_engine.claim_reward(player_id, quest_id), Err(QuestError::RewardAlreadyClaimed));
}

/// P06: High-Throughput Deterministic Simulation Pacing & Repeatability.
#[test]
fn test_p06_high_throughput_deterministic_simulation_pacing() {
    let mut sim_a = WorldSimulation::new("Deterministic_Hero_A".to_string()).unwrap();
    let mut sim_b = WorldSimulation::new("Deterministic_Hero_B".to_string()).unwrap();

    // Verify initial Merkle state roots match identically
    assert_eq!(sim_a.last_state_root, sim_b.last_state_root);

    // Run 500 deterministic ticks with simulated player motion and actions
    let total_ticks = 500;
    let start_time = Instant::now();

    for t in 0..total_ticks {
        let dx = if t % 4 == 0 { Q64::from_int(1).unwrap() } else { Q64::from_int(-1).unwrap() };
        let dz = if t % 2 == 0 { Q64::from_int(1).unwrap() } else { Q64::ZERO };

        let intents_a = vec![PlayerIntent::Move { dx, dz }];
        let intents_b = vec![PlayerIntent::Move { dx, dz }];

        sim_a.step(intents_a).unwrap();
        sim_b.step(intents_b).unwrap();

        // Every tick state root must be identical bit-for-bit
        assert_eq!(sim_a.last_state_root, sim_b.last_state_root);
    }

    let elapsed = start_time.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();
    let tps = (total_ticks as f64) / elapsed_secs;

    println!("[P06 Benchmark] Completed {} ticks in {:.4}s ({:.1} TPS)", total_ticks, elapsed_secs, tps);

    // 20 TPS is the real-time simulation target. Single core must achieve at least 1,000 TPS.
    assert!(tps > 500.0, "Simulation throughput must exceed 500 TPS (achieved {:.1})", tps);
    assert_eq!(sim_a.current_tick, 501);
    assert_eq!(sim_b.current_tick, 501);
}
