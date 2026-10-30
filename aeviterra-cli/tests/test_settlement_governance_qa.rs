#![allow(warnings, clippy::all)]
use aeviterra_core::game::settlement::*;
use aeviterra_core::assets::grammar::{StructuralPrimitive, ArchitecturalStyle, MaterialType};
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::consensus::envelope::ActionAdmissionError;
use aeviterra_cli::multiplayer::TwoSessionEnvironment;

/// ============================================================================
/// AEVITERRA 10-PLAYER GAME TEST CAMPAIGN — AGENT 6: MUNICIPAL GOVERNANCE AUDITOR
/// Dedicated Integration Test Suite: Settlement, Homestead Charter, Mayoral Tenure,
/// Concurrent Plot Claims, and Municipal Treasury Tax Bond Invariants.
///
/// Specification Target:
/// - V4 §10 & V4.1 G5: Homesteads & Settlement Governance
/// - MIN_TENURE_TICKS = 518_400 (~30 days at 20 ticks/sec)
/// - MIN_TAX_BOND_MARKS = 100
/// ============================================================================

// ============================================================================
// PART 1: MAYORAL TENURE AND RESIDENCY CHECKS (MIN_TENURE_TICKS = 518_400)
// ============================================================================

/// Test 01: Verify exact mathematical boundaries of mayoral voting tenure.
/// One tick below 518,400 must fail; exactly 518,400 must pass.
#[test]
fn test_01_mayoral_tenure_exact_tick_boundaries() {
    let mut charter = SettlementCharter::new(1, "Sanctuary Freehold".to_string(), 1000);
    let player_id = 1001u64;
    let t0 = 10_000u64;

    // Player claims initial homestead plot at tick T0
    assert!(charter.claim_plot(1, player_id, t0).is_ok());

    // Satisfy municipal tax bond
    assert!(charter.pay_tax(player_id, MIN_TAX_BOND_MARKS).is_ok());

    // Boundary A: current_tick < registration_tick (clock anomaly / time-drift)
    assert_eq!(
        charter.can_vote_for_mayor(player_id, t0.saturating_sub(1)),
        Err(SettlementError::InsufficientTenure),
        "Clock before registration must be rejected as insufficient tenure"
    );

    // Boundary B: Exactly 1 tick BEFORE tenure eligibility (518,399 ticks accrued)
    let tick_shy = t0 + MIN_TENURE_TICKS - 1;
    assert_eq!(
        charter.can_vote_for_mayor(player_id, tick_shy),
        Err(SettlementError::InsufficientTenure),
        "518,399 ticks must fail with InsufficientTenure"
    );

    // Boundary C: EXACTLY MIN_TENURE_TICKS accrued (518,400 ticks)
    let tick_exact = t0 + MIN_TENURE_TICKS;
    assert_eq!(
        charter.can_vote_for_mayor(player_id, tick_exact),
        Ok(true),
        "Exactly 518,400 ticks must successfully qualify for mayoral voting"
    );

    // Boundary D: 1 tick BEYOND tenure eligibility (518,401 ticks accrued)
    let tick_beyond = t0 + MIN_TENURE_TICKS + 1;
    assert_eq!(
        charter.can_vote_for_mayor(player_id, tick_beyond),
        Ok(true),
        "Tenure beyond 518,400 ticks must remain valid"
    );
}

/// Test 02: Verify that residency registration_tick is immutable and persistent
/// even when a player claims additional plots later in time.
#[test]
fn test_02_residency_registration_tick_immutability_on_subsequent_claims() {
    let mut charter = SettlementCharter::new(1, "Aethelgard Plains".to_string(), 1000);
    let settler_id = 1002u64;
    let genesis_tick = 500u64;

    // 1. Initial plot claim at tick 500 establishes official residency
    assert!(charter.claim_plot(10, settler_id, genesis_tick).is_ok());
    assert_eq!(
        charter.residents.get(&settler_id).unwrap().registration_tick,
        genesis_tick,
        "Registration tick should match the first plot claim"
    );

    // 2. Later claims for secondary homestead plots (e.g., pasture, workshop)
    assert!(charter.claim_plot(20, settler_id, 200_000).is_ok());
    assert!(charter.claim_plot(30, settler_id, 500_000).is_ok());

    // Registration tick MUST NOT be overwritten by subsequent claims!
    assert_eq!(
        charter.residents.get(&settler_id).unwrap().registration_tick,
        genesis_tick,
        "Subsequent plot claims must not reset the resident's historical tenure tick"
    );

    // Pay required tax bond
    assert!(charter.pay_tax(settler_id, 100).is_ok());

    // Election at tick 520,000:
    // Tenure from genesis: 520,000 - 500 = 519,500 >= 518,400 (Eligible!)
    // If registration_tick had reset to 500,000, tenure would be only 20,000 (Ineligible!)
    let election_tick = 520_000u64;
    assert_eq!(
        charter.can_vote_for_mayor(settler_id, election_tick),
        Ok(true),
        "Settler's tenure must be calculated from initial settlement entry"
    );
}

/// Test 03: Verify rejection of non-residents and foreign actors.
#[test]
fn test_03_non_resident_and_foreign_actor_rejection() {
    let mut charter = SettlementCharter::new(1, "Valoria Capital".to_string(), 1000);
    let resident_id = 1001u64;
    let foreigner_id = 9999u64;

    assert!(charter.claim_plot(1, resident_id, 100).is_ok());

    // Unregistered foreigner cannot vote
    assert_eq!(
        charter.can_vote_for_mayor(foreigner_id, 1_000_000),
        Err(SettlementError::SybilVotingDisqualified),
        "Unregistered actor must be disqualified"
    );

    // Unregistered foreigner cannot pay taxes into a resident account
    assert_eq!(
        charter.pay_tax(foreigner_id, 500),
        Err(SettlementError::SybilVotingDisqualified),
        "Taxes cannot be credited to an unregistered non-resident"
    );

    // Non-resident cannot be elected mayor
    assert_eq!(
        charter.elect_mayor(foreigner_id, 1_000_000),
        Err(SettlementError::SybilVotingDisqualified),
        "Non-resident cannot assume mayoral office"
    );
}

/// Test 04: Sybil Swarm Attack Mitigation (V4.1 G5).
/// Simulates a rival guild attempting to deploy 7 bot accounts with flash capital
/// right before an election to seize control of the settlement.
#[test]
fn test_04_sybil_swarm_election_takeover_attack_blocked() {
    let mut charter = SettlementCharter::new(1, "Borderlands Citadel".to_string(), 1001);

    // Honest Pioneer 1 (Current Mayor): registered at tick 0
    assert!(charter.claim_plot(1, 1001, 0).is_ok());
    assert!(charter.pay_tax(1001, 200).is_ok());

    // Honest Pioneer 2: registered at tick 1,000
    assert!(charter.claim_plot(2, 1002, 1_000).is_ok());
    assert!(charter.pay_tax(1002, 150).is_ok());

    // Honest Pioneer 3: registered at tick 5,000
    assert!(charter.claim_plot(3, 1003, 5_000).is_ok());
    assert!(charter.pay_tax(1003, 100).is_ok());

    // Adversary guild spins up 7 Sybil bot accounts at tick 515,000
    // Election is scheduled for tick 525,000 (only 10,000 ticks later)
    let sybil_spawn_tick = 515_000u64;
    let election_tick = 525_000u64;
    let bot_ids: Vec<u64> = (2001..=2007).collect();

    for (i, &bot_id) in bot_ids.iter().enumerate() {
        let plot_id = 100 + i as u64;
        assert!(charter.claim_plot(plot_id, bot_id, sybil_spawn_tick).is_ok());
        // Adversary floods the bots with 5,000 Marks each (50x the required tax bond!)
        assert!(charter.pay_tax(bot_id, 5_000).is_ok());
    }

    // Verify all 3 honest pioneers can vote
    assert_eq!(charter.can_vote_for_mayor(1001, election_tick), Ok(true));
    assert_eq!(charter.can_vote_for_mayor(1002, election_tick), Ok(true));
    assert_eq!(charter.can_vote_for_mayor(1003, election_tick), Ok(true));

    // Verify EVERY SINGLE SYBIL BOT is blocked by the 518,400 tenure requirement
    for &bot_id in &bot_ids {
        let vote_res = charter.can_vote_for_mayor(bot_id, election_tick);
        assert_eq!(
            vote_res,
            Err(SettlementError::InsufficientTenure),
            "Bot {} must be rejected due to insufficient tenure despite 5,000 Marks tax bond",
            bot_id
        );

        // Bots cannot be elected mayor either
        assert_eq!(
            charter.elect_mayor(bot_id, election_tick),
            Err(SettlementError::InsufficientTenure)
        );
    }

    // Attempting to install bot candidate fails; mayor remains honest Pioneer 1
    assert_eq!(charter.mayor_id, 1001);
}

// ============================================================================
// PART 2: HOMESTEAD PLOT CLAIMING AND CONCURRENT PLOT CLAIMING EDGE CASES
// ============================================================================

/// Test 05: Verify first-come first-served plot claiming and rejection of plot theft.
#[test]
fn test_05_plot_claiming_first_come_first_served_and_reclaim() {
    let mut charter = SettlementCharter::new(1, "Aevitas Haven".to_string(), 1000);
    let owner_a = 1001u64;
    let intruder_b = 1002u64;
    let plot_id = 42u64;

    // Owner A claims plot 42
    assert!(charter.claim_plot(plot_id, owner_a, 100).is_ok());
    assert_eq!(charter.plots.len(), 1);
    let plot = charter.plots.get(&plot_id).unwrap();
    assert_eq!(plot.owner_id, owner_a);
    assert_eq!(plot.settlement_id, 1);
    assert!(plot.built_structures.is_empty());

    // Intruder B attempts to claim the exact same plot 42 -> fails
    let steal_attempt = charter.claim_plot(plot_id, intruder_b, 101);
    assert_eq!(steal_attempt, Err(SettlementError::PlotAlreadyClaimed));

    // Owner A attempts to re-claim the same plot 42 -> fails (idempotent duplicate rejection)
    let duplicate_claim = charter.claim_plot(plot_id, owner_a, 102);
    assert_eq!(duplicate_claim, Err(SettlementError::PlotAlreadyClaimed));

    // Verify state integrity
    assert_eq!(charter.plots.len(), 1);
    assert_eq!(charter.plots.get(&plot_id).unwrap().owner_id, owner_a);
}

/// Test 06: Concurrent plot claim race among 10 players on a single prime plot.
/// Verifies deterministic consensus behavior: exactly 1 wins, 9 fail with PlotAlreadyClaimed.
#[test]
fn test_06_10_player_concurrent_claim_race_on_single_plot() {
    let mut charter = SettlementCharter::new(1, "Solaria Outpost".to_string(), 1000);
    let prime_plot_id = 777u64;
    let player_ids: Vec<u64> = (1001..=1010).collect();
    let tick = 1000u64;

    let mut successful_claims = 0;
    let mut failed_claims = 0;
    let mut winning_owner = 0;

    // 10 players issue claim intent in the same consensus block
    for &pid in &player_ids {
        match charter.claim_plot(prime_plot_id, pid, tick) {
            Ok(()) => {
                successful_claims += 1;
                winning_owner = pid;
            }
            Err(SettlementError::PlotAlreadyClaimed) => {
                failed_claims += 1;
            }
            Err(e) => panic!("Unexpected error during claim race: {:?}", e),
        }
    }

    assert_eq!(successful_claims, 1, "Exactly one player must win the plot claim race");
    assert_eq!(failed_claims, 9, "All 9 rival claims must be rejected with PlotAlreadyClaimed");
    assert_eq!(winning_owner, 1001, "First serialized player in block order wins");

    // Re-verify underlying map invariants
    assert_eq!(charter.plots.len(), 1);
    assert_eq!(charter.plots.get(&prime_plot_id).unwrap().owner_id, 1001);
}

/// Test 07: 10 players concurrently claiming 10 distinct plots.
/// Verifies non-interfering parallel plot registration and resident creation.
#[test]
fn test_07_concurrent_disjoint_plot_claiming_10_players() {
    let mut charter = SettlementCharter::new(10, "Equinox Valley".to_string(), 1000);
    let player_ids: Vec<u64> = (1001..=1010).collect();

    for (i, &pid) in player_ids.iter().enumerate() {
        let plot_id = (i + 1) as u64;
        let claim_tick = 100 + i as u64;
        assert!(charter.claim_plot(plot_id, pid, claim_tick).is_ok());
    }

    assert_eq!(charter.plots.len(), 10);
    assert_eq!(charter.residents.len(), 10);

    for (i, &pid) in player_ids.iter().enumerate() {
        let plot_id = (i + 1) as u64;
        let plot = charter.plots.get(&plot_id).unwrap();
        assert_eq!(plot.owner_id, pid);
        assert_eq!(plot.settlement_id, 10);

        let resident = charter.residents.get(&pid).unwrap();
        assert_eq!(resident.actor_id, pid);
        assert_eq!(resident.settlement_id, 10);
        assert_eq!(resident.registration_tick, 100 + i as u64);
        assert_eq!(resident.cumulative_taxes_paid, 0);
    }
}

/// Test 08: Action Admission Envelope Verification for Settlement Actions across P2P.
/// Verifies replay protection, high-water mark sequence validation, and envelope digest binding.
#[test]
fn test_08_p2p_network_envelope_action_admission_plot_claim() {
    let mut net_env = TwoSessionEnvironment::new().expect("Init two session environment");
    let current_tick = 500u64;
    net_env.current_tick = current_tick;

    let alice_id = net_env.session_alice.player_id;
    let bob_id = net_env.session_bob.player_id;

    // Alice constructs a signed plot claim action envelope
    let alice_env = net_env.session_alice.create_signed_action(
        0x00A1, // Municipal Claim Plot
        42u64.to_be_bytes().to_vec(),
        current_tick,
    );

    // Bob's admission verifier admits Alice's action
    assert!(net_env.session_bob.admission_verifier.verify_and_record(&alice_env, current_tick).is_ok());

    // Bob's local settlement charter applies Alice's claim
    assert!(net_env.session_bob.settlement_charter.claim_plot(42, alice_id, current_tick).is_ok());
    assert_eq!(net_env.session_bob.settlement_charter.plots.get(&42).unwrap().owner_id, alice_id);

    // Replay Attack: Adversary resubmits Alice's envelope to Bob's node
    let replay_err = net_env.session_bob.admission_verifier.verify_and_record(&alice_env, current_tick);
    assert_eq!(
        replay_err,
        Err(ActionAdmissionError::StaleOrReplayedSequence { expected_min: 2, got: 1 }),
        "Replayed sequence number must be rejected at consensus admission gate"
    );

    // Bob creates his own signed claim for the same plot 42
    let bob_env = net_env.session_bob.create_signed_action(
        0x00A1,
        42u64.to_be_bytes().to_vec(),
        current_tick,
    );
    assert!(net_env.session_bob.admission_verifier.verify_and_record(&bob_env, current_tick).is_ok());

    // Executing on Bob's settlement charter fails with PlotAlreadyClaimed!
    let bob_claim_err = net_env.session_bob.settlement_charter.claim_plot(42, bob_id, current_tick);
    assert_eq!(bob_claim_err, Err(SettlementError::PlotAlreadyClaimed));
}

/// Test 09: Modular construction verification & homestead property rights.
/// Verifies that only the plot owner can build, structures must be valid primitives,
/// and building on nonexistent plots is blocked.
#[test]
fn test_09_homestead_structural_primitives_construction_and_access_control() {
    let mut charter = SettlementCharter::new(1, "Stonehaven Freehold".to_string(), 1000);
    let owner_id = 1001u64;
    let intruder_id = 1002u64;
    let plot_id = 10u64;

    assert!(charter.claim_plot(plot_id, owner_id, 100).is_ok());

    let valid_wall = StructuralPrimitive {
        style: ArchitecturalStyle::FortifiedCitadel,
        primary_material: MaterialType::Granite,
        secondary_material: MaterialType::SteelAlloy,
        length: Q64::from_int(20).unwrap(),
        height: Q64::from_int(6).unwrap(),
        thickness: Q64::from_int(2).unwrap(),
        features: 0b001,
    };

    // 1. Owner builds valid structure -> Ok
    assert!(charter.build_structure(plot_id, owner_id, valid_wall.clone()).is_ok());
    assert_eq!(charter.plots.get(&plot_id).unwrap().built_structures.len(), 1);

    // 2. Intruder attempts to build on Owner's plot -> Err(NotPlotOwner)
    assert_eq!(
        charter.build_structure(plot_id, intruder_id, valid_wall.clone()),
        Err(SettlementError::NotPlotOwner)
    );

    // 3. Attempt to build on unallocated plot -> Err(PlotNotFound)
    assert_eq!(
        charter.build_structure(999, owner_id, valid_wall.clone()),
        Err(SettlementError::PlotNotFound)
    );

    // 4. Owner attempts to build invalid structure (zero thickness constraint violation)
    let invalid_wall = StructuralPrimitive {
        thickness: Q64::ZERO, // Illegal: zero thickness
        ..valid_wall.clone()
    };
    assert_eq!(
        charter.build_structure(plot_id, owner_id, invalid_wall),
        Err(SettlementError::InvalidStructureConstraints)
    );

    // 5. Build second valid structure (Watchtower) -> stacks cleanly
    let valid_tower = StructuralPrimitive {
        style: ArchitecturalStyle::NordicTimber,
        primary_material: MaterialType::Ironwood,
        secondary_material: MaterialType::ObsidianGlass,
        length: Q64::from_int(8).unwrap(),
        height: Q64::from_int(16).unwrap(),
        thickness: Q64::from_int(1).unwrap(),
        features: 0b110,
    };
    assert!(charter.build_structure(plot_id, owner_id, valid_tower).is_ok());
    assert_eq!(charter.plots.get(&plot_id).unwrap().built_structures.len(), 2);
}

// ============================================================================
// PART 3: MUNICIPAL TREASURY AND TAX BOND INVARIANTS (MIN_TAX_BOND_MARKS = 100)
// ============================================================================

/// Test 10: Verify exact mathematical threshold of MIN_TAX_BOND_MARKS = 100.
/// 99 Marks fails; 100 Marks passes.
#[test]
fn test_10_tax_bond_exact_threshold_invariants() {
    let mut charter = SettlementCharter::new(1, "Ironhold Haven".to_string(), 1000);
    let player_id = 1001u64;
    let t0 = 100u64;
    let election_tick = t0 + MIN_TENURE_TICKS; // Satisfies tenure requirement

    assert!(charter.claim_plot(1, player_id, t0).is_ok());

    // 0 Marks paid -> SybilVotingDisqualified
    assert_eq!(
        charter.can_vote_for_mayor(player_id, election_tick),
        Err(SettlementError::SybilVotingDisqualified)
    );

    // Pay 50 Marks (total: 50 < 100) -> Still disqualified
    assert!(charter.pay_tax(player_id, 50).is_ok());
    assert_eq!(
        charter.can_vote_for_mayor(player_id, election_tick),
        Err(SettlementError::SybilVotingDisqualified)
    );

    // Pay 49 Marks (total: 99 < 100) -> Still disqualified by 1 single Mark!
    assert!(charter.pay_tax(player_id, 49).is_ok());
    assert_eq!(charter.residents.get(&player_id).unwrap().cumulative_taxes_paid, 99);
    assert_eq!(
        charter.can_vote_for_mayor(player_id, election_tick),
        Err(SettlementError::SybilVotingDisqualified),
        "99 Marks must fail with SybilVotingDisqualified"
    );

    // Pay 1 Mark (total: 100 == MIN_TAX_BOND_MARKS) -> EXACT BOUNDARY PASS!
    assert!(charter.pay_tax(player_id, 1).is_ok());
    assert_eq!(charter.residents.get(&player_id).unwrap().cumulative_taxes_paid, 100);
    assert_eq!(
        charter.can_vote_for_mayor(player_id, election_tick),
        Ok(true),
        "Exactly 100 Marks must qualify the resident to vote"
    );

    // Pay additional 50 Marks (total: 150 > 100) -> Still passes
    assert!(charter.pay_tax(player_id, 50).is_ok());
    assert_eq!(charter.residents.get(&player_id).unwrap().cumulative_taxes_paid, 150);
    assert_eq!(charter.can_vote_for_mayor(player_id, election_tick), Ok(true));
}

/// Test 11: Incremental micro-tax levy contributions.
/// Verifies that multiple fractional tax payments correctly accumulate without rounding error.
#[test]
fn test_11_cumulative_incremental_tax_contributions() {
    let mut charter = SettlementCharter::new(1, "Port Luminis".to_string(), 1000);
    let citizen_id = 1005u64;
    let t0 = 0u64;
    let election_tick = MIN_TENURE_TICKS;

    assert!(charter.claim_plot(1, citizen_id, t0).is_ok());

    // Sequence of 7 micro-tax levies
    let levies = [10u64, 15, 20, 5, 25, 15, 10]; // Sum = 100
    let mut running_sum = 0u64;

    for (step, &levy) in levies.iter().enumerate() {
        assert!(charter.pay_tax(citizen_id, levy).is_ok());
        running_sum += levy;

        assert_eq!(
            charter.residents.get(&citizen_id).unwrap().cumulative_taxes_paid,
            running_sum
        );
        assert_eq!(charter.treasury_marks, running_sum);

        if running_sum < MIN_TAX_BOND_MARKS {
            assert_eq!(
                charter.can_vote_for_mayor(citizen_id, election_tick),
                Err(SettlementError::SybilVotingDisqualified),
                "Step {}: At {} Marks, voter must remain disqualified",
                step,
                running_sum
            );
        } else {
            assert_eq!(
                charter.can_vote_for_mayor(citizen_id, election_tick),
                Ok(true),
                "Step {}: At {} Marks, voter must be qualified!",
                step,
                running_sum
            );
        }
    }

    assert_eq!(charter.residents.get(&citizen_id).unwrap().cumulative_taxes_paid, 100);
    assert_eq!(charter.treasury_marks, 100);
}

/// Test 12: Municipal Treasury Conservation and Deficit Spending Prevention.
/// Invariant: Treasury balance equals total taxes deposited minus disbursements.
/// Disbursing more than treasury balance must be rejected.
#[test]
fn test_12_municipal_treasury_exact_conservation_and_overdraft_prevention() {
    let mut charter = SettlementCharter::new(1, "Grand Bastion".to_string(), 1000);
    let p1 = 1001u64;
    let p2 = 1002u64;
    let p3 = 1003u64;

    assert!(charter.claim_plot(1, p1, 0).is_ok());
    assert!(charter.claim_plot(2, p2, 0).is_ok());
    assert!(charter.claim_plot(3, p3, 0).is_ok());

    // Three citizens pay taxes into the municipal treasury
    assert!(charter.pay_tax(p1, 250).is_ok());
    assert!(charter.pay_tax(p2, 450).is_ok());
    assert!(charter.pay_tax(p3, 300).is_ok());

    // Total treasury must be exact sum: 250 + 450 + 300 = 1,000 Marks
    assert_eq!(charter.treasury_marks, 1_000);

    // Public Works Disbursement 1: 400 Marks for Defensive Perimeter Walls
    assert!(charter.disburse_treasury(400).is_ok());
    assert_eq!(charter.treasury_marks, 600);

    // Public Works Disbursement 2: 600 Marks for Public Granary
    assert!(charter.disburse_treasury(600).is_ok());
    assert_eq!(charter.treasury_marks, 0);

    // Deficit Spending / Overdraft Attempt: Disburse 1 Mark from empty treasury
    let overdraft_res = charter.disburse_treasury(1);
    assert_eq!(
        overdraft_res,
        Err(SettlementError::InsufficientTreasuryFunds),
        "Municipal treasury cannot undergo overdraft / negative balance"
    );
    assert_eq!(charter.treasury_marks, 0);
}

/// Test 13: Mayoral Election and Charter Succession.
/// Only a fully qualified resident can assume mayorship.
#[test]
fn test_13_mayoral_succession_and_transfer_of_charter() {
    let mut charter = SettlementCharter::new(1, "Highspire Keep".to_string(), 1000);
    let incumbent_mayor = 1000u64;
    let worthy_candidate = 1001u64;
    let impoverished_candidate = 1002u64;
    let newcomer_candidate = 1003u64;

    let election_tick = 600_000u64;

    // Worthy candidate: registered early, paid taxes
    assert!(charter.claim_plot(1, worthy_candidate, 1_000).is_ok());
    assert!(charter.pay_tax(worthy_candidate, 250).is_ok());

    // Impoverished candidate: registered early, failed to pay taxes
    assert!(charter.claim_plot(2, impoverished_candidate, 1_000).is_ok());
    assert!(charter.pay_tax(impoverished_candidate, 50).is_ok());

    // Newcomer candidate: paid massive tax, registered late
    assert!(charter.claim_plot(3, newcomer_candidate, 550_000).is_ok());
    assert!(charter.pay_tax(newcomer_candidate, 5_000).is_ok());

    // 1. Impoverished candidate cannot be elected
    assert_eq!(
        charter.elect_mayor(impoverished_candidate, election_tick),
        Err(SettlementError::SybilVotingDisqualified)
    );

    // 2. Newcomer candidate cannot be elected
    assert_eq!(
        charter.elect_mayor(newcomer_candidate, election_tick),
        Err(SettlementError::InsufficientTenure)
    );

    // Mayor is still incumbent
    assert_eq!(charter.mayor_id, incumbent_mayor);

    // 3. Worthy candidate elected successfully!
    assert!(charter.elect_mayor(worthy_candidate, election_tick).is_ok());
    assert_eq!(charter.mayor_id, worthy_candidate);
}

/// Test 14: 10-Player Comprehensive Municipal Campaign Simulation.
/// Simulates the 10-player game test campaign across 600,000 ticks:
/// - Agent 1 (Gladiator): Founder, tenure 600,000 ticks, paid 200 Marks
/// - Agent 2 (Ranger): Founder, tenure 600,000 ticks, paid 150 Marks
/// - Agent 3 (Cartographer): Founder, tenure 600,000 ticks, paid 100 Marks
/// - Agent 4 (Blacksmith): Wave 1, tenure 590,000 ticks, paid 300 Marks
/// - Agent 5 (Chaos Monkey): Wave 1, tenure 590,000 ticks, paid 100 Marks
/// - Agent 6 (Governance QA): Wave 1, tenure 590,000 ticks, paid 100 Marks
/// - Agent 7 (Arcanist): Wave 2, tenure 400,000 ticks (< MIN_TENURE_TICKS)
/// - Agent 8 (Tax Evader): Founder, tenure 600,000 ticks, paid 0 Marks (< MIN_TAX_BOND_MARKS)
/// - Agent 9 (Sybil Bot Alpha): Infiltrator, tenure 20,000 ticks, paid 5,000 Marks
/// - Agent 10 (Sybil Bot Beta): Infiltrator, tenure 20,000 ticks, paid 5,000 Marks
#[test]
fn test_14_10_player_municipal_governance_end_to_end_campaign_audit() {
    let mut charter = SettlementCharter::new(1, "Aeviterra Federation Sanctuary".to_string(), 1001);

    struct CampaignPlayer {
        id: u64,
        name: &'static str,
        claim_tick: u64,
        tax_contributed: u64,
        expected_can_vote: bool,
        expected_disqualification: Option<SettlementError>,
    }

    let players = vec![
        CampaignPlayer {
            id: 1001,
            name: "Agent 1: Gladiator Ares",
            claim_tick: 0,
            tax_contributed: 200,
            expected_can_vote: true,
            expected_disqualification: None,
        },
        CampaignPlayer {
            id: 1002,
            name: "Agent 2: Ranger Sylva",
            claim_tick: 0,
            tax_contributed: 150,
            expected_can_vote: true,
            expected_disqualification: None,
        },
        CampaignPlayer {
            id: 1003,
            name: "Agent 3: Cartographer Atlas",
            claim_tick: 0,
            tax_contributed: 100,
            expected_can_vote: true,
            expected_disqualification: None,
        },
        CampaignPlayer {
            id: 1004,
            name: "Agent 4: Blacksmith Vulcan",
            claim_tick: 10_000,
            tax_contributed: 300,
            expected_can_vote: true,
            expected_disqualification: None,
        },
        CampaignPlayer {
            id: 1005,
            name: "Agent 5: Chaos Monkey QA",
            claim_tick: 10_000,
            tax_contributed: 100,
            expected_can_vote: true,
            expected_disqualification: None,
        },
        CampaignPlayer {
            id: 1006,
            name: "Agent 6: Governance QA Auditor",
            claim_tick: 10_000,
            tax_contributed: 100,
            expected_can_vote: true,
            expected_disqualification: None,
        },
        CampaignPlayer {
            id: 1007,
            name: "Agent 7: Arcanist Freya",
            claim_tick: 200_000, // Tenure: 600,000 - 200,000 = 400,000 < 518,400!
            tax_contributed: 100,
            expected_can_vote: false,
            expected_disqualification: Some(SettlementError::InsufficientTenure),
        },
        CampaignPlayer {
            id: 1008,
            name: "Agent 8: Free-Rider Evader",
            claim_tick: 0,
            tax_contributed: 0, // Taxes: 0 < 100!
            expected_can_vote: false,
            expected_disqualification: Some(SettlementError::SybilVotingDisqualified),
        },
        CampaignPlayer {
            id: 1009,
            name: "Agent 9: Sybil Infiltrator Alpha",
            claim_tick: 580_000, // Tenure: 600,000 - 580,000 = 20,000 < 518,400!
            tax_contributed: 5_000,
            expected_can_vote: false,
            expected_disqualification: Some(SettlementError::InsufficientTenure),
        },
        CampaignPlayer {
            id: 1010,
            name: "Agent 10: Sybil Infiltrator Beta",
            claim_tick: 580_000, // Tenure: 600,000 - 580,000 = 20,000 < 518,400!
            tax_contributed: 5_000,
            expected_can_vote: false,
            expected_disqualification: Some(SettlementError::InsufficientTenure),
        },
    ];

    // 1. Process all 10 plot claims
    for (i, p) in players.iter().enumerate() {
        let plot_id = 100 + i as u64;
        assert!(charter.claim_plot(plot_id, p.id, p.claim_tick).is_ok());
        if p.tax_contributed > 0 {
            assert!(charter.pay_tax(p.id, p.tax_contributed).is_ok());
        }
    }

    assert_eq!(charter.plots.len(), 10);
    assert_eq!(charter.residents.len(), 10);

    // Verify treasury accounting invariant: total deposited equals sum of taxes
    let total_expected_treasury: u64 = players.iter().map(|p| p.tax_contributed).sum();
    assert_eq!(charter.treasury_marks, total_expected_treasury);
    assert_eq!(charter.treasury_marks, 11_050);

    // 2. Settlement Election at tick 600,000
    let election_tick = 600_000u64;
    let mut eligible_voters = 0;
    let mut disqualified_voters = 0;

    for p in &players {
        let vote_result = charter.can_vote_for_mayor(p.id, election_tick);
        if p.expected_can_vote {
            assert_eq!(
                vote_result,
                Ok(true),
                "Player {} ({}) must be eligible to vote",
                p.id,
                p.name
            );
            eligible_voters += 1;
        } else {
            assert_eq!(
                vote_result,
                Err(p.expected_disqualification.clone().unwrap()),
                "Player {} ({}) must be disqualified with {:?}",
                p.id,
                p.name,
                p.expected_disqualification
            );
            disqualified_voters += 1;
        }
    }

    assert_eq!(eligible_voters, 6, "Exactly 6 players qualify for mayoral voting");
    assert_eq!(disqualified_voters, 4, "Exactly 4 players are disqualified (tenure or tax)");

    // 3. Mayoral election: Eligible Blacksmith Vulcan is elected Mayor
    assert!(charter.elect_mayor(1004, election_tick).is_ok());
    assert_eq!(charter.mayor_id, 1004);

    // 4. Municipal public infrastructure disbursement: 2,500 Marks for Central Forge
    assert!(charter.disburse_treasury(2_500).is_ok());
    assert_eq!(charter.treasury_marks, 11_050 - 2_500);
    assert_eq!(charter.treasury_marks, 8_550);

    // 5. Final audit check: All 10 plots intact, 10 residents recorded
    assert_eq!(charter.plots.len(), 10);
    assert_eq!(charter.residents.len(), 10);
}

// ============================================================================
// PART 4: ROUND 2 ADVERSARIAL AUDIT & EXPLOIT PROBES
// ============================================================================

/// Test 15: Round 2 Verification — Bounded plot ownership and structure stacking limits (E19).
/// Verifies MAX_PLOTS_PER_RESIDENT = 4 and MAX_STRUCTURES_PER_PLOT = 32 enforcement.
#[test]
fn test_15_round2_adversarial_plot_monopoly_and_structure_capacity_limits() {
    let mut charter = SettlementCharter::new(1, "E19 Defense Freehold".to_string(), 1000);
    let baron_id = 9001u64;
    let other_player = 9002u64;

    // 1. Land Baron claims 4 plots (maximum allowed)
    for i in 1..=4 {
        assert_eq!(charter.claim_plot(i, baron_id, 100), Ok(()));
    }
    assert_eq!(charter.plots.len(), 4);

    // 2. Land Baron attempts to monopolize a 5th plot -> Blocked by ResidentPlotLimitExceeded
    assert_eq!(
        charter.claim_plot(5, baron_id, 101),
        Err(SettlementError::ResidentPlotLimitExceeded),
        "5th plot claim must be blocked by MAX_PLOTS_PER_RESIDENT limit"
    );

    // 3. Second-order bypass attempt: Re-claiming an existing plot does NOT allow a bypass
    assert_eq!(
        charter.claim_plot(1, baron_id, 102),
        Err(SettlementError::PlotAlreadyClaimed)
    );

    // 4. Another legitimate resident CAN claim plot 5
    assert_eq!(charter.claim_plot(5, other_player, 103), Ok(()));
    assert_eq!(charter.plots.len(), 5);

    // 5. Structure stacking limits: Build exactly 32 structures on Plot 1
    let wall = StructuralPrimitive {
        style: ArchitecturalStyle::FortifiedCitadel,
        primary_material: MaterialType::Granite,
        secondary_material: MaterialType::SteelAlloy,
        length: Q64::from_int(10).unwrap(),
        height: Q64::from_int(4).unwrap(),
        thickness: Q64::from_int(2).unwrap(),
        features: 0,
    };

    for _ in 0..MAX_STRUCTURES_PER_PLOT {
        assert_eq!(charter.build_structure(1, baron_id, wall.clone()), Ok(()));
    }
    assert_eq!(charter.plots.get(&1).unwrap().built_structures.len(), 32);

    // 6. Attempting to build 33rd structure -> Blocked by PlotCapacityExceeded
    assert_eq!(
        charter.build_structure(1, baron_id, wall.clone()),
        Err(SettlementError::PlotCapacityExceeded),
        "33rd structure must be blocked by MAX_STRUCTURES_PER_PLOT limit"
    );
    assert_eq!(charter.plots.get(&1).unwrap().built_structures.len(), 32);
}

/// Test 16: Round 2 Adversarial Audit — Integer Boundary & Non-Atomic Mutation Vulnerability Probe.
/// Probes:
/// 1. Non-atomic partial mutation in pay_tax when treasury marks overflow u64::MAX.
/// 2. Arithmetic overflow in tenure calculation when registration_tick is near u64::MAX.
#[test]
fn test_16_round2_adversarial_overflow_and_partial_mutation_vulnerabilities() {
    let mut charter = SettlementCharter::new(1, "Overflow Citadel".to_string(), 1000);
    let actor_id = 9999u64;

    assert!(charter.claim_plot(1, actor_id, 100).is_ok());

    // --- Vulnerability Probe A: Non-Atomic State Mutation on Treasury Overflow ---
    // Seed settlement treasury to near u64::MAX
    charter.treasury_marks = u64::MAX - 50;

    // Resident attempts to pay 100 Marks (MIN_TAX_BOND_MARKS).
    // The treasury cannot hold u64::MAX - 50 + 100, so pay_tax fails with TaxCalculationOverflow.
    let pay_res = charter.pay_tax(actor_id, 100);
    assert_eq!(pay_res, Err(SettlementError::TaxCalculationOverflow));

    // R2-05 Remediation: Atomic rollback leaves cumulative_taxes_paid strictly at 0 when treasury overflows!
    let taxes_recorded = charter.residents.get(&actor_id).unwrap().cumulative_taxes_paid;
    assert_eq!(taxes_recorded, 0, "R2-05 Remediated: Atomic rollback leaves cumulative_taxes_paid at 0 on treasury overflow!");

    // --- Vulnerability Probe B: Election Tick Arithmetic Overflow Near u64::MAX (R2-06 Remediated) ---
    let late_actor_id = 8888u64;
    let late_tick = u64::MAX - 100_000;
    assert!(charter.claim_plot(2, late_actor_id, late_tick).is_ok());

    let vote_check = charter.can_vote_for_mayor(late_actor_id, late_tick);
    assert_eq!(
        vote_check,
        Err(SettlementError::InsufficientTenure),
        "R2-06 Remediated: can_vote_for_mayor handles near-u64::MAX registration ticks without overflow or panic!"
    );
}
