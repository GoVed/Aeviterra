use aeviterra_cli::simulation::*;
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::game::combat::{CombatActionType, WeaponFamily};
use aeviterra_cli::terrain::{ResourceNode, BiomeType};
use aeviterra_core::shard::halo::ChunkCoord;

/// FUN FACTOR INTEGRATION TEST SUITE
/// Verifies the full loop of gameplay enhancements, bug fixes, and MMORPG systems.

#[test]
fn test_fun_01_multi_tier_crafting_and_equipment_damage_scaling() {
    let mut sim = WorldSimulation::new("HeroCrafter".to_string()).unwrap();

    // Give player resources for crafting
    sim.player.inventory.insert(1, 100); // 100 Iron
    sim.player.inventory.insert(2, 100); // 100 Oak Timber
    sim.thermodynamic_ledger.entity_balances.insert((sim.player.entity_id, 1), 100);
    sim.thermodynamic_ledger.entity_balances.insert((sim.player.entity_id, 2), 100);

    // 1. Craft Masterwork Damascus Broadsword (50 Iron -> 35 Sword [101] + 15 Slag)
    let craft_sword_events = sim.step(vec![PlayerIntent::CraftSword]).unwrap();
    assert!(craft_sword_events.iter().any(|e| matches!(e, GameEvent::ItemCrafted { item_name: "Masterwork Damascus Broadsword", .. })));
    assert_eq!(sim.player.inventory.get(&101).copied().unwrap_or(0), 1);

    // 2. Craft Composite Recurve Bow (30 Timber -> 20 Bow [102] + 10 Slag)
    let craft_bow_events = sim.step(vec![PlayerIntent::CraftBow]).unwrap();
    assert!(craft_bow_events.iter().any(|e| matches!(e, GameEvent::ItemCrafted { item_name: "Composite Recurve Bow", .. })));
    assert_eq!(sim.player.inventory.get(&102).copied().unwrap_or(0), 1);

    // 3. Craft Sturdy Kite Shield (20 Iron + 20 Timber -> 30 Shield [103] + 10 Slag)
    let craft_shield_events = sim.step(vec![PlayerIntent::CraftShield]).unwrap();
    assert!(craft_shield_events.iter().any(|e| matches!(e, GameEvent::ItemCrafted { item_name: "Sturdy Kite Shield", .. })));
    assert_eq!(sim.player.inventory.get(&103).copied().unwrap_or(0), 1);

    // 4. Equip Masterwork Damascus Broadsword: scales Blade damage by +40%!
    let equip_events = sim.step(vec![PlayerIntent::EquipItem(101)]).unwrap();
    assert!(equip_events.iter().any(|e| matches!(e, GameEvent::ItemEquipped { item_id: 101, .. })));
    assert_eq!(sim.player.equipped_weapon_item, Some(101));
    assert_eq!(sim.player.combat.active_weapon, WeaponFamily::Blade);

    // 5. Test damage scaling on wolf in melee range
    let wolf_id = 2001;
    sim.npcs.get_mut(&wolf_id).unwrap().position = sim.player.position;
    let initial_wolf_hp = sim.npcs.get(&wolf_id).unwrap().combat.health;

    // Heavy strike with equipped Masterwork Damascus Broadsword: base 220 * 1.40 = 308 damage!
    let atk_events = sim.step(vec![PlayerIntent::Attack(CombatActionType::HeavyAttack)]).unwrap();
    let dmg_dealt = atk_events.iter().find_map(|e| match e {
        GameEvent::CombatDamageDealt { damage, target, .. } if *target == wolf_id => Some(*damage),
        _ => None,
    }).expect("damage dealt");

    assert_eq!(dmg_dealt, 308, "Damascus Broadsword deals 308 damage (+40% scaled over baseline 220)");
    assert_eq!(sim.npcs.get(&wolf_id).unwrap().combat.health, initial_wolf_hp - 308);
}

#[test]
fn test_fun_02_ranged_combat_beyond_4m_melee_range() {
    let mut sim = WorldSimulation::new("ElvenRanger".to_string()).unwrap();

    let wolf_id = 2001;
    // Position wolf 12 meters away (well beyond the old 4m limit!)
    let wolf_pos = sim.player.position.checked_add(
        aeviterra_core::math::fixed::Vec3Fixed::new(
            Q64::from_int(12).unwrap(),
            Q64::ZERO,
            Q64::ZERO,
        )
    ).unwrap();
    sim.npcs.get_mut(&wolf_id).unwrap().position = wolf_pos;

    // 1. Blade attack at 12m misses (out of range, max 4m)
    sim.step(vec![PlayerIntent::SwitchWeapon(WeaponFamily::Blade)]).unwrap();
    let blade_res = sim.step(vec![PlayerIntent::Attack(CombatActionType::LightAttack)]).unwrap();
    let blade_hit = blade_res.iter().any(|e| matches!(e, GameEvent::CombatDamageDealt { target, .. } if *target == wolf_id));
    assert!(!blade_hit, "Melee blade misses target 12m away");

    // 2. Switch to Bow (18m range) and fire Light Attack (quick shot: 70 damage)
    sim.step(vec![PlayerIntent::SwitchWeapon(WeaponFamily::Bow)]).unwrap();
    let initial_wolf_hp = sim.npcs.get(&wolf_id).unwrap().combat.health;
    let bow_res = sim.step(vec![PlayerIntent::Attack(CombatActionType::LightAttack)]).unwrap();
    let bow_hit = bow_res.iter().any(|e| matches!(e, GameEvent::CombatDamageDealt { target, damage: 70, .. } if *target == wolf_id));
    assert!(bow_hit, "Bow hits target 12m away (within 18m ranged radius)");
    assert_eq!(sim.npcs.get(&wolf_id).unwrap().combat.health, initial_wolf_hp - 70);
}

#[test]
fn test_fun_03_mob_defeat_drops_pelts_fangs_and_marks() {
    let mut sim = WorldSimulation::new("BeastHunter".to_string()).unwrap();

    let wolf_id = 2001;
    sim.npcs.get_mut(&wolf_id).unwrap().position = sim.player.position;
    sim.npcs.get_mut(&wolf_id).unwrap().combat.health = 50; // Low HP

    let initial_marks = sim.player.marks;
    assert_eq!(sim.player.inventory.get(&4).copied().unwrap_or(0), 0); // 0 pelts
    assert_eq!(sim.player.inventory.get(&5).copied().unwrap_or(0), 0); // 0 fangs

    // Execute lethal strike
    let res = sim.step(vec![PlayerIntent::Attack(CombatActionType::LightAttack)]).unwrap();

    // Verify NpcSlain event and marks reward
    assert!(res.iter().any(|e| matches!(e, GameEvent::NpcSlain { target, reward_marks: 25, .. } if *target == wolf_id)));
    assert_eq!(sim.player.marks, initial_marks + 25);

    // Verify organic drops: 2 Pelts, 1 Fang
    assert_eq!(sim.player.inventory.get(&4).copied().unwrap_or(0), 2);
    assert_eq!(sim.player.inventory.get(&5).copied().unwrap_or(0), 1);

    // Verify thermodynamic balance for beast drops
    assert_eq!(sim.thermodynamic_ledger.get_balance(sim.player.entity_id, 4), 2);
    assert_eq!(sim.thermodynamic_ledger.get_balance(sim.player.entity_id, 5), 1);
}

#[test]
fn test_fun_04_stranded_remainder_mining_depletes_cleanly() {
    let mut sim = WorldSimulation::new("Prospector".to_string()).unwrap();

    // Insert a node with 7 ore remaining (stranded remainder that previously trapped the miner)
    let node_id = 7777;
    sim.resource_nodes.insert(node_id, ResourceNode {
        node_id,
        chunk: ChunkCoord::from_world_pos(sim.player.position),
        world_pos: sim.player.position,
        resource_id: 1, // Iron Ore
        remaining_amount: 7,
        max_amount: 100,
    });

    // Harvest the remaining 7 units
    let res = sim.step(vec![PlayerIntent::Harvest(node_id)]).unwrap();
    let harvest_event = res.iter().any(|e| matches!(e, GameEvent::ResourceHarvested { amount: 7, .. }));
    assert!(harvest_event, "Successfully harvested remaining 7 units");

    // Node is now completely empty (0 units)
    assert_eq!(sim.resource_nodes.get(&node_id).unwrap().remaining_amount, 0);
    assert_eq!(sim.player.inventory.get(&1).copied().unwrap_or(0), 7);
}

#[test]
fn test_fun_05_biome_distribution_contains_all_four_biomes() {
    let generator = aeviterra_cli::terrain::GenesisTerrainGenerator::new(*b"AEVITERRA_GENESIS_SEED_2026_100Y");

    let mut found_mountain = false;
    let mut found_deepwilds = false;
    let mut found_frontier = false;
    let mut found_heartlands = false;

    for z in (-1000..=1000).step_by(50) {
        for x in (-1000..=1000).step_by(50) {
            let qx = Q64::from_int(x).unwrap();
            let qz = Q64::from_int(z).unwrap();
            match generator.biome_at(qx, qz).unwrap() {
                BiomeType::MountainPeak => found_mountain = true,
                BiomeType::DeepWilds => found_deepwilds = true,
                BiomeType::FrontierWilds => found_frontier = true,
                BiomeType::Heartlands => found_heartlands = true,
                _ => {}
            }
        }
    }

    assert!(found_mountain, "MountainPeak biome exists across continent");
    assert!(found_deepwilds, "DeepWilds biome exists across continent");
    assert!(found_frontier, "FrontierWilds biome exists across continent");
    assert!(found_heartlands, "Heartlands biome exists across continent");
}
