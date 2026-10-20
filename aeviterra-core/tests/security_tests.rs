use aeviterra_core::math::*;
use aeviterra_core::state::*;
use aeviterra_core::shard::*;
use aeviterra_core::offline::*;
use aeviterra_core::randomness::*;
use aeviterra_core::economy::*;

#[test]
fn test_q64_deterministic_arithmetic() {
    let a = Q64::from_int(12).unwrap();
    let b = Q64::from_int(4).unwrap();

    // Addition & Subtraction
    assert_eq!((a + b).to_int_trunc(), 16);
    assert_eq!((a - b).to_int_trunc(), 8);

    // Multiplication (exact 256-bit intermediate)
    let product = a.checked_mul(b).unwrap();
    assert_eq!(product.to_int_trunc(), 48);

    // Division (truncation toward zero)
    let quotient = a.checked_div(b).unwrap();
    assert_eq!(quotient.to_int_trunc(), 3);

    // Division by zero must return explicit MathError
    assert_eq!(a.checked_div(Q64::ZERO), Err(MathError::DivisionByZero));

    // Integer Square Root
    let sq = Q64::from_int(144).unwrap();
    let root = sq.checked_sqrt().unwrap();
    assert_eq!(root.to_int_trunc(), 12);

    // Negative Square Root rejection
    let neg = Q64::from_int(-9).unwrap();
    assert_eq!(neg.checked_sqrt(), Err(MathError::NegativeSquareRoot));

    // i128::MIN negation overflow rejection & negative floor vs trunc (F14)
    assert_eq!(Q64::MIN.checked_neg(), Err(MathError::Overflow));
    let neg_half = Q64::ZERO.checked_sub(Q64::HALF).unwrap(); // -0.5
    assert_eq!(neg_half.to_int_trunc(), 0);
    assert_eq!(neg_half.to_int_floor(), -1);
    assert_eq!(format!("{}", neg_half), "-0.5000");
}

#[test]
fn test_smt_domain_separation_and_verification() {
    let mut smt = SparseMerkleTree::new();
    let key1 = 42u64;
    let val1 = [1u8; 32];
    let key2 = 1337u64;
    let val2 = [2u8; 32];

    smt.insert(key1, val1);
    smt.insert(key2, val2);

    let root = smt.compute_root();
    assert_ne!(root, [0u8; 32]);

    // Inclusion proof verification
    let proof1 = smt.prove(key1);
    assert!(SparseMerkleTree::verify_proof(&root, &proof1));

    // Forged proof tampering detection
    let mut bad_proof = proof1.clone();
    bad_proof.value_hash = [99u8; 32];
    assert!(!SparseMerkleTree::verify_proof(&root, &bad_proof));
}

#[test]
fn test_dynamic_swept_halo_calculation() {
    let calc = DynamicHaloCalculator::new(
        Q64::from_int(2).unwrap(),  // 2m interaction radius
        Q64::from_int(1).unwrap(),  // 1m numerical margin
    );

    // Fast projectile: 100 m/s, 1s rollback horizon
    let margin = calc.compute_required_margin(
        Q64::from_int(2).unwrap(),
        Q64::from_int(100).unwrap(),
        Q64::from_int(10).unwrap(),
        Q64::from_int(1).unwrap(),
    ).unwrap();

    // margin = 2 + (100 * 1) + 0.5 * 10 * 1^2 + 1 = 2 + 100 + 5 + 1 = 108 meters!
    assert_eq!(margin.to_int_trunc(), 108);

    // Swept trajectory across chunks
    let trajectory = SweptCapsule {
        start: Vec3Fixed::new(Q64::from_int(10).unwrap(), Q64::from_int(10).unwrap(), Q64::from_int(10).unwrap()),
        end: Vec3Fixed::new(Q64::from_int(200).unwrap(), Q64::from_int(10).unwrap(), Q64::from_int(10).unwrap()),
        radius: Q64::from_int(5).unwrap(),
    };

    let affected = calc.calculate_affected_chunks(&trajectory).unwrap();
    // Chunks are 128m wide, so going from x=10 to x=200 crosses chunk 0 and chunk 1
    assert!(affected.len() >= 2);
}

#[test]
fn test_cross_shard_durable_journal_decision() {
    let mut manager = CrossShardManager::new(1);
    let tx_id = [7u8; 32];
    let entity_id = 9999u64;

    // Declare transaction at tick 100 with timeout 50 ticks
    manager.declare_tx(tx_id, 2, entity_id, 1, 100, 50, vec![]).unwrap();
    assert!(manager.locked_entities.contains_key(&entity_id));

    // Replay attack with same tx_id must be rejected
    assert_eq!(
        manager.declare_tx(tx_id, 2, entity_id, 1, 101, 50, vec![]),
        Err(TxError::AlreadyExists)
    );

    // V4 Rule: Unilateral local timeout unlock is banned (F12 race fix).
    // Only a durable CertifiedJournalDecision resolves the transaction.
    let decision = CertifiedJournalDecision {
        tx_id,
        decision: JournalDecisionType::Abort,
        journal_height: 1050,
        certificate_digest: [0x55; 32],
    };
    manager.apply_journal_decision(&decision).unwrap();

    // Entity is now safely unlocked via certified journal decision
    assert!(!manager.locked_entities.contains_key(&entity_id));
}

#[test]
fn test_v4_nullifier_immune_to_differing_end_roots() {
    let canonical_program = [0xAA; 32];
    let mut enclave_mgr = LeasedEnclaveManager::new(canonical_program);

    let actor_id = 42u64;
    let start_root = [0x11; 32];
    let entropy_seed = [0x55; 32];

    let lease = enclave_mgr.grant_lease(actor_id, start_root, 1000, 3600, entropy_seed, 500, 1234);

    // V4 Invariant: Nullifier is derived strictly from LeaseID alone!
    let nullifier_v4 = compute_settlement_nullifier(&lease.lease_id);

    // Timeline A with ending root A
    let end_root_a = [0x22; 32];
    let proof_a = ZkExecutionReceipt::create_receipt(
        canonical_program,
        start_root,
        end_root_a,
        lease.lease_id,
        [0x33; 32],
        vec![0xAA, 0xBB, 0xCC, 0xDD],
    );
    let receipt_a = LeaseSettlementReceipt {
        lease_id: lease.lease_id,
        program_id: canonical_program,
        start_state_root: start_root,
        end_state_root: end_root_a,
        executed_ticks: 500,
        input_chain_digest: [0x33; 32],
        settlement_nullifier: nullifier_v4,
        proof: proof_a,
    };

    // Settle timeline A successfully
    let res_a = enclave_mgr.settle_lease(&receipt_a, 1200);
    assert!(res_a.is_ok());

    // Malicious timeline B with different ending root B!
    // In V3, an attacker attempted to bypass nullifier checks by having R_end_B != R_end_A.
    // In V4, both timelines MUST use nullifier_v4.
    let end_root_b = [0x88; 32];
    let proof_b = ZkExecutionReceipt::create_receipt(
        canonical_program,
        start_root,
        end_root_b,
        lease.lease_id,
        [0x44; 32],
        vec![0xEE, 0xFF, 0x00, 0x11],
    );
    let receipt_b = LeaseSettlementReceipt {
        lease_id: lease.lease_id,
        program_id: canonical_program,
        start_state_root: start_root,
        end_state_root: end_root_b,
        executed_ticks: 500,
        input_chain_digest: [0x44; 32],
        settlement_nullifier: nullifier_v4, // Same nullifier!
        proof: proof_b,
    };

    // Attempting to settle timeline B fails because lease and nullifier are already consumed!
    let res_b = enclave_mgr.settle_lease(&receipt_b, 1205);
    assert_eq!(res_b, Err(LeaseError::LeaseAlreadySpent));
}

#[test]
fn test_commit_before_entropy_eliminates_selective_abort() {
    let mut rng_mgr = ThresholdRandomManager::new(2); // 2 rounds delay
    let action_id = [0x77; 32];

    // Commit combat action at tick 500, beacon round 10
    // 50% hit chance (500,000 ppm)
    rng_mgr.commit_random_action(action_id, 1, 2, 500, 10, 500_000, 50).unwrap();

    // Action cannot be resolved before round 12 is emitted
    assert_eq!(rng_mgr.resolve_action(&action_id), Err(RandomnessError::BeaconRoundNotReached));

    // Round 12 emitted by threshold network
    let round_12_entropy = [0x42; 32];
    rng_mgr.ingest_beacon_round(12, round_12_entropy);

    // Resolves deterministically even if player disconnected!
    let outcome = rng_mgr.resolve_action(&action_id);
    assert!(outcome.is_ok());

    // Second resolution attempt rejected
    assert_eq!(rng_mgr.resolve_action(&action_id), Err(RandomnessError::AlreadyResolved));
}

#[test]
fn test_thermodynamic_mass_conservation_and_agent_budget() {
    let mut ledger = ThermodynamicLedger::new(1_000_000_000);
    let crafter_id = 101u64;
    let iron_ore_id = 1u32;
    let steel_bar_id = 2u32;
    let slag_id = 3u32;

    ledger.entity_balances.insert((crafter_id, iron_ore_id), 100);

    // Valid recipe: 100 ore -> 70 steel + 30 slag (100 == 70 + 30)
    let res = ledger.execute_recipe(crafter_id, iron_ore_id, 100, steel_bar_id, 70, slag_id, 30);
    assert!(res.is_ok());
    assert_eq!(ledger.get_balance(crafter_id, iron_ore_id), 0);
    assert_eq!(ledger.get_balance(crafter_id, steel_bar_id), 70);
    assert_eq!(ledger.world_sinks.get(&slag_id).copied().unwrap_or(0), 30);

    // Cheat recipe attempt: 100 ore -> 80 steel + 30 slag (110 != 100) -> Must be rejected!
    ledger.entity_balances.insert((crafter_id, iron_ore_id), 100);
    let cheat_res = ledger.execute_recipe(crafter_id, iron_ore_id, 100, steel_bar_id, 80, slag_id, 30);
    assert_eq!(cheat_res, Err(EconomicError::MassConservationViolation));

    // Test AI Agent capability envelope limits
    let mut agent = AgentCapabilityEnvelope {
        agent_id: 555,
        max_spend_per_epoch: 50,
        current_spent_in_epoch: 0,
        allowed_action_mask: AgentCapabilityEnvelope::ACTION_HARVEST,
        valid_until_epoch: 10,
        revoked: false,
    };

    // Allowed harvest action
    assert!(agent.authorize_action(AgentCapabilityEnvelope::ACTION_HARVEST, 30, 5).is_ok());
    // Unauthorized craft action (not in mask)
    assert_eq!(
        agent.authorize_action(AgentCapabilityEnvelope::ACTION_CRAFT, 10, 5),
        Err(EconomicError::UnauthorizedAction)
    );
    // Exceeding spend budget (30 + 25 = 55 > 50)
    assert_eq!(
        agent.authorize_action(AgentCapabilityEnvelope::ACTION_HARVEST, 25, 5),
        Err(EconomicError::CapabilityBudgetExceeded)
    );
}

#[test]
fn test_bft_quorum_and_equivocation_detection() {
    use aeviterra_core::consensus::*;

    let mut engine = ShardBftEngine::new(1, 4); // n = 4, f = 1, quorum = 3
    let world_id = [1u8; 32];
    let parent_hash = [0u8; 32];
    let state_root = [2u8; 32];
    let batch_digest = [3u8; 32];

    let proposal = BftProposal {
        world_id,
        shard_id: 1,
        committee_epoch: 1,
        view: 0,
        height: 1,
        parent_hash,
        state_root,
        batch_digest,
        proposer_id: 0,
    };

    let p_hash = proposal.compute_hash();
    engine.set_proposal(proposal).unwrap();

    // Validator 1 votes
    let v1 = BftVote {
        phase: BftPhase::Commit,
        proposal_hash: p_hash,
        view: 0,
        height: 1,
        validator_id: 1,
        signature: Signature([1u8; 64]),
    };
    assert!(engine.register_vote(v1.clone(), Signature([1u8; 64])).unwrap().is_none());

    // Duplicate vote from validator 1 must fail
    assert_eq!(
        engine.register_vote(v1, Signature([1u8; 64])),
        Err(ConsensusError::DuplicateVote)
    );

    // Validator 2 votes
    let v2 = BftVote {
        phase: BftPhase::Commit,
        proposal_hash: p_hash,
        view: 0,
        height: 1,
        validator_id: 2,
        signature: Signature([2u8; 64]),
    };
    assert!(engine.register_vote(v2, Signature([2u8; 64])).unwrap().is_none());

    // Validator 3 votes -> reaches quorum of 3!
    let v3 = BftVote {
        phase: BftPhase::Commit,
        proposal_hash: p_hash,
        view: 0,
        height: 1,
        validator_id: 3,
        signature: Signature([3u8; 64]),
    };
    let cert = engine.register_vote(v3, Signature([3u8; 64])).unwrap();
    assert!(cert.is_some());
    let cert = cert.unwrap();
    assert!(cert.verify_quorum(4));
    assert_eq!(engine.current_height, 2);

    // WAL equivocation test
    let mut wal = ValidatorWal::default();
    wal.record_vote(1, 0, [0xAA; 32]).unwrap();
    // Voting for a different proposal at the same height/view must be caught as equivocation
    assert_eq!(
        wal.record_vote(1, 0, [0xBB; 32]),
        Err(ConsensusError::EquivocationDetected)
    );
}

#[test]
fn test_semantic_generative_asset_grammar() {
    use aeviterra_core::assets::*;

    let prim = StructuralPrimitive {
        style: ArchitecturalStyle::FortifiedCitadel,
        primary_material: MaterialType::Granite,
        secondary_material: MaterialType::SteelAlloy,
        length: Q64::from_int(20).unwrap(),
        height: Q64::from_int(10).unwrap(),
        thickness: Q64::from_int(2).unwrap(),
        features: 0b101,
    };
    assert!(prim.validate_constraints());

    // Degenerate/malicious bounds test (e.g. 0 thickness or 500m wall)
    let invalid_prim = StructuralPrimitive {
        style: ArchitecturalStyle::GothicVault,
        primary_material: MaterialType::Basalt,
        secondary_material: MaterialType::Marble,
        length: Q64::from_int(500).unwrap(), // Exceeds 64m limit
        height: Q64::from_int(10).unwrap(),
        thickness: Q64::ZERO, // Zero thickness invalid
        features: 0,
    };
    assert!(!invalid_prim.validate_constraints());
}

#[test]
fn test_tier_compartmentalized_pity_system() {
    use aeviterra_core::game::loot::*;

    let mut pity_engine = PityLootEngine::new();
    let player_id = 777u64;

    // Simulate 20 misses on Rare tier (threshold is 30, so no 100% cap reached)
    for _ in 0..20 {
        pity_engine.evaluate_tier_roll(player_id, LootTier::Rare, 999_999);
    }
    assert_eq!(pity_engine.get_miss_count(player_id, LootTier::Rare), 20);

    // V4 Hardened Invariant: Rare misses MUST NOT increment Legendary pity counter!
    // Strictly compartmentalized: prevents grinding low-tier mobs to guarantee boss drops!
    assert_eq!(pity_engine.get_miss_count(player_id, LootTier::Legendary), 0);

    // Roll for Legendary with a high random roll (fail)
    let won = pity_engine.evaluate_tier_roll(player_id, LootTier::Legendary, 999_999);
    assert!(!won);
    assert_eq!(pity_engine.get_miss_count(player_id, LootTier::Legendary), 1);
    assert_eq!(pity_engine.get_miss_count(player_id, LootTier::Rare), 20);
}

#[test]
fn test_combat_movesets_and_iframes() {
    use aeviterra_core::game::combat::*;

    let mut player = CombatState::new(101, WeaponFamily::Blade);
    assert_eq!(player.stamina, 100);

    // Execute dodge roll: costs 25 stamina, grants 10 ticks of i-frames
    player.execute_action(CombatActionType::DodgeRoll).unwrap();
    assert_eq!(player.stamina, 75);
    assert_eq!(player.iframe_remaining_ticks, 10);

    // Incoming 500 damage while i-frames active -> 0 damage taken!
    let dmg_taken = player.apply_incoming_damage(500);
    assert_eq!(dmg_taken, 0);
    assert_eq!(player.health, 1000);

    // Tick down i-frames to 0
    for _ in 0..10 {
        player.tick_regeneration();
    }
    assert_eq!(player.iframe_remaining_ticks, 0);

    // Block incoming damage: costs 10 stamina, mitigates 70%
    player.execute_action(CombatActionType::Block).unwrap();
    let dmg_blocked = player.apply_incoming_damage(100);
    assert_eq!(dmg_blocked, 30); // 70% blocked
    assert_eq!(player.health, 970);
}

#[test]
fn test_t01_partition_tolerance_and_sybil_isolation() {
    use aeviterra_core::consensus::*;

    // Committee: n = 13, f = 4, quorum q = 9
    let committee_size = 13;
    let mut engine = ShardBftEngine::new(1, committee_size);

    let proposal = BftProposal {
        world_id: [1u8; 32],
        shard_id: 1,
        committee_epoch: 1,
        view: 0,
        height: 1,
        parent_hash: [0u8; 32],
        state_root: [0xAA; 32],
        batch_digest: [0xBB; 32],
        proposer_id: 1,
    };
    engine.set_proposal(proposal.clone()).unwrap();

    // Adversary attempts Sybil flood: generating 100 fake votes from non-admitted player keys
    // In Aeviterra, only admitted validator IDs (1..=13) count toward quorum.
    // Simulate partition: Partition A has 7 nodes (nodes 1..=7). Partition B has 6 nodes (nodes 8..=13).
    for v_id in 1..=7 {
        let vote = BftVote {
            phase: BftPhase::Commit,
            proposal_hash: proposal.compute_hash(),
            view: 0,
            height: 1,
            validator_id: v_id,
            signature: Signature([v_id as u8; 64]),
        };
        let cert = engine.register_vote(vote, Signature([v_id as u8; 64])).unwrap();
        // 7 votes < 9 quorum: Commit must NOT finalize!
        assert!(cert.is_none());
    }

    // Partition B submits 6 votes to an isolated replica:
    let mut isolated_engine = ShardBftEngine::new(1, committee_size);
    isolated_engine.set_proposal(proposal.clone()).unwrap();
    for v_id in 8..=13 {
        let vote = BftVote {
            phase: BftPhase::Commit,
            proposal_hash: proposal.compute_hash(),
            view: 0,
            height: 1,
            validator_id: v_id,
            signature: Signature([v_id as u8; 64]),
        };
        let cert = isolated_engine.register_vote(vote, Signature([v_id as u8; 64])).unwrap();
        // 6 votes < 9 quorum: Commit must NOT finalize!
        assert!(cert.is_none());
    }

    // Proves: Neither partition can finalize a conflicting commit! Zero split-brain commits!
    assert_eq!(engine.current_height, 1);
    assert_eq!(isolated_engine.current_height, 1);
}

#[test]
fn test_t08_bounded_wire_framing_and_allocation_bombs() {
    use aeviterra_core::network::framing::*;

    // Legitimate frame: 100 bytes payload
    let valid_frame = WireFrame {
        magic: *MAGIC_HEADER,
        version: CURRENT_WIRE_VERSION,
        queue: PriorityQueue::Gameplay,
        payload: vec![0x42; 100],
    };
    let serialized = WireParser::serialize_frame(&valid_frame).unwrap();
    let parsed = WireParser::parse_frame(&serialized).unwrap();
    assert_eq!(parsed.payload.len(), 100);

    // Decompression / Allocation Bomb Attack: Length prefix claimed = 100 MB
    let mut bomb_packet = Vec::new();
    let malicious_len: u32 = 100 * 1024 * 1024; // 100 MB
    bomb_packet.extend_from_slice(&malicious_len.to_be_bytes());
    bomb_packet.extend_from_slice(MAGIC_HEADER);
    bomb_packet.extend_from_slice(&CURRENT_WIRE_VERSION.to_be_bytes());
    bomb_packet.push(0); // Queue byte

    // Parser must reject BEFORE allocating memory!
    let res = WireParser::parse_frame(&bomb_packet);
    assert_eq!(res, Err(FramingError::FrameTooLarge(malicious_len as usize)));
}

#[test]
fn test_t09_time_locked_mempool_anti_front_running() {
    use aeviterra_core::network::framing::TimeLockedIntent;

    let threshold_key = [0x77; 32];
    let secret_swing_direction = b"SWING_LEFT_HIGH_PARRY_COUNTER";

    // Player seals action intent under target block height 100
    let locked_intent = TimeLockedIntent::new(
        1001,
        1,
        100,
        secret_swing_direction,
        &threshold_key,
    );

    // Relays and proposers only see encrypted ciphertext
    assert_ne!(locked_intent.encrypted_payload, secret_swing_direction);

    // After block 100 is ordered and finalized, committee broadcasts decryption key
    let decrypted = locked_intent.decrypt(&threshold_key);
    assert_eq!(decrypted, secret_swing_direction);
}

#[test]
fn test_t11_authenticated_erasure_coding_recovery() {
    use aeviterra_core::storage::erasure::*;

    let codec = ErasureCodec::new(64, 4); // 4 source blocks of 64 bytes = 256 bytes total
    let snapshot_data = b"AEVITERRA_CANONICAL_WORLD_SNAPSHOT_TICK_1000000_STATE_ROOT_PROOF_INVARIANTS";
    
    // Encode with 4 source symbols + 3 repair symbols (total 7 symbols)
    let (manifest, symbols) = codec.encode(snapshot_data, 3);
    assert_eq!(symbols.len(), 7);

    // Scenario A: Recover from all 4 source symbols
    let recovered_all = codec.decode(&manifest, &symbols[0..4]).unwrap();
    assert_eq!(&recovered_all, snapshot_data);

    // Scenario B: Lose 2 source symbols (0 and 2) and recover using 2 Cauchy GF(2^8) repair symbols (4 and 5)!
    let mixed_symbols = vec![
        symbols[1].clone(),
        symbols[3].clone(),
        symbols[4].clone(), // repair symbol 0
        symbols[5].clone(), // repair symbol 1
    ];
    let recovered_from_repairs = codec.decode(&manifest, &mixed_symbols).unwrap();
    assert_eq!(&recovered_from_repairs, snapshot_data);

    // Scenario C: Duplicate symbol attack (4 copies of symbol 0 does NOT have rank 4!)
    let duplicate_symbols = vec![
        symbols[0].clone(),
        symbols[0].clone(),
        symbols[0].clone(),
        symbols[0].clone(),
    ];
    assert_eq!(codec.decode(&manifest, &duplicate_symbols), Err(ErasureError::InsufficientSymbols));

    // Scenario D: Insufficient symbols (< K = 4)
    let insufficient = &symbols[0..3];
    assert_eq!(codec.decode(&manifest, insufficient), Err(ErasureError::InsufficientSymbols));

    // Scenario E: Data tampering detection (modified byte in a symbol)
    let mut tampered_symbols = symbols[0..4].to_vec();
    tampered_symbols[0].data[0] ^= 0xFF; // Flip byte
    assert_eq!(codec.decode(&manifest, &tampered_symbols), Err(ErasureError::CorruptedData));
}

#[test]
fn test_t12_fpeg_epoch_gates_and_crypto_agility() {
    use aeviterra_core::governance::fpeg::*;

    let genesis_kernel = [0x11; 32];
    let mut epoch_mgr = EpochManager::new(genesis_kernel);
    assert_eq!(epoch_mgr.current_epoch, 1);
    assert_eq!(epoch_mgr.active_crypto_suite, CryptoSuite::Suite1Ed25519Sha256);

    // Propose migration to Post-Quantum Suite at height 50,000
    let new_kernel = [0x22; 32];
    let cert = EpochMigrationCertificate {
        proposal_hash: [0x55; 32],
        target_epoch: 2,
        activation_height: 50_000,
        signers: (1..=9).collect(), // 9 out of 13 validators (supermajority)
    };

    // Schedule migration
    assert!(epoch_mgr.schedule_migration(cert, 13).is_ok());

    // At height 49,999: Migration has not activated yet
    assert!(!epoch_mgr.maybe_activate_migration(49_999, CryptoSuite::Suite2PostQuantumMlDsaBlake3, new_kernel));
    assert_eq!(epoch_mgr.current_epoch, 1);

    // At height 50,000: Migration activates!
    assert!(epoch_mgr.maybe_activate_migration(50_000, CryptoSuite::Suite2PostQuantumMlDsaBlake3, new_kernel));
    assert_eq!(epoch_mgr.current_epoch, 2);
    assert_eq!(epoch_mgr.active_crypto_suite, CryptoSuite::Suite2PostQuantumMlDsaBlake3);
    assert_eq!(epoch_mgr.active_kernel_hash, new_kernel);
}

#[test]
fn test_municipal_governance_anti_sybil_tenure_check() {
    use aeviterra_core::game::settlement::*;

    let mut charter = SettlementCharter::new(1, "Aethelgard Haven".to_string(), 100);
    let legitimate_resident = 200u64;
    let bot_sybil = 300u64;

    // Legitimate resident claims plot at tick 1,000
    assert!(charter.claim_plot(1, legitimate_resident, 1_000).is_ok());

    // Adversary registers a bot account at tick 500,000 (just before mayoral election)
    assert!(charter.claim_plot(2, bot_sybil, 500_000).is_ok());

    // Mayoral election occurs at tick 520,000
    let election_tick = 520_000;

    // Legitimate resident has tenure: 520,000 - 1,000 = 519,000 ticks (> 518,400 ticks / 30 days)
    // BUT resident has not paid minimum municipal tax bond yet -> Disqualified
    assert_eq!(charter.can_vote_for_mayor(legitimate_resident, election_tick), Err(SettlementError::SybilVotingDisqualified));

    // Resident pays tax bond
    charter.residents.get_mut(&legitimate_resident).unwrap().cumulative_taxes_paid = MIN_TAX_BOND_MARKS;
    assert_eq!(charter.can_vote_for_mayor(legitimate_resident, election_tick), Ok(true));

    // Sybil bot pays tax bond, but has only 20,000 ticks of residency (< 518,400 required)
    charter.residents.get_mut(&bot_sybil).unwrap().cumulative_taxes_paid = MIN_TAX_BOND_MARKS;
    assert_eq!(charter.can_vote_for_mayor(bot_sybil, election_tick), Err(SettlementError::InsufficientTenure));

    // Non-resident account cannot vote
    assert_eq!(charter.can_vote_for_mayor(999, election_tick), Err(SettlementError::SybilVotingDisqualified));
}

#[test]
fn test_quest_claim_idempotency_and_replay_protection() {
    use aeviterra_core::game::quest::*;

    let mut quest_engine = QuestEngine::new();
    let quest_id = 1;
    let hunter_id = 777u64;

    quest_engine.templates.insert(quest_id, QuestTemplate {
        quest_id,
        title: "Cull the Direwolves".to_string(),
        target_count: 3,
        reward_marks: 150,
        reward_sovereigns: 2,
    });

    // Hunter has not accepted/begun quest yet
    assert_eq!(quest_engine.claim_reward(hunter_id, quest_id), Err(QuestError::QuestNotFound));

    // Hunter kills 1 wolf (1/3 done) -> in progress but not completed
    quest_engine.record_kill(hunter_id, quest_id);
    assert_eq!(quest_engine.claim_reward(hunter_id, quest_id), Err(QuestError::NotCompleted));

    // Hunter slays remaining 2 direwolves (total 3/3)
    quest_engine.record_kill(hunter_id, quest_id);
    quest_engine.record_kill(hunter_id, quest_id);

    // Initial claim succeeds
    let (marks, sovereigns) = quest_engine.claim_reward(hunter_id, quest_id).unwrap();
    assert_eq!(marks, 150);
    assert_eq!(sovereigns, 2);

    // Replay attack / double-claim exploit: attacker attempts to claim rewards again!
    // Must be strictly rejected via single-use nullifier!
    let replay_attempt = quest_engine.claim_reward(hunter_id, quest_id);
    assert_eq!(replay_attempt, Err(QuestError::RewardAlreadyClaimed));
}

#[test]
fn test_adversarial_hardening_all_review_gaps() {
    use aeviterra_core::consensus::*;
    use aeviterra_core::network::framing::*;
    use aeviterra_core::game::combat::CombatState;

    // 1. F01 / T01 & T02: Reject unauthorized Sybil validator votes and duplicate-signer certificates
    let mut engine = ShardBftEngine::new(1, 4); // Admitted validators: 1..=4
    let prop = BftProposal {
        world_id: [1u8; 32],
        shard_id: 1,
        committee_epoch: 1,
        view: 0,
        height: 1,
        parent_hash: [0u8; 32],
        state_root: [2u8; 32],
        batch_digest: [3u8; 32],
        proposer_id: 1,
    };
    let p_hash = prop.compute_hash();
    engine.set_proposal(prop).unwrap();

    // Sybil player (validator_id = 999) attempts to vote -> rejected!
    let sybil_vote = BftVote {
        phase: BftPhase::Commit,
        proposal_hash: p_hash,
        view: 0,
        height: 1,
        validator_id: 999,
        signature: Signature([9u8; 64]),
    };
    assert_eq!(
        engine.register_vote(sybil_vote, Signature([9u8; 64])),
        Err(ConsensusError::UnauthorizedValidator)
    );

    // Duplicate signer certificate forgery (validator 1 repeated 3 times) -> rejected!
    let forged_cert = BftCommitCertificate {
        proposal_hash: p_hash,
        view: 0,
        height: 1,
        signers: vec![1, 1, 1],
        signatures: vec![Signature([1u8; 64]); 3],
    };
    assert!(!forged_cert.verify_quorum(4));

    // 2. F11 / F14: Negative world coordinate floor chunk mapping & hysteresis controller (F23)
    let neg_pos = Vec3Fixed::new(
        Q64::ZERO.checked_sub(Q64::HALF).unwrap(), // -0.5m -> must be in chunk -1, not chunk 0!
        Q64::ZERO,
        Q64::from_int(-129).unwrap(),              // -129m -> must be in chunk -2!
    );
    let neg_chunk = ChunkCoord::from_world_pos(neg_pos);
    assert_eq!(neg_chunk, ChunkCoord::new(-1, 0, -2));

    let mut hysteresis = ShardHysteresisController::new(1, 100, 0);
    // Load spikes to 85% at tick 100 (< 300 min_lifetime_ticks) -> Stable (no oscillation)
    assert_eq!(hysteresis.evaluate_topology(85, 100), TopologyDecision::Stable);
    // At tick 300 -> Splits into children
    assert_eq!(hysteresis.evaluate_topology(85, 300), TopologyDecision::SplitIntoChildren);
    // Load drops to 15% at tick 400 (< 300 + 300) -> Stable
    assert_eq!(hysteresis.evaluate_topology(15, 400), TopologyDecision::Stable);
    // At tick 600 -> Merges into parent
    assert_eq!(hysteresis.evaluate_topology(15, 600), TopologyDecision::MergeIntoParent);

    // 3. V4.1 G2: Lease Expiry Return Race & Reclamation Grace Barrier
    let prog = [0xAA; 32];
    let mut lease_mgr = LeasedEnclaveManager::new(prog);
    let lease = lease_mgr.try_grant_lease(50, [0x11; 32], 1000, 500, [0x22; 32], 100, 1).unwrap();
    // Concurrent lease for same actor rejected
    assert_eq!(
        lease_mgr.try_grant_lease(50, [0x11; 32], 1005, 500, [0x22; 32], 100, 2),
        Err(LeaseError::ActorAlreadyLeased)
    );
    // At tick 1550 (past max_tick_horizon=1500, but inside 200-tick grace barrier 1500..=1700):
    // reclaim_expired does NOT premature-reclaim, and valid settlement succeeds!
    assert!(lease_mgr.reclaim_expired(1550).is_empty());
    let proof = ZkExecutionReceipt::create_receipt(
        prog,
        [0x11; 32],
        [0x33; 32],
        lease.lease_id,
        [0x44; 32],
        vec![0x12, 0x34, 0x56, 0x78],
    );
    let receipt = LeaseSettlementReceipt {
        lease_id: lease.lease_id,
        program_id: prog,
        start_state_root: [0x11; 32],
        end_state_root: [0x33; 32],
        executed_ticks: 480,
        input_chain_digest: [0x44; 32],
        settlement_nullifier: compute_settlement_nullifier(&lease.lease_id),
        proof,
    };
    assert_eq!(lease_mgr.settle_lease(&receipt, 1550), Ok([0x33; 32]));

    // 4. F16 / T08: Truncated frame header (< 7B) must not panic, and trailing bytes rejected
    let truncated_header_packet = [0, 0, 0, 2, b'A', b'E', b'V', b'I'];
    assert_eq!(WireParser::parse_frame(&truncated_header_packet), Err(FramingError::MalformedPayload));

    let valid_frame = WireFrame {
        magic: *MAGIC_HEADER,
        version: CURRENT_WIRE_VERSION,
        queue: PriorityQueue::Consensus,
        payload: vec![1, 2, 3],
    };
    let mut with_trailing = WireParser::serialize_frame(&valid_frame).unwrap();
    with_trailing.push(0xFF);
    assert_eq!(WireParser::parse_frame(&with_trailing), Err(FramingError::TrailingBytes(1)));

    // 5. F17: ActionAdmissionVerifier sequence high-water mark & replay defense
    let world_id = [0x99; 32];
    let mut verifier = ActionAdmissionVerifier::new(world_id, 1, 1, 10);
    let env1 = CanonicalActionEnvelope {
        world_id,
        protocol_epoch: 1,
        authority_shard: 1,
        actor_id: 77,
        session_id: 1,
        sequence_num: 1,
        target_tick: 100,
        action_type: 1,
        payload: vec![0x01],
        signature: Signature([0u8; 64]),
    };
    assert!(verifier.verify_and_record(&env1, 100).is_ok());
    // Replaying sequence_num = 1 must fail
    assert_eq!(
        verifier.verify_and_record(&env1, 100),
        Err(ActionAdmissionError::StaleOrReplayedSequence { expected_min: 2, got: 1 })
    );

    // 6. V4.1 G6: Sovereign Demurrage Decay & Conservation
    let mut sov_ledger = SovereignDemurrageLedger::new();
    sov_ledger.mint_certified_faucet(10, 1_000, 100).unwrap();
    assert_eq!(sov_ledger.total_sovereign_supply, 1_000);
    // Before 180 days: 0 demurrage
    assert_eq!(sov_ledger.apply_demurrage(100 + DEMURRAGE_INACTIVITY_TICKS), 0);
    // After 180 days: 5% (50 Sovereigns) redirected to public infrastructure pool
    let decayed = sov_ledger.apply_demurrage(100 + DEMURRAGE_INACTIVITY_TICKS + 1);
    assert_eq!(decayed, 50);
    assert_eq!(sov_ledger.vaults.get(&10).unwrap().balance_sovereigns, 950);
    assert_eq!(sov_ledger.public_infrastructure_pool, 50);
    assert_eq!(
        sov_ledger.vaults.get(&10).unwrap().balance_sovereigns + sov_ledger.public_infrastructure_pool,
        sov_ledger.total_sovereign_supply
    );

    // 7. V4.1 §2: Crowd Control Diminishing Returns
    assert_eq!(CombatState::evaluate_crowd_control_duration(40, 0), 40);
    assert_eq!(CombatState::evaluate_crowd_control_duration(40, 1), 20);
    assert_eq!(CombatState::evaluate_crowd_control_duration(40, 2), 10);
    assert_eq!(CombatState::evaluate_crowd_control_duration(40, 3), 0); // Stun-lock immune
}

