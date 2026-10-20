use aeviterra_cli::simulation::{WorldSimulation, PlayerIntent, GameEvent};
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::game::combat::{CombatActionType, WeaponFamily};
use aeviterra_core::state::smt::SparseMerkleTree;
use aeviterra_core::shard::halo::ChunkCoord;
use std::io::{self, Write};
use std::time::Instant;

fn print_banner() {
    println!("\x1b[1;36m=================================================================================\x1b[0m");
    println!("\x1b[1;32m      AEVITERRA PROTOCOL V4.1 — AUTONOMOUS VIRTUAL WORLD SIMULATION ENGINE       \x1b[0m");
    println!("\x1b[1;30m      Century-Scale Autonomous Virtual World • Deterministic Simulation Loop     \x1b[0m");
    println!("\x1b[1;36m=================================================================================\x1b[0m");
}

fn print_status(sim: &WorldSimulation) {
    let p = &sim.player;
    let biome = sim.terrain.biome_at(p.position.x, p.position.z).unwrap();
    let chunk = ChunkCoord::from_world_pos(p.position);

    println!("\n\x1b[1;33m--- AVATAR STATUS [Tick #{}] ---\x1b[0m", sim.current_tick);
    println!("  Name: \x1b[1m{}\x1b[0m (ID: {})", p.name, p.entity_id);
    println!("  Coordinates: ({:.2}, {:.2}, {:.2}) | Chunk: [{}, {}, {}]",
        p.position.x, p.position.y, p.position.z, chunk.x, chunk.y, chunk.z);
    println!("  Biome: \x1b[1;35m{:?}\x1b[0m", biome);

    // Health Bar
    let hp_pct = (p.combat.health * 10) / p.combat.max_health;
    let hp_bar: String = (0..10).map(|i| if i < hp_pct { '█' } else { '░' }).collect();
    println!("  Health:  [\x1b[1;31m{}\x1b[0m] {}/{}", hp_bar, p.combat.health, p.combat.max_health);

    // Stamina Bar
    let sta_pct = (p.combat.stamina * 10) / p.combat.max_stamina;
    let sta_bar: String = (0..10).map(|i| if i < sta_pct { '█' } else { '░' }).collect();
    println!("  Stamina: [\x1b[1;32m{}\x1b[0m] {}/{}", sta_bar, p.combat.stamina, p.combat.max_stamina);

    println!("  Weapon:  \x1b[1;34m{:?}\x1b[0m (i-frames: {} ticks, blocking: {})",
        p.combat.active_weapon, p.combat.iframe_remaining_ticks, p.combat.is_blocking);

    println!("  Currency: \x1b[1;33m{} Marks\x1b[0m | \x1b[1;36m{} Sovereigns\x1b[0m", p.marks, p.sovereigns);
    print!("  Inventory: ");
    if p.inventory.values().all(|&v| v == 0) {
        println!("(Empty)");
    } else {
        for (&res_id, &count) in &p.inventory {
            if count > 0 {
                let name = match res_id {
                    1 => "Iron Ore",
                    2 => "Oak Timber",
                    3 => "Silver",
                    101 => "Damascus Sword",
                    _ => "Item",
                };
                print!("[{}: {}] ", name, count);
            }
        }
        println!();
    }

    let root_hex = hex_str(&sim.last_state_root);
    println!("  \x1b[1;30mCanonical SMT Root: 0x{}\x1b[0m", &root_hex[0..16]);
}

fn hex_str(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn render_map(sim: &WorldSimulation) {
    let center_x = sim.player.position.x.to_int_trunc();
    let center_z = sim.player.position.z.to_int_trunc();
    let radius = 12; // 25x25 grid cells (~50m field of view)

    println!("\n\x1b[1;33m--- LOCAL TERRAIN MAP (Grid: 2m/cell, Center: [{}, {}]) ---\x1b[0m", center_x, center_z);
    println!("  \x1b[1;32m@\x1b[0m = Player  |  \x1b[1;31mW\x1b[0m = Wolf/Hostile  |  \x1b[1;33m*\x1b[0m = Resource Vein  |  \x1b[1;30m.\x1b[0m = Grassland  |  \x1b[1;37m^\x1b[0m = Hill");

    for dz in -radius..=radius {
        print!("  ");
        for dx in -radius..=radius {
            let wx = center_x + dx * 2;
            let wz = center_z + dz * 2;

            if dx == 0 && dz == 0 {
                print!("\x1b[1;32m@ \x1b[0m"); // Player
                continue;
            }

            // Check if NPC is in this cell
            let mut has_npc = false;
            for npc in sim.npcs.values() {
                if npc.combat.health > 0 {
                    let nx = npc.position.x.to_int_trunc();
                    let nz = npc.position.z.to_int_trunc();
                    if (nx - wx).abs() <= 1 && (nz - wz).abs() <= 1 {
                        print!("\x1b[1;31mW \x1b[0m");
                        has_npc = true;
                        break;
                    }
                }
            }
            if has_npc { continue; }

            // Check for resource vein
            let mut has_node = false;
            for node in sim.resource_nodes.values() {
                if node.remaining_amount > 0 {
                    let rx = node.world_pos.x.to_int_trunc();
                    let rz = node.world_pos.z.to_int_trunc();
                    if (rx - wx).abs() <= 2 && (rz - wz).abs() <= 2 {
                        print!("\x1b[1;33m* \x1b[0m");
                        has_node = true;
                        break;
                    }
                }
            }
            if has_node { continue; }

            // Terrain elevation display
            let qx = Q64::from_int(wx).unwrap();
            let qz = Q64::from_int(wz).unwrap();
            let elev = sim.terrain.elevation_at(qx, qz).unwrap().to_int_trunc();

            if elev > 100 {
                print!("\x1b[1;37m^ \x1b[0m"); // High elevation
            } else if elev > 60 {
                print!("\x1b[0;32m, \x1b[0m"); // Woodlands
            } else {
                print!("\x1b[1;30m. \x1b[0m"); // Plains
            }
        }
        println!();
    }
}

fn execute_intents_and_report(sim: &mut WorldSimulation, intents: Vec<PlayerIntent>) {
    let events = sim.step(intents).expect("Simulation step failed");
    for ev in events {
        match ev {
            GameEvent::EntityMoved { id: _id, new_pos } => {
                println!("  \x1b[1;34m-> Moved to ({:.2}, {:.2}, {:.2})\x1b[0m", new_pos.x, new_pos.y, new_pos.z);
            }
            GameEvent::CombatActionExecuted { action, stamina_cost, .. } => {
                println!("  \x1b[1;36m-> Executed {:?} (Cost: {} stamina)\x1b[0m", action, stamina_cost);
            }
            GameEvent::CombatDamageDealt { attacker, target, damage, target_health } => {
                println!("  \x1b[1;31m-> [COMBAT] Entity #{} hit #{} for {} damage! Target HP: {}\x1b[0m",
                    attacker, target, damage, target_health);
            }
            GameEvent::ResourceHarvested { resource_name, amount, .. } => {
                println!("  \x1b[1;33m-> [GATHER] Harvested +{} {} (Logged in Thermodynamic Ledger)\x1b[0m", amount, resource_name);
            }
            GameEvent::ItemCrafted { item_name, .. } => {
                println!("  \x1b[1;32m-> [CRAFT] Masterwork crafted: {} (Closed recipe, mass conserved!)\x1b[0m", item_name);
            }
            GameEvent::StateRootUpdated { tick, root } => {
                let r_hex = hex_str(&root);
                println!("  \x1b[1;30m[Tick #{}] SMT Root: 0x{}\x1b[0m", tick, &r_hex[0..16]);
            }
        }
    }
}

fn run_automated_battle(sim: &mut WorldSimulation) {
    println!("\n\x1b[1;31m=== ENGAGING IN CONTINUOUS DETERMINISTIC BATTLE (25 TICKS) ===\x1b[0m");
    for i in 1..=25 {
        let intent = if i % 4 == 0 {
            PlayerIntent::Attack(CombatActionType::HeavyAttack)
        } else if i == 5 {
            PlayerIntent::Attack(CombatActionType::DodgeRoll) // Test i-frames
        } else if i == 7 {
            PlayerIntent::Attack(CombatActionType::Block)
        } else {
            PlayerIntent::Attack(CombatActionType::LightAttack)
        };

        println!("\n\x1b[1m[Tick +{}]\x1b[0m", i);
        execute_intents_and_report(sim, vec![intent]);

        let wolf_hp = sim.npcs.get(&2001).map(|n| n.combat.health).unwrap_or(0);
        if wolf_hp == 0 {
            println!("\n\x1b[1;32m*** VICTORY! Direwolf Stalker has been slain! ***\x1b[0m");
            break;
        }
    }
}

fn run_benchmark(sim: &mut WorldSimulation) {
    println!("\n\x1b[1;35m=== RUNNING DETERMINISTIC BENCHMARK: 2,000 CONSECUTIVE TICKS ===\x1b[0m");
    let start = Instant::now();
    let ticks = 2000;

    let move_intent = PlayerIntent::Move {
        dx: Q64::from_int(1).unwrap(),
        dz: Q64::from_int(0).unwrap(),
    };

    for _ in 0..ticks {
        let _ = sim.step(vec![move_intent.clone()]);
    }

    let elapsed = start.elapsed();
    let tps = (ticks as f64) / elapsed.as_secs_f64();

    println!("  Simulated {} ticks in {:.3} seconds", ticks, elapsed.as_secs_f64());
    println!("  Throughput: \x1b[1;32m{:.0} Ticks Per Second (TPS)\x1b[0m", tps);
    println!("  (Production target is 20 TPS; current engine runs at \x1b[1;32m{:.1}x realtime speed\x1b[0m)", tps / 20.0);
    println!("  Final Canonical SMT Root: 0x{}", hex_str(&sim.last_state_root));
}

fn audit_merkle_proof(sim: &WorldSimulation) {
    println!("\n\x1b[1;36m=== CRYPTOGRAPHIC SPARSE MERKLE TREE AUDIT ===\x1b[0m");
    let player_id = sim.player.entity_id;
    let proof = sim.smt.prove(player_id);

    println!("  Proving membership for Entity #{}:", player_id);
    println!("  Leaf Value Hash: 0x{}", hex_str(&proof.value_hash));
    println!("  Sibling Proof Elements: {} levels (64-bit SMT)", proof.siblings.len());

    let is_valid = SparseMerkleTree::verify_proof(&sim.last_state_root, &proof);
    if is_valid {
        println!("  Verification Result: \x1b[1;32mVALID! Proof strictly validates against Canonical Root 0x{}\x1b[0m",
            &hex_str(&sim.last_state_root)[0..16]);
    } else {
        println!("  Verification Result: \x1b[1;31mFAILED! Tampering detected.\x1b[0m");
    }
}

fn print_help() {
    println!("\n\x1b[1mCOMMANDS AVAILABLE:\x1b[0m");
    println!("  \x1b[1;32mw / a / s / d\x1b[0m             Move North / West / South / East (3 meters)");
    println!("  \x1b[1;36mlight / l\x1b[0m                 Execute Light Attack (15 stamina)");
    println!("  \x1b[1;36mheavy / h\x1b[0m                 Execute Heavy Strike (35 stamina, 220 dmg)");
    println!("  \x1b[1;36mroll / r\x1b[0m                  Dodge Roll (25 stamina, 200ms invincible i-frames)");
    println!("  \x1b[1;36mblock / b\x1b[0m                 Raise Shield / Defensive Poise (mitigates 70% damage)");
    println!("  \x1b[1;34mweapon <blade|bow|shield>\x1b[0m Switch active weapon family");
    println!("  \x1b[1;33mgather / g\x1b[0m                Harvest nearby resource deposit into integer ledger");
    println!("  \x1b[1;32mcraft / c\x1b[0m                 Craft Damascus Broadsword (closed mass recipe: 50 ore -> 35 sword + 15 slag)");
    println!("  \x1b[1;35mmap / m\x1b[0m                   Render local continental terrain map");
    println!("  \x1b[1;33mstatus / stat\x1b[0m             Display avatar attributes, inventory & SMT root");
    println!("  \x1b[1;31mbattle / auto\x1b[0m             Run automated continuous combat encounter");
    println!("  \x1b[1;36maudit\x1b[0m                     Verify cryptographic SMT inclusion proof");
    println!("  \x1b[1;35mbench\x1b[0m                     Run 2,000-tick performance TPS benchmark");
    println!("  \x1b[1;30mquit / q\x1b[0m                  Exit simulation");
}

fn main() {
    print_banner();

    let mut sim = WorldSimulation::new("Aevitas_Pioneer".to_string())
        .expect("Failed to initialize Aeviterra world simulation");

    println!("\n\x1b[1;32mWorld initialized successfully from 32-byte Genesis Seed!\x1b[0m");
    print_status(&sim);
    println!("\nType \x1b[1;36mhelp\x1b[0m or \x1b[1;36m?\x1b[0m to view available commands. Type \x1b[1;32mm\x1b[0m for map, \x1b[1;31mbattle\x1b[0m to fight.");

    let stdin = io::stdin();
    loop {
        print!("\n\x1b[1;32maeviterra [tick:{}]>\x1b[0m ", sim.current_tick);
        io::stdout().flush().unwrap();

        let mut input = String::new();
        if stdin.read_line(&mut input).is_err() {
            break;
        }

        let trimmed = input.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0].to_lowercase().as_str() {
            "q" | "quit" | "exit" => {
                println!("Exiting Aeviterra simulation. State persisted in memory.");
                break;
            }
            "h" | "help" | "?" => print_help(),
            "s" | "stat" | "status" => print_status(&sim),
            "m" | "map" => render_map(&sim),
            "w" | "north" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Move { dx: Q64::ZERO, dz: Q64::from_int(-3).unwrap() }]),
            "s_down" | "south" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Move { dx: Q64::ZERO, dz: Q64::from_int(3).unwrap() }]),
            "a" | "west" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Move { dx: Q64::from_int(-3).unwrap(), dz: Q64::ZERO }]),
            "d" | "east" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Move { dx: Q64::from_int(3).unwrap(), dz: Q64::ZERO }]),
            "l" | "light" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Attack(CombatActionType::LightAttack)]),
            "heavy" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Attack(CombatActionType::HeavyAttack)]),
            "r" | "roll" | "dodge" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Attack(CombatActionType::DodgeRoll)]),
            "b" | "block" => execute_intents_and_report(&mut sim, vec![PlayerIntent::Attack(CombatActionType::Block)]),
            "weapon" => {
                if parts.len() > 1 {
                    let family = match parts[1].to_lowercase().as_str() {
                        "bow" => WeaponFamily::Bow,
                        "shield" => WeaponFamily::Shield,
                        _ => WeaponFamily::Blade,
                    };
                    execute_intents_and_report(&mut sim, vec![PlayerIntent::SwitchWeapon(family)]);
                    println!("  \x1b[1;34mSwitched active weapon to {:?}\x1b[0m", family);
                } else {
                    println!("Usage: weapon <blade|bow|shield>");
                }
            }
            "g" | "gather" => {
                // Find closest resource node
                let node_id = sim.resource_nodes.keys().next().copied();
                if let Some(id) = node_id {
                    execute_intents_and_report(&mut sim, vec![PlayerIntent::Harvest(id)]);
                } else {
                    println!("  No resource veins discovered nearby. Explore to find veins!");
                }
            }
            "c" | "craft" => {
                execute_intents_and_report(&mut sim, vec![PlayerIntent::CraftSword]);
            }
            "battle" | "auto" => run_automated_battle(&mut sim),
            "audit" => audit_merkle_proof(&sim),
            "bench" => run_benchmark(&mut sim),
            _ => {
                println!("Unknown command: '{}'. Type 'help' for commands.", trimmed);
            }
        }
    }
}
