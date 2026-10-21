use aeviterra_cli::simulation::*;
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::game::combat::CombatActionType;

/// CHAOS MONKEY ADVERSARIAL TEST SUITE
/// Verifies architectural security, exploit mitigation, and state machine integrity.

#[test]
fn test_chaos_01_zombie_avatar_ghost_mining_and_crafting() {
    let mut sim = WorldSimulation::new("ZombiePioneer".to_string()).unwrap();

    // Kill the player (set HP to 0)
    sim.player.combat.health = 0;
    assert_eq!(sim.player.combat.health, 0);

    // 1. Can a corpse walk? (B01 Fixed: Dead players cannot move)
    let pre_pos = sim.player.position;
    let move_res = sim.step(vec![PlayerIntent::Move {
        dx: Q64::from_int(3).unwrap(),
        dz: Q64::from_int(3).unwrap(),
    }]);
    assert!(move_res.is_ok());
    let post_pos = sim.player.position;
    assert_eq!(pre_pos.x, post_pos.x, "Dead corpse cannot walk around the world!");
    let died_event = move_res.unwrap().iter().any(|e| matches!(e, GameEvent::PlayerDied { .. }));
    assert!(died_event, "Emits PlayerDied event on attempt to act while dead");

    // 2. Can a corpse harvest resources? (B01 Fixed: Dead players cannot harvest)
    let node_id = sim.resource_nodes.keys().next().copied().unwrap();
    sim.player.position = sim.resource_nodes.get(&node_id).unwrap().world_pos;
    let harvest_res = sim.step(vec![PlayerIntent::Harvest(node_id)]).unwrap();
    let harvested = harvest_res.iter().any(|e| matches!(e, GameEvent::ResourceHarvested { .. }));
    assert!(!harvested, "Corpse cannot harvest resources from nodes!");
    assert_eq!(sim.player.inventory.get(&1).copied().unwrap_or(0), 0);

    // 3. Can a corpse craft masterwork weapons? (B01 Fixed: Dead players cannot craft)
    sim.player.inventory.insert(1, 100);
    sim.thermodynamic_ledger.entity_balances.insert((sim.player.entity_id, 1), 100);
    let craft_res = sim.step(vec![PlayerIntent::CraftSword]).unwrap();
    let crafted = craft_res.iter().any(|e| matches!(e, GameEvent::ItemCrafted { .. }));
    assert!(!crafted, "Corpse cannot forge masterwork swords!");
    assert_eq!(sim.player.inventory.get(&101).copied().unwrap_or(0), 0);
}

#[test]
fn test_chaos_02_infinite_iframes_stamina_positive_loophole() {
    let mut sim = WorldSimulation::new("ImmortalRoller".to_string()).unwrap();

    // Spawn an aggressive NPC directly in melee range
    let npc_id = 2001;
    sim.npcs.get_mut(&npc_id).unwrap().position = sim.player.position;

    // Roll once: spends 25 stamina, gains 10 ticks of invulnerability
    let initial_hp = sim.player.combat.health;
    sim.step(vec![PlayerIntent::Attack(CombatActionType::DodgeRoll)]).unwrap();
    assert_eq!(sim.player.combat.iframe_remaining_ticks, 10);
    assert_eq!(sim.player.combat.stamina, 75);

    // During i-frames, stamina regeneration is strictly FROZEN (B02 Fixed: strictly stamina-negative)
    for _ in 0..5 {
        sim.step(vec![]).unwrap();
    }
    // Player took 0 damage despite NPC biting in melee range
    assert_eq!(sim.player.combat.health, initial_hp);
    // Stamina did NOT regenerate during roll (remains 75, not >= 95)
    assert_eq!(sim.player.combat.stamina, 75, "Stamina recovery is frozen during active roll i-frames");
    assert!(sim.player.combat.iframe_remaining_ticks > 0);
}

#[test]
fn test_chaos_03_permanent_zero_cost_shield_block() {
    let mut sim = WorldSimulation::new("TurtlingKnight".to_string()).unwrap();

    // Block once: costs 10 stamina, sets is_blocking = true
    sim.step(vec![PlayerIntent::Attack(CombatActionType::Block)]).unwrap();
    assert!(sim.player.combat.is_blocking);

    // Move 1 tick across the world
    sim.step(vec![PlayerIntent::Move { dx: Q64::from_int(1).unwrap(), dz: Q64::ZERO }]).unwrap();

    // B03 Fixed: Movement cancels defensive blocking posture!
    assert!(!sim.player.combat.is_blocking, "Movement cancels shield block posture!");
}

#[test]
fn test_chaos_04_multi_intent_single_tick_speed_burst() {
    let mut sim = WorldSimulation::new("SpeedHacker".to_string()).unwrap();
    let start_x = sim.player.position.x;

    // In a single tick, send 10 Move intents of 5m each
    let intents = vec![
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
        PlayerIntent::Move { dx: Q64::from_int(5).unwrap(), dz: Q64::ZERO },
    ];

    sim.step(intents).unwrap();
    let end_x = sim.player.position.x;
    let delta = (end_x - start_x).to_int_trunc();

    // B04 Fixed: Multi-intent batching is strictly clamped to max 5m per tick!
    assert_eq!(delta, 5, "Multi-intent movement clamped to 5 m/s maximum displacement per tick");
}

#[test]
fn test_chaos_05_gathering_from_nonexistent_node() {
    let mut sim = WorldSimulation::new("GhostMiner".to_string()).unwrap();

    // Harvest a non-existent node ID
    let events = sim.step(vec![PlayerIntent::Harvest(99999999)]).unwrap();
    // Silently succeeds with 0 events, 0 errors, advancing tick
    assert!(events.iter().all(|e| !matches!(e, GameEvent::ResourceHarvested { .. })));
}

#[test]
fn test_chaos_06_dead_npc_remains_immortal_corpse_in_tree() {
    let mut sim = WorldSimulation::new("WolfSlayer".to_string()).unwrap();

    let wolf_id = 2001;
    // Set wolf health to 0
    sim.npcs.get_mut(&wolf_id).unwrap().combat.health = 0;

    // Step simulation
    sim.step(vec![]).unwrap();

    // Wolf is still present in memory and SMT
    assert!(sim.npcs.contains_key(&wolf_id));
    assert_eq!(sim.npcs.get(&wolf_id).unwrap().combat.health, 0);
    // No loot dropped, no marks awarded, no death event emitted
    assert_eq!(sim.player.marks, 50);
}
