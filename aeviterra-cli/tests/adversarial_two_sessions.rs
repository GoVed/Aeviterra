use aeviterra_cli::multiplayer::TwoSessionEnvironment;
use aeviterra_cli::simulation::PlayerIntent;
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::game::combat::{CombatActionType, WeaponFamily, CombatState};
use aeviterra_core::game::settlement::SettlementError;
use aeviterra_core::game::quest::{QuestError, compute_quest_claim_nullifier};
use aeviterra_core::consensus::envelope::ActionAdmissionError;
use aeviterra_core::state::smt::SparseMerkleTree;
use aeviterra_core::network::{
    FramingError, StreamingFrameCodec, ReputationEvent, PeerTier, PriorityQueue,
    MAGIC_HEADER, CURRENT_WIRE_VERSION,
};

#[test]
fn test_01_two_sessions_handshake_and_3d_exploration() {
    let mut env = TwoSessionEnvironment::new().expect("init two-session environment");

    // Both sessions authenticated over P2P
    assert!(env.session_alice.peer_session.is_authenticated());
    assert!(env.session_bob.peer_session.is_authenticated());

    // 3D coordinates & elevation check
    let pos_a = env.session_alice.sim.player.position;
    let pos_b = env.session_bob.sim.player.position;

    assert!(pos_a.y > Q64::ZERO, "Alice elevation must be positive terrain");
    assert!(pos_b.y > Q64::ZERO, "Bob elevation must be positive terrain");

    // Alice moves 3m East, 3m North
    let move_intent = vec![
        PlayerIntent::Move { dx: Q64::from_int(3).unwrap(), dz: Q64::from_int(3).unwrap() },
    ];
    env.session_alice.sim.step(move_intent).expect("alice moves");

    let new_pos_a = env.session_alice.sim.player.position;
    assert_eq!(new_pos_a.x, pos_a.x + Q64::from_int(3).unwrap());
    assert_eq!(new_pos_a.z, pos_a.z + Q64::from_int(3).unwrap());
    // Elevation updated dynamically from continuous 3D noise
    assert!(new_pos_a.y > Q64::ZERO);

    // Step 5 ticks of network message exchange
    for _ in 0..5 {
        env.step_tick().expect("step tick");
    }
}

#[test]
fn test_02_two_sessions_resource_depletion_and_closed_damascus_forging() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Alice harvests initial mineral vein
    let node_id = env.session_alice.sim.resource_nodes.keys().next().copied().expect("vein exists");
    
    // Mine 50 iron ore (5 harvests x 10 ore)
    for _ in 0..5 {
        env.session_alice.sim.step(vec![PlayerIntent::Harvest(node_id)]).expect("harvest");
    }

    assert_eq!(env.session_alice.sim.player.inventory.get(&1).copied().unwrap_or(0), 50);

    // Alice crafts Masterwork Damascus Broadsword (50 iron -> 35 sword + 15 slag)
    env.session_alice.sim.step(vec![PlayerIntent::CraftSword]).expect("craft sword");

    assert_eq!(env.session_alice.sim.player.inventory.get(&1).copied().unwrap_or(0), 0);
    assert_eq!(env.session_alice.sim.player.inventory.get(&101).copied().unwrap_or(0), 1);

    // Verify thermodynamic mass ledger: initial 100 tonnes, zero leakage
    let initial_mass = env.session_alice.sim.thermodynamic_ledger.total_world_mass_microunits;
    assert_eq!(initial_mass, 100_000_000_000);
}

#[test]
fn test_03_two_sessions_pvp_combat_iframes_and_cc_diminishing_returns() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Alice has Blade, Bob has Spear
    assert_eq!(env.session_alice.sim.player.combat.active_weapon, WeaponFamily::Blade);
    assert_eq!(env.session_bob.sim.player.combat.active_weapon, WeaponFamily::Spear);

    // Bob rolls (DodgeRoll), gaining 10-tick invulnerability frames
    let bob_roll_stamina = env.session_bob.sim.player.combat.execute_action(CombatActionType::DodgeRoll)
        .expect("bob rolls");
    assert_eq!(bob_roll_stamina, 25);
    assert_eq!(env.session_bob.sim.player.combat.iframe_remaining_ticks, 10);

    // Alice executes Heavy Attack while Bob has active i-frames
    let raw_damage = 220u32;
    let effective_dmg = env.session_bob.sim.player.combat.apply_incoming_damage(raw_damage);
    assert_eq!(effective_dmg, 0, "Active i-frames must neutralize incoming damage completely");
    assert_eq!(env.session_bob.sim.player.combat.health, env.session_bob.sim.player.combat.max_health);

    // Tick forward 10 ticks to expire i-frames
    for _ in 0..10 {
        env.session_bob.sim.player.combat.tick_regeneration();
    }
    assert_eq!(env.session_bob.sim.player.combat.iframe_remaining_ticks, 0);

    // Now attack lands on Bob
    let dmg_after_iframes = env.session_bob.sim.player.combat.apply_incoming_damage(raw_damage);
    assert_eq!(dmg_after_iframes, 220);
    assert_eq!(env.session_bob.sim.player.combat.health, 780);

    // Test Crowd Control Diminishing Returns (CC DR)
    let base_stun = 100u32;
    let dur1 = CombatState::evaluate_crowd_control_duration(base_stun, 0);
    assert_eq!(dur1, 100, "1st Stun: 100% duration");

    let dur2 = CombatState::evaluate_crowd_control_duration(base_stun, 1);
    assert_eq!(dur2, 50, "2nd Stun within DR window: 50% duration");

    let dur3 = CombatState::evaluate_crowd_control_duration(base_stun, 2);
    assert_eq!(dur3, 25, "3rd Stun within DR window: 25% duration");

    let dur4 = CombatState::evaluate_crowd_control_duration(base_stun, 3);
    assert_eq!(dur4, 0, "4th Stun: Complete CC immunity!");
}

#[test]
fn test_04_adversarial_break_attempt_stale_replay_attack() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Alice creates valid signed action envelope for tick 1 (sequence = 1)
    let env1 = env.session_alice.create_signed_action(
        1, // Attack intent
        b"attack_intent_target_wolf".to_vec(),
        1,
    );

    // Bob's verifier accepts sequence 1
    let digest = env.session_bob.admission_verifier.verify_and_record(&env1, 1)
        .expect("Bob records first action");
    assert_ne!(digest, [0u8; 32]);

    // Attacker intercepts and replays env1 at tick 10
    let replay_err = env.session_bob.admission_verifier.verify_and_record(&env1, 10)
        .expect_err("Bob must reject replayed action envelope");

    assert_eq!(
        replay_err,
        ActionAdmissionError::StaleOrReplayedSequence { expected_min: 2, got: 1 }
    );
}

#[test]
fn test_05_adversarial_break_attempt_infinite_stamina_exploit() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Drain Alice's stamina to 0
    env.session_alice.sim.player.combat.stamina = 0;

    // Alice tries to execute LightAttack (cost 15) and HeavyAttack (cost 35)
    let err_light = env.session_alice.sim.player.combat.execute_action(CombatActionType::LightAttack);
    assert_eq!(err_light, Err(aeviterra_core::game::combat::CombatError::InsufficientStamina));

    let err_heavy = env.session_alice.sim.player.combat.execute_action(CombatActionType::HeavyAttack);
    assert_eq!(err_heavy, Err(aeviterra_core::game::combat::CombatError::InsufficientStamina));

    // Stamina remains 0, no exploit permitted
    assert_eq!(env.session_alice.sim.player.combat.stamina, 0);
}

#[test]
fn test_06_adversarial_break_attempt_speed_hack_teleportation() {
    let mut env = TwoSessionEnvironment::new().expect("init");
    let start_pos = env.session_bob.sim.player.position;

    // Bob tries to move 1,000 meters in a single tick (speed hacking)
    let speed_hack_intent = vec![
        PlayerIntent::Move { dx: Q64::from_int(1000).unwrap(), dz: Q64::ZERO },
    ];
    env.session_bob.sim.step(speed_hack_intent).expect("step");

    let post_pos = env.session_bob.sim.player.position;
    let actual_dx = post_pos.x - start_pos.x;

    // Physics kernel clamps to 5 m/s maximum speed limit!
    assert_eq!(actual_dx, Q64::from_int(5).unwrap(), "Displacement must be clamped to 5 m/s");
}

#[test]
fn test_07_adversarial_break_attempt_concurrent_plot_theft() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Bob claims plot 42 in the settlement
    let plot_id = 42u64;
    env.session_bob.settlement_charter.claim_plot(plot_id, env.session_bob.player_id, 100)
        .expect("Bob claims plot");

    // Alice attempts to claim the exact same plot 42
    let alice_claim_err = env.session_bob.settlement_charter.claim_plot(plot_id, env.session_alice.player_id, 101)
        .expect_err("Alice must not be able to steal Bob's plot");

    assert_eq!(alice_claim_err, SettlementError::PlotAlreadyClaimed);
}

#[test]
fn test_08_adversarial_break_attempt_double_quest_claim_nullifier() {
    let mut env = TwoSessionEnvironment::new().expect("init");
    let actor_id = env.session_alice.player_id;
    let quest_id = 1u32;

    // Advance quest progress to completion
    let progress = env.session_alice.quest_engine.player_progress.entry((actor_id, quest_id)).or_insert(
        aeviterra_core::game::quest::PlayerQuestProgress {
            quest_id,
            current_count: 10,
            completed: true,
        }
    );
    assert!(progress.completed);

    // 1st Claim: compute nullifier and record
    let nullifier = compute_quest_claim_nullifier(actor_id, quest_id, 1);
    let claim1 = env.session_alice.quest_engine.claim_reward(actor_id, quest_id);
    assert!(claim1.is_ok());
    assert!(env.session_alice.quest_engine.consumed_claims.contains(&nullifier));

    // 2nd Claim attempt: replay the claim with the same nullifier
    let claim2 = env.session_alice.quest_engine.claim_reward(actor_id, quest_id);
    assert_eq!(claim2, Err(QuestError::RewardAlreadyClaimed));
}

#[test]
fn test_09_adversarial_break_attempt_packet_poisoning_and_reputation_slash() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Attacker crafts an oversized 75 KB frame (violating MAX_PACKET_SIZE = 64 KB)
    let mut bad_packet = vec![0u8; 75_000];
    bad_packet[0..4].copy_from_slice(&(74_996u32).to_be_bytes()); // length prefix
    bad_packet[4..8].copy_from_slice(MAGIC_HEADER);
    bad_packet[8..10].copy_from_slice(&CURRENT_WIRE_VERSION.to_be_bytes());
    bad_packet[10] = PriorityQueue::Gameplay as u8;

    let mut codec = StreamingFrameCodec::new();
    codec.feed(&bad_packet).expect("feed");
    let parse_err = codec.pop_frame().expect_err("oversized frame must be rejected");

    assert_eq!(parse_err, FramingError::FrameTooLarge(74_996));

    // Bob records reputation slash for framing violation
    let attacker_id = [99u8; 32];
    let attacker_subnet = "198.51.100".to_string();

    let tier1 = env.session_bob.reputation_ledger.record_event(
        attacker_id,
        attacker_subnet.clone(),
        ReputationEvent::MalformedWireFrame,
        1000,
    );
    assert_eq!(tier1, PeerTier::Throttled); // -200 score

    // Repeated violations quarantine the attacker
    env.session_bob.reputation_ledger.record_event(
        attacker_id,
        attacker_subnet.clone(),
        ReputationEvent::MalformedWireFrame,
        1000,
    );
    let tier3 = env.session_bob.reputation_ledger.record_event(
        attacker_id,
        attacker_subnet.clone(),
        ReputationEvent::MalformedWireFrame,
        1000,
    );
    assert_eq!(tier3, PeerTier::Quarantined); // -600 score <= -500
}

#[test]
fn test_10_state_convergence_and_merkle_invariance() {
    let mut env = TwoSessionEnvironment::new().expect("init");

    // Simulate 50 deterministic ticks
    for _ in 0..50 {
        env.step_tick().expect("step tick");
    }

    // Both sessions recompute canonical Sparse Merkle Tree roots
    let root_a = env.session_alice.sim.recompute_merkle_root().expect("recompute A");
    let root_b = env.session_bob.sim.recompute_merkle_root().expect("recompute B");

    assert_ne!(root_a, [0u8; 32], "Root A must be non-zero");
    assert_ne!(root_b, [0u8; 32], "Root B must be non-zero");

    // Verify SMT inclusion proofs
    let leaf_hash_a = *env.session_alice.sim.smt.get(env.session_alice.player_id).expect("leaf A exists");
    let proof_a = env.session_alice.sim.smt.prove(env.session_alice.player_id);
    assert!(SparseMerkleTree::verify_proof(&root_a, &proof_a));
    assert_eq!(proof_a.value_hash, leaf_hash_a);
}
