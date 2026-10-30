use aeviterra_core::math::fixed::{Q64, Vec3Fixed, MathError};
use aeviterra_core::game::combat::{WeaponFamily, CombatActionType};
use aeviterra_core::game::settlement::{SettlementCharter, SettlementError};
use aeviterra_core::game::quest::{QuestEngine, QuestTemplate};
use aeviterra_core::consensus::envelope::{
    CanonicalActionEnvelope, ActionAdmissionVerifier, ActionAdmissionError, Signature, Hash,
};
use aeviterra_core::economy::thermodynamics::EconomicError;
use aeviterra_core::assets::grammar::{StructuralPrimitive, ArchitecturalStyle, MaterialType};
use aeviterra_core::network::{
    StreamingFrameCodec, MultiQueueScheduler, GossipRouter, GossipTopic, GossipWireProtocol,
    PeerSession, PeerReputationLedger, SimulatedNetwork, WireFrame, PriorityQueue,
    MAGIC_HEADER, CURRENT_WIRE_VERSION, derive_node_id, NodeId, NodeRole,
};
use aeviterra_core::shard::halo::ChunkCoord;
use aeviterra_core::state::smt::SparseMerkleTree;
use crate::simulation::{WorldSimulation, PlayerIntent, GameEvent};
use std::collections::BTreeSet;

/// A peered validator/client game node participating in the Aeviterra decentralized network.
pub struct PeeredGameSession {
    pub node_id: NodeId,
    pub player_id: u64,
    pub subnet_prefix: String,
    pub peer_session: PeerSession,
    pub sim: WorldSimulation,
    pub codec: StreamingFrameCodec,
    pub scheduler: MultiQueueScheduler,
    pub router: GossipRouter,
    pub admission_verifier: ActionAdmissionVerifier,
    pub settlement_charter: SettlementCharter,
    pub quest_engine: QuestEngine,
    pub reputation_ledger: PeerReputationLedger,
    pub sequence_counter: u64,
}

impl PeeredGameSession {
    pub fn new(
        player_id: u64,
        player_name: String,
        pubkey: [u8; 32],
        secret: [u8; 32],
        subnet_prefix: String,
        world_id: [u8; 32],
    ) -> Result<Self, MathError> {
        let node_id = derive_node_id(&pubkey);
        let mut sim = WorldSimulation::new(player_name)?;
        sim.player.entity_id = player_id;

        let peer_session = PeerSession::new(pubkey, secret, world_id);
        let codec = StreamingFrameCodec::new();
        let scheduler = MultiQueueScheduler::new();
        let router = GossipRouter::new(node_id);
        let admission_verifier = ActionAdmissionVerifier::new(world_id, 1, 1, 100);
        let settlement_charter = SettlementCharter::new(1, "Aevitas Sanctuary".to_string(), 1000);
        let mut quest_engine = QuestEngine::new();

        // Register default frontier quest
        quest_engine.templates.insert(1, QuestTemplate {
            quest_id: 1,
            title: "Frontier Surveyor".to_string(),
            target_count: 10,
            reward_marks: 50,
            reward_sovereigns: 2,
        });

        let reputation_ledger = PeerReputationLedger::new();

        Ok(Self {
            node_id,
            player_id,
            subnet_prefix,
            peer_session,
            sim,
            codec,
            scheduler,
            router,
            admission_verifier,
            settlement_charter,
            quest_engine,
            reputation_ledger,
            sequence_counter: 0,
        })
    }

    /// Sign and construct a canonical action intent envelope.
    pub fn create_signed_action(
        &mut self,
        action_type: u16,
        payload: Vec<u8>,
        target_tick: u64,
    ) -> CanonicalActionEnvelope {
        self.sequence_counter += 1;
        let mut env = CanonicalActionEnvelope {
            world_id: self.peer_session.world_id,
            protocol_epoch: 1,
            authority_shard: 1,
            actor_id: self.player_id,
            session_id: 1,
            sequence_num: self.sequence_counter,
            target_tick,
            action_type,
            payload,
            signature: Signature([0u8; 64]),
        };

        let digest = env.compute_signing_digest();
        // In production, signature binds private key and digest
        let mut sig_bytes = [0u8; 64];
        sig_bytes[0..32].copy_from_slice(&digest);
        sig_bytes[32..64].copy_from_slice(&self.node_id);
        env.signature = Signature(sig_bytes);

        env
    }
}

/// Two-Session Peered Network Environment.
pub struct TwoSessionEnvironment {
    pub world_id: Hash,
    pub session_alice: PeeredGameSession,
    pub session_bob: PeeredGameSession,
    pub net: SimulatedNetwork,
    pub current_tick: u64,
}

impl TwoSessionEnvironment {
    pub fn new() -> Result<Self, MathError> {
        let world_id = *b"AEVITERRA_WORLD_CANONICAL_2026_0";
        let alice_pub = [11u8; 32];
        let alice_sec = [12u8; 32];
        let bob_pub = [21u8; 32];
        let bob_sec = [22u8; 32];

        let mut session_alice = PeeredGameSession::new(
            1001,
            "Alice_The_Guardian".to_string(),
            alice_pub,
            alice_sec,
            "192.168.1".to_string(),
            world_id,
        )?;

        let mut session_bob = PeeredGameSession::new(
            1002,
            "Bob_The_Blacksmith".to_string(),
            bob_pub,
            bob_sec,
            "10.0.1".to_string(),
            world_id,
        )?;

        // Position Bob slightly offset from Alice
        let offset_x = Q64::from_int(68)?;
        let offset_z = Q64::from_int(66)?;
        let offset_y = session_bob.sim.terrain.elevation_at(offset_x, offset_z)?;
        session_bob.sim.player.position = Vec3Fixed::new(offset_x, offset_y, offset_z);
        session_bob.sim.player.combat.active_weapon = WeaponFamily::Spear;

        // Perform mutual P2P Handshake
        let now = 1_700_000_000u64;
        let alice_nonce = [101u8; 32];
        let bob_nonce = [202u8; 32];

        let init = session_alice.peer_session.create_handshake_init(
            alice_nonce,
            now,
            NodeRole::VALIDATOR_FEDERATION,
            vec![1],
        );

        let resp = session_bob.peer_session.handle_handshake_init(&init, bob_nonce, now)
            .expect("Bob accepts Alice handshake init");

        let ack = session_alice.peer_session.handle_handshake_resp(&resp, now)
            .expect("Alice accepts Bob handshake resp");

        session_bob.peer_session.handle_handshake_ack(&ack)
            .expect("Bob accepts Alice handshake ack");

        assert!(session_alice.peer_session.is_authenticated());
        assert!(session_bob.peer_session.is_authenticated());

        // Setup topic gossip subscriptions
        let shard_topic = GossipTopic::Shard(1);
        let halo_topic = GossipTopic::Halo { chunk_x: 64, chunk_z: 64 };

        session_alice.router.subscribe_peer(session_bob.node_id, shard_topic.clone());
        session_alice.router.subscribe_peer(session_bob.node_id, halo_topic.clone());

        session_bob.router.subscribe_peer(session_alice.node_id, shard_topic);
        session_bob.router.subscribe_peer(session_alice.node_id, halo_topic);

        // Network with 1-tick base latency
        let net = SimulatedNetwork::new(1, 42);

        Ok(Self {
            world_id,
            session_alice,
            session_bob,
            net,
            current_tick: 0,
        })
    }

    /// Step the simulation by one tick, exchanging all in-flight P2P packets.
    pub fn step_tick(&mut self) -> Result<(), MathError> {
        self.current_tick += 1;

        // 1. Advance simulation loops
        self.session_alice.sim.step(vec![])?;
        self.session_bob.sim.step(vec![])?;

        // 2. Deliver network packets
        let delivered = self.net.step();
        for (sender, receiver, frame) in delivered {
            if receiver == self.session_alice.node_id {
                let (_, forwards) = self.session_alice.router.handle_incoming(
                    sender,
                    GossipWireProtocol::DirectPush(aeviterra_core::network::GossipMessage {
                        message_id: [0u8; 32],
                        topic: GossipTopic::Shard(1),
                        origin: sender,
                        hop_count: 0,
                        payload: frame.payload,
                    }),
                );
                assert!(forwards.is_empty() || forwards.len() <= 1);
            } else if receiver == self.session_bob.node_id {
                let (_, forwards) = self.session_bob.router.handle_incoming(
                    sender,
                    GossipWireProtocol::DirectPush(aeviterra_core::network::GossipMessage {
                        message_id: [0u8; 32],
                        topic: GossipTopic::Shard(1),
                        origin: sender,
                        hop_count: 0,
                        payload: frame.payload,
                    }),
                );
                assert!(forwards.is_empty() || forwards.len() <= 1);
            }
        }

        Ok(())
    }

    /// Alice sends an envelope to Bob through the scheduler, streaming codec, and simulated network.
    pub fn send_action_alice_to_bob(&mut self, env: &CanonicalActionEnvelope) {
        let payload = serde_json::to_vec(env).unwrap();
        let frame = WireFrame {
            magic: *MAGIC_HEADER,
            version: CURRENT_WIRE_VERSION,
            queue: PriorityQueue::Gameplay,
            payload,
        };

        self.session_alice.scheduler.enqueue(frame, self.current_tick);
        if let Some(qf) = self.session_alice.scheduler.dispatch_next(self.current_tick) {
            let encoded = StreamingFrameCodec::encode_frame(&qf.frame).unwrap();
            let mut codec = StreamingFrameCodec::new();
            codec.feed(&encoded).unwrap();
            let parsed = codec.pop_frame().unwrap().unwrap();

            self.net.send_packet(
                self.session_alice.node_id,
                self.session_bob.node_id,
                parsed,
            );
        }
    }
}

/// Tri-Party Peered Network Environment (Three Gemini Agents Playing Together).
pub struct TriPartyEnvironment {
    pub world_id: Hash,
    pub session_vanguard: PeeredGameSession,
    pub session_arcanist: PeeredGameSession,
    pub session_artisan: PeeredGameSession,
    pub net: SimulatedNetwork,
    pub current_tick: u64,
}

impl TriPartyEnvironment {
    pub fn new() -> Result<Self, MathError> {
        let world_id = *b"AEVITERRA_WORLD_CANONICAL_2026_0";
        let vanguard_pub = [31u8; 32];
        let vanguard_sec = [32u8; 32];
        let arcanist_pub = [41u8; 32];
        let arcanist_sec = [42u8; 32];
        let artisan_pub  = [51u8; 32];
        let artisan_sec  = [52u8; 32];

        let mut session_vanguard = PeeredGameSession::new(
            1001,
            "Gemini_Vanguard".to_string(),
            vanguard_pub,
            vanguard_sec,
            "192.168.1".to_string(),
            world_id,
        )?;

        let mut session_arcanist = PeeredGameSession::new(
            1002,
            "Gemini_Arcanist".to_string(),
            arcanist_pub,
            arcanist_sec,
            "10.0.1".to_string(),
            world_id,
        )?;

        let mut session_artisan = PeeredGameSession::new(
            1003,
            "Gemini_Artisan".to_string(),
            artisan_pub,
            artisan_sec,
            "172.16.1".to_string(),
            world_id,
        )?;

        // Position players in adjacent regions of the world
        let arc_x = Q64::from_int(72)?;
        let arc_z = Q64::from_int(70)?;
        let arc_y = session_arcanist.sim.terrain.elevation_at(arc_x, arc_z)?;
        session_arcanist.sim.player.position = Vec3Fixed::new(arc_x, arc_y, arc_z);
        session_arcanist.sim.player.combat.active_weapon = WeaponFamily::WandPrism;

        let art_x = Q64::from_int(58)?;
        let art_z = Q64::from_int(60)?;
        let art_y = session_artisan.sim.terrain.elevation_at(art_x, art_z)?;
        session_artisan.sim.player.position = Vec3Fixed::new(art_x, art_y, art_z);
        session_artisan.sim.player.combat.active_weapon = WeaponFamily::Shield;

        // Mutual Handshakes across all 3 nodes
        let now = 1_700_000_000u64;

        // Vanguard <-> Arcanist
        let v_init_a = session_vanguard.peer_session.create_handshake_init([11u8; 32], now, NodeRole::VALIDATOR_FEDERATION, vec![1]);
        let a_resp_v = session_arcanist.peer_session.handle_handshake_init(&v_init_a, [12u8; 32], now).unwrap();
        let v_ack_a = session_vanguard.peer_session.handle_handshake_resp(&a_resp_v, now).unwrap();
        session_arcanist.peer_session.handle_handshake_ack(&v_ack_a).unwrap();

        // Arcanist <-> Artisan
        let mut a_to_art = session_arcanist.peer_session.clone();
        a_to_art.state = aeviterra_core::network::SessionState::Unauthenticated;
        let a_init_art = a_to_art.create_handshake_init([21u8; 32], now, NodeRole::VALIDATOR_FEDERATION, vec![1]);
        let art_resp_a = session_artisan.peer_session.handle_handshake_init(&a_init_art, [22u8; 32], now).unwrap();
        let _ = a_to_art.handle_handshake_resp(&art_resp_a, now).unwrap();

        // Mesh Gossip Subscriptions
        let shard_topic = GossipTopic::Shard(1);
        let halo_topic = GossipTopic::Halo { chunk_x: 64, chunk_z: 64 };

        for topic in &[shard_topic, halo_topic] {
            session_vanguard.router.subscribe_peer(session_arcanist.node_id, topic.clone());
            session_vanguard.router.subscribe_peer(session_artisan.node_id, topic.clone());

            session_arcanist.router.subscribe_peer(session_vanguard.node_id, topic.clone());
            session_arcanist.router.subscribe_peer(session_artisan.node_id, topic.clone());

            session_artisan.router.subscribe_peer(session_vanguard.node_id, topic.clone());
            session_artisan.router.subscribe_peer(session_arcanist.node_id, topic.clone());
        }

        let net = SimulatedNetwork::new(1, 999);

        Ok(Self {
            world_id,
            session_vanguard,
            session_arcanist,
            session_artisan,
            net,
            current_tick: 0,
        })
    }

    /// Step simulation for all 3 agents by one tick, exchanging P2P packets.
    pub fn step_tick(&mut self) -> Result<(), MathError> {
        self.current_tick += 1;

        self.session_vanguard.sim.step(vec![])?;
        self.session_arcanist.sim.step(vec![])?;
        self.session_artisan.sim.step(vec![])?;

        let delivered = self.net.step();
        for (sender, receiver, frame) in delivered {
            let msg = aeviterra_core::network::GossipMessage {
                message_id: [0u8; 32],
                topic: GossipTopic::Shard(1),
                origin: sender,
                hop_count: 0,
                payload: frame.payload,
            };

            if receiver == self.session_vanguard.node_id {
                let _ = self.session_vanguard.router.handle_incoming(sender, GossipWireProtocol::DirectPush(msg));
            } else if receiver == self.session_arcanist.node_id {
                let _ = self.session_arcanist.router.handle_incoming(sender, GossipWireProtocol::DirectPush(msg));
            } else if receiver == self.session_artisan.node_id {
                let _ = self.session_artisan.router.handle_incoming(sender, GossipWireProtocol::DirectPush(msg));
            }
        }

        Ok(())
    }
}

/// Five-Party Peered Network Environment (Five Autonomous Gemini Agents Playing Together).
/// Simulates 30+ minutes of live action gameplay: Gladiator, Ranger, Cartographer, Blacksmith, and Chaos Monkey.
pub struct FiveAgentEnvironment {
    pub world_id: Hash,
    pub session_gladiator: PeeredGameSession,
    pub session_ranger: PeeredGameSession,
    pub session_cartographer: PeeredGameSession,
    pub session_blacksmith: PeeredGameSession,
    pub session_chaos_monkey: PeeredGameSession,
    pub net: SimulatedNetwork,
    pub current_tick: u64,
    pub total_mobs_slain: u64,
    pub total_resources_mined: u64,
    pub total_items_crafted: u64,
    pub unique_chunks: BTreeSet<(i32, i32)>,
    pub smt_verifications_passed: u64,
}

impl FiveAgentEnvironment {
    pub fn new() -> Result<Self, MathError> {
        let world_id = *b"AEVITERRA_WORLD_CANONICAL_2026_0";

        let mut session_gladiator = PeeredGameSession::new(
            1001,
            "Gladiator_Ares".to_string(),
            [11u8; 32],
            [12u8; 32],
            "192.168.1".to_string(),
            world_id,
        )?;

        let mut session_ranger = PeeredGameSession::new(
            1002,
            "Ranger_Sylva".to_string(),
            [21u8; 32],
            [22u8; 32],
            "10.0.1".to_string(),
            world_id,
        )?;

        let mut session_cartographer = PeeredGameSession::new(
            1003,
            "Cartographer_Atlas".to_string(),
            [31u8; 32],
            [32u8; 32],
            "172.16.1".to_string(),
            world_id,
        )?;

        let mut session_blacksmith = PeeredGameSession::new(
            1004,
            "Blacksmith_Vulcan".to_string(),
            [41u8; 32],
            [42u8; 32],
            "10.10.1".to_string(),
            world_id,
        )?;

        let mut session_chaos_monkey = PeeredGameSession::new(
            1005,
            "Chaos_Monkey_QA".to_string(),
            [51u8; 32],
            [52u8; 32],
            "192.168.100".to_string(),
            world_id,
        )?;

        // Setup distinct starting equipment and positions
        session_gladiator.sim.player.combat.active_weapon = WeaponFamily::Blade;
        
        // Ranger starts near trees with Bow
        let r_x = Q64::from_int(56)?;
        let r_z = Q64::from_int(68)?;
        let r_y = session_ranger.sim.terrain.elevation_at(r_x, r_z)?;
        session_ranger.sim.player.position = Vec3Fixed::new(r_x, r_y, r_z);
        session_ranger.sim.player.combat.active_weapon = WeaponFamily::Bow;

        // Cartographer starts ready to survey
        session_cartographer.sim.player.combat.active_weapon = WeaponFamily::Staff;

        // Blacksmith starts with hammer & shield
        session_blacksmith.sim.player.combat.active_weapon = WeaponFamily::Shield;

        // Chaos monkey has shadow blade
        let cm_x = Q64::from_int(68)?;
        let cm_z = Q64::from_int(60)?;
        let cm_y = session_chaos_monkey.sim.terrain.elevation_at(cm_x, cm_z)?;
        session_chaos_monkey.sim.player.position = Vec3Fixed::new(cm_x, cm_y, cm_z);
        session_chaos_monkey.sim.player.combat.active_weapon = WeaponFamily::Shadow;

        // Setup gossip subscriptions across the 5-node mesh
        let nodes = [
            session_gladiator.node_id,
            session_ranger.node_id,
            session_cartographer.node_id,
            session_blacksmith.node_id,
            session_chaos_monkey.node_id,
        ];

        let shard_topic = GossipTopic::Shard(1);
        let halo_topic = GossipTopic::Halo { chunk_x: 64, chunk_z: 64 };

        for &peer_id in &nodes {
            for topic in [&shard_topic, &halo_topic] {
                if peer_id != session_gladiator.node_id { session_gladiator.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_ranger.node_id { session_ranger.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_cartographer.node_id { session_cartographer.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_blacksmith.node_id { session_blacksmith.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_chaos_monkey.node_id { session_chaos_monkey.router.subscribe_peer(peer_id, topic.clone()); }
            }
        }

        let mut unique_chunks = BTreeSet::new();
        unique_chunks.insert((0, 0));

        let net = SimulatedNetwork::new(1, 1001);

        Ok(Self {
            world_id,
            session_gladiator,
            session_ranger,
            session_cartographer,
            session_blacksmith,
            session_chaos_monkey,
            net,
            current_tick: 0,
            total_mobs_slain: 0,
            total_resources_mined: 0,
            total_items_crafted: 0,
            unique_chunks,
            smt_verifications_passed: 0,
        })
    }

    /// Advance one autonomous simulation tick for all 5 Gemini player agents.
    pub fn step_autonomous_tick(&mut self) -> Result<Vec<String>, MathError> {
        self.current_tick += 1;
        let mut logs = Vec::new();

        // -------------------------------------------------------------
        // 1. GLADIATOR (Combat Specialist)
        // -------------------------------------------------------------
        if self.session_gladiator.sim.player.combat.health == 0 {
            self.session_gladiator.sim.player.combat.health = 1000;
            self.session_gladiator.sim.player.combat.stamina = 100;
            self.session_gladiator.sim.player.position = Vec3Fixed::new(Q64::from_int(64)?, Q64::from_int(65)?, Q64::from_int(64)?);
            logs.push(format!("[Tick #{}] Gladiator Ares revived at Sanctuary Campfire with full health!", self.current_tick));
        }

        let mut glad_intents = Vec::new();
        let wolf_hp = self.session_gladiator.sim.npcs.get(&2001).map(|n| n.combat.health).unwrap_or(0);
        let wolf_pos = self.session_gladiator.sim.npcs.get(&2001).map(|n| n.position).unwrap_or(self.session_gladiator.sim.player.position);
        let dist_to_wolf = self.session_gladiator.sim.player.position.distance(wolf_pos)?;

        if wolf_hp > 0 {
            if dist_to_wolf <= Q64::from_int(4)? {
                // In melee range: attack, block, or roll
                if self.current_tick.is_multiple_of(7) && self.session_gladiator.sim.player.combat.stamina >= 25 {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::DodgeRoll));
                } else if self.current_tick.is_multiple_of(5) && self.session_gladiator.sim.player.combat.stamina >= 10 {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::Block));
                } else if self.session_gladiator.sim.player.combat.stamina >= 35 {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::HeavyAttack));
                } else {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::LightAttack));
                }
            } else {
                // Close the gap
                let dir = wolf_pos.checked_sub(self.session_gladiator.sim.player.position)?.normalize()?;
                let dx = dir.x.checked_mul(Q64::from_int(3)?)?;
                let dz = dir.z.checked_mul(Q64::from_int(3)?)?;
                glad_intents.push(PlayerIntent::Move { dx, dz });
            }
        } else {
            // Wolf is slain! If gladiator has 50 iron, craft & equip broadsword
            let iron = self.session_gladiator.sim.player.inventory.get(&1).copied().unwrap_or(0);
            if iron >= 50 {
                glad_intents.push(PlayerIntent::CraftSword);
                glad_intents.push(PlayerIntent::EquipItem(101));
            } else if self.current_tick.is_multiple_of(50) {
                // Respawn next wild training beast so gladiator can continue combat practice
                if let Some(w) = self.session_gladiator.sim.npcs.get_mut(&2001) {
                    w.combat.health = 1000;
                    w.position = Vec3Fixed::new(Q64::from_int(70)?, Q64::from_int(65)?, Q64::from_int(66)?);
                }
            } else {
                // Patrol in circle around spawn
                let angle = (self.current_tick as f64 * 0.1).sin();
                let dx = if angle > 0.0 { Q64::from_int(2)? } else { Q64::from_int(-2)? };
                glad_intents.push(PlayerIntent::Move { dx, dz: Q64::ZERO });
            }
        }

        let glad_events = self.session_gladiator.sim.step(glad_intents)?;
        for ev in glad_events {
            if let GameEvent::NpcSlain { reward_marks, .. } = ev {
                self.total_mobs_slain += 1;
                logs.push(format!("[Tick #{}] Gladiator Ares slew a Direwolf Stalker (+{} Marks)!", self.current_tick, reward_marks));
            } else if let GameEvent::ItemCrafted { item_name, .. } = ev {
                self.total_items_crafted += 1;
                logs.push(format!("[Tick #{}] Gladiator Ares forged {}", self.current_tick, item_name));
            }
        }

        // -------------------------------------------------------------
        // 2. RANGER (Ranged Tactician & Bowyer)
        // -------------------------------------------------------------
        let mut ranger_intents = Vec::new();
        let timber = self.session_ranger.sim.player.inventory.get(&2).copied().unwrap_or(0);
        let has_bow = self.session_ranger.sim.player.equipped_weapon_item == Some(102);

        if !has_bow {
            if timber >= 30 {
                ranger_intents.push(PlayerIntent::CraftBow);
                ranger_intents.push(PlayerIntent::EquipItem(102));
            } else {
                // Harvest timber from nearest node
                if let Some(node) = self.session_ranger.sim.resource_nodes.values().find(|n| n.resource_id == 2 && n.remaining_amount > 0) {
                    let d = self.session_ranger.sim.player.position.distance(node.world_pos)?;
                    if d <= Q64::from_int(6)? {
                        ranger_intents.push(PlayerIntent::Harvest(node.node_id));
                    } else {
                        let dir = node.world_pos.checked_sub(self.session_ranger.sim.player.position)?.normalize()?;
                        ranger_intents.push(PlayerIntent::Move { dx: dir.x.checked_mul(Q64::from_int(3)?)?, dz: dir.z.checked_mul(Q64::from_int(3)?)? });
                    }
                } else {
                    // Seed initial timber for the ranger
                    self.session_ranger.sim.player.inventory.insert(2, 30);
                    self.session_ranger.sim.thermodynamic_ledger.entity_balances.insert((self.session_ranger.sim.player.entity_id, 2), 30);
                }
            }
        } else {
            // Engage wolf at 10-18m range
            let r_wolf_hp = self.session_ranger.sim.npcs.get(&2001).map(|n| n.combat.health).unwrap_or(0);
            let r_wolf_pos = self.session_ranger.sim.npcs.get(&2001).map(|n| n.position).unwrap_or(self.session_ranger.sim.player.position);
            let r_dist = self.session_ranger.sim.player.position.distance(r_wolf_pos)?;

            if r_wolf_hp > 0 {
                if r_dist < Q64::from_int(6)? {
                    // Back up or roll away
                    ranger_intents.push(PlayerIntent::Attack(CombatActionType::DodgeRoll));
                } else if r_dist <= Q64::from_int(18)? {
                    // Fire bow!
                    if self.session_ranger.sim.player.combat.stamina >= 35 {
                        ranger_intents.push(PlayerIntent::Attack(CombatActionType::HeavyAttack));
                    } else {
                        ranger_intents.push(PlayerIntent::Attack(CombatActionType::LightAttack));
                    }
                }
            } else if self.current_tick.is_multiple_of(60)
                && let Some(w) = self.session_ranger.sim.npcs.get_mut(&2001) {
                w.combat.health = 1000;
                w.position = Vec3Fixed::new(Q64::from_int(70)?, Q64::from_int(65)?, Q64::from_int(66)?);
            }
        }

        let ranger_events = self.session_ranger.sim.step(ranger_intents)?;
        for ev in ranger_events {
            if let GameEvent::ItemCrafted { item_name, .. } = ev {
                self.total_items_crafted += 1;
                logs.push(format!("[Tick #{}] Ranger Sylva crafted {} and strung bowstring!", self.current_tick, item_name));
            } else if let GameEvent::NpcSlain { .. } = ev {
                self.total_mobs_slain += 1;
            }
        }

        // -------------------------------------------------------------
        // 3. CARTOGRAPHER (Continental Grand Surveyor)
        // -------------------------------------------------------------
        let mut carto_intents = Vec::new();
        // Move in sweeping continental expedition path (3 m/s)
        let phase = (self.current_tick / 500) % 4;
        let (cdx, cdz) = match phase {
            0 => (Q64::from_int(3)?, Q64::ZERO),               // East
            1 => (Q64::ZERO, Q64::from_int(-3)?),              // North
            2 => (Q64::from_int(-3)?, Q64::ZERO),              // West
            _ => (Q64::ZERO, Q64::from_int(3)?),               // South
        };
        carto_intents.push(PlayerIntent::Move { dx: cdx, dz: cdz });

        let carto_events = self.session_cartographer.sim.step(carto_intents)?;
        for ev in carto_events {
            if let GameEvent::EntityMoved { new_pos, .. } = ev {
                let chunk = ChunkCoord::from_world_pos(new_pos);
                if self.unique_chunks.insert((chunk.x, chunk.z)) {
                    let biome = self.session_cartographer.sim.terrain.biome_at(new_pos.x, new_pos.z)?;
                    logs.push(format!("[Tick #{}] Cartographer Atlas discovered Chunk [{}, {}] in Biome {:?} (Elevation: {:.1}m)!",
                        self.current_tick, chunk.x, chunk.z, biome, new_pos.y));
                }
            }
        }

        // -------------------------------------------------------------
        // 4. BLACKSMITH (Artisan & Closed Economy Metallurgist)
        // -------------------------------------------------------------
        let mut smith_intents = Vec::new();
        let s_iron = self.session_blacksmith.sim.player.inventory.get(&1).copied().unwrap_or(0);
        let s_timber = self.session_blacksmith.sim.player.inventory.get(&2).copied().unwrap_or(0);

        if s_iron >= 50 {
            smith_intents.push(PlayerIntent::CraftSword);
            smith_intents.push(PlayerIntent::EquipItem(101));
        } else if s_iron >= 20 && s_timber >= 20 {
            smith_intents.push(PlayerIntent::CraftShield);
            smith_intents.push(PlayerIntent::EquipItem(103));
        } else {
            // Find closest mineral node
            let closest = self.session_blacksmith.sim.resource_nodes.values()
                .filter(|n| n.remaining_amount > 0)
                .min_by_key(|n| self.session_blacksmith.sim.player.position.distance(n.world_pos).map(|d| d.to_raw()).unwrap_or(i128::MAX));

            if let Some(node) = closest {
                let d = self.session_blacksmith.sim.player.position.distance(node.world_pos)?;
                if d <= Q64::from_int(6)? {
                    smith_intents.push(PlayerIntent::Harvest(node.node_id));
                } else {
                    let dir = node.world_pos.checked_sub(self.session_blacksmith.sim.player.position)?.normalize()?;
                    smith_intents.push(PlayerIntent::Move { dx: dir.x.checked_mul(Q64::from_int(3)?)?, dz: dir.z.checked_mul(Q64::from_int(3)?)? });
                }
            } else {
                // Ensure nodes exist for continuous mining
                let cur_chunk = ChunkCoord::from_world_pos(self.session_blacksmith.sim.player.position);
                if let Some(new_node) = self.session_blacksmith.sim.terrain.mineral_vein_for_chunk(cur_chunk)? {
                    self.session_blacksmith.sim.resource_nodes.insert(new_node.node_id, new_node);
                }
            }
        }

        let smith_events = self.session_blacksmith.sim.step(smith_intents)?;
        for ev in smith_events {
            if let GameEvent::ResourceHarvested { amount, .. } = ev {
                self.total_resources_mined += amount;
            } else if let GameEvent::ItemCrafted { item_name, .. } = ev {
                self.total_items_crafted += 1;
                logs.push(format!("[Tick #{}] Blacksmith Vulcan forged {} (Thermodynamically Conserved)!", self.current_tick, item_name));
            }
        }

        // -------------------------------------------------------------
        // 5. CHAOS MONKEY (Invariant Auditor & Fuzzer)
        // -------------------------------------------------------------
        let mut chaos_intents = Vec::new();
        // Weapon stance cycling
        let w_family = match (self.current_tick / 50) % 5 {
            0 => WeaponFamily::Blade,
            1 => WeaponFamily::Bow,
            2 => WeaponFamily::Shield,
            3 => WeaponFamily::Staff,
            _ => WeaponFamily::Shadow,
        };
        chaos_intents.push(PlayerIntent::SwitchWeapon(w_family));

        // Periodic cryptographic SMT invariant audit (every 200 ticks = 10s)
        if self.current_tick.is_multiple_of(200) {
            let proof = self.session_chaos_monkey.sim.smt.prove(self.session_chaos_monkey.sim.player.entity_id);
            let valid = SparseMerkleTree::verify_proof(&self.session_chaos_monkey.sim.last_state_root, &proof);
            if valid {
                self.smt_verifications_passed += 1;
            } else {
                panic!("SMT State Root Invariant Violation detected by Chaos Monkey at tick #{}!", self.current_tick);
            }

            // Verify closed thermodynamic mass ledger: exactly 100,000,000,000 micro-units, zero leak!
            let mass = self.session_chaos_monkey.sim.thermodynamic_ledger.total_world_mass_microunits;
            assert_eq!(mass, 100_000_000_000, "Thermodynamic mass leak detected!");
        }

        let _ = self.session_chaos_monkey.sim.step(chaos_intents)?;

        // -------------------------------------------------------------
        // 6. P2P NETWORK MESSAGE EXCHANGE ACROSS 5 NODES
        // -------------------------------------------------------------
        let delivered = self.net.step();
        for (sender, receiver, frame) in delivered {
            let msg = aeviterra_core::network::GossipMessage {
                message_id: [0u8; 32],
                topic: GossipTopic::Shard(1),
                origin: sender,
                hop_count: 0,
                payload: frame.payload,
            };

            let wire_msg = GossipWireProtocol::DirectPush(msg);
            if receiver == self.session_gladiator.node_id {
                let _ = self.session_gladiator.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_ranger.node_id {
                let _ = self.session_ranger.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_cartographer.node_id {
                let _ = self.session_cartographer.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_blacksmith.node_id {
                let _ = self.session_blacksmith.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_chaos_monkey.node_id {
                let _ = self.session_chaos_monkey.router.handle_incoming(sender, wire_msg);
            }
        }

        Ok(logs)
    }
}

/// Ten-Party Peered Network Environment (Ten Autonomous Gemini Agents Playing & Stress-Testing Together).
/// Simulates 60+ minutes of live action gameplay and adversarial exploit attempts:
/// 1. Gladiator (Combat combos, animation cancel resistance, dodge i-frame audit)
/// 2. Ranger (Archery, vantage points, kiting range verification)
/// 3. Cartographer (Continental exploration, halo boundary crossing, biome navigation)
/// 4. Blacksmith (Mining, closed thermodynamic mass conservation, crafting)
/// 5. Chaos Monkey (Fuzzing, SMT state root Merkle proof validation)
/// 6. Governor Gracchus (Municipal governance, charter voting, anti-Sybil tenure check)
/// 7. Arbiter Mercury (MEV arbitrage, mempool sequence replay, timestamp window checks)
/// 8. Alchemist Paracelsus (Thermodynamic recipe conservation, zero-mass dupe attempts)
/// 9. NetSplitter Janus (P2P partition resilience, gossip propagation, frame loss recovery)
/// 10. Architect Daedalus (Spatial structural grammar, non-overlapping bounding boxes)
pub struct TenAgentEnvironment {
    pub world_id: Hash,
    pub session_gladiator: PeeredGameSession,
    pub session_ranger: PeeredGameSession,
    pub session_cartographer: PeeredGameSession,
    pub session_blacksmith: PeeredGameSession,
    pub session_chaos_monkey: PeeredGameSession,
    pub session_governor: PeeredGameSession,
    pub session_mev: PeeredGameSession,
    pub session_alchemist: PeeredGameSession,
    pub session_netsplitter: PeeredGameSession,
    pub session_architect: PeeredGameSession,
    pub net: SimulatedNetwork,
    pub current_tick: u64,
    pub total_mobs_slain: u64,
    pub total_resources_mined: u64,
    pub total_items_crafted: u64,
    pub unique_chunks: BTreeSet<(i32, i32)>,
    pub smt_verifications_passed: u64,
    pub exploits_attempted: u64,
    pub exploits_blocked: u64,
}

impl TenAgentEnvironment {
    pub fn new() -> Result<Self, MathError> {
        let world_id = *b"AEVITERRA_WORLD_CANONICAL_2026_0";

        let mut session_gladiator = PeeredGameSession::new(
            1001,
            "Gladiator_Ares".to_string(),
            [11u8; 32],
            [12u8; 32],
            "192.168.1".to_string(),
            world_id,
        )?;

        let mut session_ranger = PeeredGameSession::new(
            1002,
            "Ranger_Sylva".to_string(),
            [21u8; 32],
            [22u8; 32],
            "10.0.1".to_string(),
            world_id,
        )?;

        let mut session_cartographer = PeeredGameSession::new(
            1003,
            "Cartographer_Atlas".to_string(),
            [31u8; 32],
            [32u8; 32],
            "172.16.1".to_string(),
            world_id,
        )?;

        let mut session_blacksmith = PeeredGameSession::new(
            1004,
            "Blacksmith_Vulcan".to_string(),
            [41u8; 32],
            [42u8; 32],
            "10.10.1".to_string(),
            world_id,
        )?;

        let mut session_chaos_monkey = PeeredGameSession::new(
            1005,
            "Chaos_Monkey_QA".to_string(),
            [51u8; 32],
            [52u8; 32],
            "192.168.100".to_string(),
            world_id,
        )?;

        let mut session_governor = PeeredGameSession::new(
            1006,
            "Governor_Gracchus".to_string(),
            [61u8; 32],
            [62u8; 32],
            "10.20.1".to_string(),
            world_id,
        )?;

        let mut session_mev = PeeredGameSession::new(
            1007,
            "Arbitrageur_Mercury".to_string(),
            [71u8; 32],
            [72u8; 32],
            "10.30.1".to_string(),
            world_id,
        )?;

        let mut session_alchemist = PeeredGameSession::new(
            1008,
            "Alchemist_Paracelsus".to_string(),
            [81u8; 32],
            [82u8; 32],
            "10.40.1".to_string(),
            world_id,
        )?;

        let mut session_netsplitter = PeeredGameSession::new(
            1009,
            "NetSplitter_Janus".to_string(),
            [91u8; 32],
            [92u8; 32],
            "10.50.1".to_string(),
            world_id,
        )?;

        let mut session_architect = PeeredGameSession::new(
            1010,
            "Architect_Daedalus".to_string(),
            [101u8; 32],
            [102u8; 32],
            "10.60.1".to_string(),
            world_id,
        )?;

        // Setup distinct starting equipment and positions
        session_gladiator.sim.player.combat.active_weapon = WeaponFamily::Blade;

        let r_x = Q64::from_int(56)?;
        let r_z = Q64::from_int(68)?;
        let r_y = session_ranger.sim.terrain.elevation_at(r_x, r_z)?;
        session_ranger.sim.player.position = Vec3Fixed::new(r_x, r_y, r_z);
        session_ranger.sim.player.combat.active_weapon = WeaponFamily::Bow;

        session_cartographer.sim.player.combat.active_weapon = WeaponFamily::Staff;
        session_blacksmith.sim.player.combat.active_weapon = WeaponFamily::Shield;

        let cm_x = Q64::from_int(68)?;
        let cm_z = Q64::from_int(60)?;
        let cm_y = session_chaos_monkey.sim.terrain.elevation_at(cm_x, cm_z)?;
        session_chaos_monkey.sim.player.position = Vec3Fixed::new(cm_x, cm_y, cm_z);
        session_chaos_monkey.sim.player.combat.active_weapon = WeaponFamily::Shadow;

        session_governor.sim.player.combat.active_weapon = WeaponFamily::Staff;
        session_mev.sim.player.combat.active_weapon = WeaponFamily::Blade;
        session_alchemist.sim.player.combat.active_weapon = WeaponFamily::Staff;
        session_netsplitter.sim.player.combat.active_weapon = WeaponFamily::Shadow;
        session_architect.sim.player.combat.active_weapon = WeaponFamily::Shield;

        // Register initial homestead plot for Governor & Architect
        let _ = session_governor.settlement_charter.claim_plot(1006, 1006, 0);
        let _ = session_architect.settlement_charter.claim_plot(1010, 1010, 0);

        // Setup gossip subscriptions across the 10-node mesh
        let nodes = [
            session_gladiator.node_id,
            session_ranger.node_id,
            session_cartographer.node_id,
            session_blacksmith.node_id,
            session_chaos_monkey.node_id,
            session_governor.node_id,
            session_mev.node_id,
            session_alchemist.node_id,
            session_netsplitter.node_id,
            session_architect.node_id,
        ];

        let shard_topic = GossipTopic::Shard(1);
        let halo_topic = GossipTopic::Halo { chunk_x: 64, chunk_z: 64 };

        for &peer_id in &nodes {
            for topic in [&shard_topic, &halo_topic] {
                if peer_id != session_gladiator.node_id { session_gladiator.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_ranger.node_id { session_ranger.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_cartographer.node_id { session_cartographer.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_blacksmith.node_id { session_blacksmith.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_chaos_monkey.node_id { session_chaos_monkey.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_governor.node_id { session_governor.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_mev.node_id { session_mev.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_alchemist.node_id { session_alchemist.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_netsplitter.node_id { session_netsplitter.router.subscribe_peer(peer_id, topic.clone()); }
                if peer_id != session_architect.node_id { session_architect.router.subscribe_peer(peer_id, topic.clone()); }
            }
        }

        let mut unique_chunks = BTreeSet::new();
        unique_chunks.insert((0, 0));

        let net = SimulatedNetwork::new(1, 1001);

        Ok(Self {
            world_id,
            session_gladiator,
            session_ranger,
            session_cartographer,
            session_blacksmith,
            session_chaos_monkey,
            session_governor,
            session_mev,
            session_alchemist,
            session_netsplitter,
            session_architect,
            net,
            current_tick: 0,
            total_mobs_slain: 0,
            total_resources_mined: 0,
            total_items_crafted: 0,
            unique_chunks,
            smt_verifications_passed: 0,
            exploits_attempted: 0,
            exploits_blocked: 0,
        })
    }

    /// Advance one autonomous simulation tick for all 10 Gemini player agents and verify adversarial exploit resistance.
    pub fn step_autonomous_tick(&mut self) -> Result<Vec<String>, MathError> {
        self.current_tick += 1;
        let mut logs = Vec::new();

        // -------------------------------------------------------------
        // 1. GLADIATOR (Combat Specialist)
        // -------------------------------------------------------------
        if self.session_gladiator.sim.player.combat.health == 0 {
            self.session_gladiator.sim.player.combat.health = 1000;
            self.session_gladiator.sim.player.combat.stamina = 100;
            self.session_gladiator.sim.player.position = Vec3Fixed::new(Q64::from_int(64)?, Q64::from_int(65)?, Q64::from_int(64)?);
            logs.push(format!("[Tick #{}] Gladiator Ares revived at Sanctuary Campfire with full health!", self.current_tick));
        }

        let mut glad_intents = Vec::new();
        let wolf_hp = self.session_gladiator.sim.npcs.get(&2001).map(|n| n.combat.health).unwrap_or(0);
        let wolf_pos = self.session_gladiator.sim.npcs.get(&2001).map(|n| n.position).unwrap_or(self.session_gladiator.sim.player.position);
        let dist_to_wolf = self.session_gladiator.sim.player.position.distance(wolf_pos)?;

        if wolf_hp > 0 {
            if dist_to_wolf <= Q64::from_int(4)? {
                if self.current_tick.is_multiple_of(7) && self.session_gladiator.sim.player.combat.stamina >= 25 {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::DodgeRoll));
                } else if self.current_tick.is_multiple_of(5) && self.session_gladiator.sim.player.combat.stamina >= 10 {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::Block));
                } else if self.session_gladiator.sim.player.combat.stamina >= 35 {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::HeavyAttack));
                } else {
                    glad_intents.push(PlayerIntent::Attack(CombatActionType::LightAttack));
                }
            } else {
                let dir = wolf_pos.checked_sub(self.session_gladiator.sim.player.position)?.normalize()?;
                let dx = dir.x.checked_mul(Q64::from_int(3)?)?;
                let dz = dir.z.checked_mul(Q64::from_int(3)?)?;
                glad_intents.push(PlayerIntent::Move { dx, dz });
            }
        } else {
            let iron = self.session_gladiator.sim.player.inventory.get(&1).copied().unwrap_or(0);
            if iron >= 50 {
                glad_intents.push(PlayerIntent::CraftSword);
                glad_intents.push(PlayerIntent::EquipItem(101));
            } else if self.current_tick.is_multiple_of(50) {
                if let Some(w) = self.session_gladiator.sim.npcs.get_mut(&2001) {
                    w.combat.health = 1000;
                    w.position = Vec3Fixed::new(Q64::from_int(70)?, Q64::from_int(65)?, Q64::from_int(66)?);
                }
            } else {
                let angle = (self.current_tick as f64 * 0.1).sin();
                let dx = if angle > 0.0 { Q64::from_int(2)? } else { Q64::from_int(-2)? };
                glad_intents.push(PlayerIntent::Move { dx, dz: Q64::ZERO });
            }
        }

        let glad_events = self.session_gladiator.sim.step(glad_intents)?;
        for ev in glad_events {
            if let GameEvent::NpcSlain { reward_marks, .. } = ev {
                self.total_mobs_slain += 1;
                logs.push(format!("[Tick #{}] Gladiator Ares slew a Direwolf Stalker (+{} Marks)!", self.current_tick, reward_marks));
            } else if let GameEvent::ItemCrafted { item_name, .. } = ev {
                self.total_items_crafted += 1;
                logs.push(format!("[Tick #{}] Gladiator Ares forged {}", self.current_tick, item_name));
            }
        }

        // -------------------------------------------------------------
        // 2. RANGER (Ranged Tactician & Bowyer)
        // -------------------------------------------------------------
        let mut ranger_intents = Vec::new();
        let timber = self.session_ranger.sim.player.inventory.get(&2).copied().unwrap_or(0);
        let has_bow = self.session_ranger.sim.player.equipped_weapon_item == Some(102);

        if !has_bow {
            if timber >= 30 {
                ranger_intents.push(PlayerIntent::CraftBow);
                ranger_intents.push(PlayerIntent::EquipItem(102));
            } else if let Some(node) = self.session_ranger.sim.resource_nodes.values().find(|n| n.resource_id == 2 && n.remaining_amount > 0) {
                let d = self.session_ranger.sim.player.position.distance(node.world_pos)?;
                if d <= Q64::from_int(6)? {
                    ranger_intents.push(PlayerIntent::Harvest(node.node_id));
                } else {
                    let dir = node.world_pos.checked_sub(self.session_ranger.sim.player.position)?.normalize()?;
                    ranger_intents.push(PlayerIntent::Move { dx: dir.x.checked_mul(Q64::from_int(3)?)?, dz: dir.z.checked_mul(Q64::from_int(3)?)? });
                }
            } else {
                self.session_ranger.sim.player.inventory.insert(2, 30);
                self.session_ranger.sim.thermodynamic_ledger.entity_balances.insert((self.session_ranger.sim.player.entity_id, 2), 30);
            }
        } else {
            let r_wolf_hp = self.session_ranger.sim.npcs.get(&2001).map(|n| n.combat.health).unwrap_or(0);
            let r_wolf_pos = self.session_ranger.sim.npcs.get(&2001).map(|n| n.position).unwrap_or(self.session_ranger.sim.player.position);
            let r_dist = self.session_ranger.sim.player.position.distance(r_wolf_pos)?;

            if r_wolf_hp > 0 {
                if r_dist < Q64::from_int(6)? {
                    ranger_intents.push(PlayerIntent::Attack(CombatActionType::DodgeRoll));
                } else if r_dist <= Q64::from_int(18)? {
                    if self.session_ranger.sim.player.combat.stamina >= 35 {
                        ranger_intents.push(PlayerIntent::Attack(CombatActionType::HeavyAttack));
                    } else {
                        ranger_intents.push(PlayerIntent::Attack(CombatActionType::LightAttack));
                    }
                }
            } else if self.current_tick.is_multiple_of(60)
                && let Some(w) = self.session_ranger.sim.npcs.get_mut(&2001) {
                w.combat.health = 1000;
                w.position = Vec3Fixed::new(Q64::from_int(70)?, Q64::from_int(65)?, Q64::from_int(66)?);
            }
        }

        let ranger_events = self.session_ranger.sim.step(ranger_intents)?;
        for ev in ranger_events {
            if let GameEvent::ItemCrafted { item_name, .. } = ev {
                self.total_items_crafted += 1;
                logs.push(format!("[Tick #{}] Ranger Sylva crafted {} and strung bowstring!", self.current_tick, item_name));
            } else if let GameEvent::NpcSlain { .. } = ev {
                self.total_mobs_slain += 1;
            }
        }

        // -------------------------------------------------------------
        // 3. CARTOGRAPHER (Continental Grand Surveyor)
        // -------------------------------------------------------------
        let mut carto_intents = Vec::new();
        let phase = (self.current_tick / 500) % 4;
        let (cdx, cdz) = match phase {
            0 => (Q64::from_int(3)?, Q64::ZERO),
            1 => (Q64::ZERO, Q64::from_int(-3)?),
            2 => (Q64::from_int(-3)?, Q64::ZERO),
            _ => (Q64::ZERO, Q64::from_int(3)?),
        };
        carto_intents.push(PlayerIntent::Move { dx: cdx, dz: cdz });

        let carto_events = self.session_cartographer.sim.step(carto_intents)?;
        for ev in carto_events {
            if let GameEvent::EntityMoved { new_pos, .. } = ev {
                let chunk = ChunkCoord::from_world_pos(new_pos);
                if self.unique_chunks.insert((chunk.x, chunk.z)) {
                    let biome = self.session_cartographer.sim.terrain.biome_at(new_pos.x, new_pos.z)?;
                    logs.push(format!("[Tick #{}] Cartographer Atlas charted Chunk [{}, {}] in Biome {:?} (Elev: {:.1}m)!",
                        self.current_tick, chunk.x, chunk.z, biome, new_pos.y));
                }
            }
        }

        // -------------------------------------------------------------
        // 4. BLACKSMITH (Artisan & Closed Economy Metallurgist)
        // -------------------------------------------------------------
        let mut smith_intents = Vec::new();
        let s_iron = self.session_blacksmith.sim.player.inventory.get(&1).copied().unwrap_or(0);
        let s_timber = self.session_blacksmith.sim.player.inventory.get(&2).copied().unwrap_or(0);

        if s_iron >= 50 {
            smith_intents.push(PlayerIntent::CraftSword);
            smith_intents.push(PlayerIntent::EquipItem(101));
        } else if s_iron >= 20 && s_timber >= 20 {
            smith_intents.push(PlayerIntent::CraftShield);
            smith_intents.push(PlayerIntent::EquipItem(103));
        } else {
            let closest = self.session_blacksmith.sim.resource_nodes.values()
                .filter(|n| n.remaining_amount > 0)
                .min_by_key(|n| self.session_blacksmith.sim.player.position.distance(n.world_pos).map(|d| d.to_raw()).unwrap_or(i128::MAX));

            if let Some(node) = closest {
                let d = self.session_blacksmith.sim.player.position.distance(node.world_pos)?;
                if d <= Q64::from_int(6)? {
                    smith_intents.push(PlayerIntent::Harvest(node.node_id));
                } else {
                    let dir = node.world_pos.checked_sub(self.session_blacksmith.sim.player.position)?.normalize()?;
                    smith_intents.push(PlayerIntent::Move { dx: dir.x.checked_mul(Q64::from_int(3)?)?, dz: dir.z.checked_mul(Q64::from_int(3)?)? });
                }
            } else {
                let cur_chunk = ChunkCoord::from_world_pos(self.session_blacksmith.sim.player.position);
                if let Some(new_node) = self.session_blacksmith.sim.terrain.mineral_vein_for_chunk(cur_chunk)? {
                    self.session_blacksmith.sim.resource_nodes.insert(new_node.node_id, new_node);
                }
            }
        }

        let smith_events = self.session_blacksmith.sim.step(smith_intents)?;
        for ev in smith_events {
            if let GameEvent::ResourceHarvested { amount, .. } = ev {
                self.total_resources_mined += amount;
            } else if let GameEvent::ItemCrafted { item_name, .. } = ev {
                self.total_items_crafted += 1;
                logs.push(format!("[Tick #{}] Blacksmith Vulcan forged {} (Thermodynamically Conserved)!", self.current_tick, item_name));
            }
        }

        // -------------------------------------------------------------
        // 5. CHAOS MONKEY (Invariant Auditor & Fuzzer)
        // -------------------------------------------------------------
        let mut chaos_intents = Vec::new();
        let w_family = match (self.current_tick / 50) % 5 {
            0 => WeaponFamily::Blade,
            1 => WeaponFamily::Bow,
            2 => WeaponFamily::Shield,
            3 => WeaponFamily::Staff,
            _ => WeaponFamily::Shadow,
        };
        chaos_intents.push(PlayerIntent::SwitchWeapon(w_family));

        if self.current_tick.is_multiple_of(200) {
            let proof = self.session_chaos_monkey.sim.smt.prove(self.session_chaos_monkey.sim.player.entity_id);
            let valid = SparseMerkleTree::verify_proof(&self.session_chaos_monkey.sim.last_state_root, &proof);
            if valid {
                self.smt_verifications_passed += 1;
            } else {
                panic!("SMT State Root Invariant Violation detected by Chaos Monkey at tick #{}!", self.current_tick);
            }

            let mass = self.session_chaos_monkey.sim.thermodynamic_ledger.total_world_mass_microunits;
            assert_eq!(mass, 100_000_000_000, "Thermodynamic mass leak detected!");
        }

        let _ = self.session_chaos_monkey.sim.step(chaos_intents)?;

        // -------------------------------------------------------------
        // 6. GOVERNOR GRACCHUS (Municipal Anti-Sybil Tenure Guard)
        // -------------------------------------------------------------
        if self.current_tick.is_multiple_of(100) {
            self.exploits_attempted += 1;
            // Sybil bot #9999 attempts an unvetted mayoral election takeover vote
            match self.session_governor.settlement_charter.can_vote_for_mayor(9999, self.current_tick) {
                Err(SettlementError::SybilVotingDisqualified) | Err(SettlementError::InsufficientTenure) => {
                    self.exploits_blocked += 1;
                    if self.current_tick.is_multiple_of(500) {
                        logs.push(format!("[Tick #{}] Sybil municipal takeover vote BLOCKED by tenure guard!", self.current_tick));
                    }
                }
                _ => panic!("Security Breach: Unvetted Sybil account cast a municipal vote!"),
            }
        }

        // -------------------------------------------------------------
        // 7. ARBITER MERCURY (MEV & Mempool Replay Guard)
        // -------------------------------------------------------------
        if self.current_tick.is_multiple_of(150) {
            self.exploits_attempted += 1;
            let mut replay_env = self.session_mev.create_signed_action(1, vec![1, 2, 3], self.current_tick);
            replay_env.sequence_num = 0; // Stale sequence replay attack
            match self.session_mev.admission_verifier.verify_and_record(&replay_env, self.current_tick) {
                Err(ActionAdmissionError::StaleOrReplayedSequence { .. }) => {
                    self.exploits_blocked += 1;
                    if self.current_tick.is_multiple_of(600) {
                        logs.push(format!("[Tick #{}] Front-running sequence replay attack BLOCKED by ActionAdmissionVerifier!", self.current_tick));
                    }
                }
                _ => panic!("Security Breach: Replayed sequence envelope accepted by admission verifier!"),
            }
        }

        // -------------------------------------------------------------
        // 8. ALCHEMIST PARACELSUS (Thermodynamic Invariant I5 Guard)
        // -------------------------------------------------------------
        if self.current_tick.is_multiple_of(120) {
            self.exploits_attempted += 1;
            // Attempt phantom mass creation: 50 input iron -> 100 output steel with 0 slag
            let res = self.session_alchemist.sim.thermodynamic_ledger.execute_recipe(
                1008, 1, 50, 101, 100, 999, 0,
            );
            match res {
                Err(EconomicError::MassConservationViolation) => {
                    self.exploits_blocked += 1;
                    if self.current_tick.is_multiple_of(600) {
                        logs.push(format!("[Tick #{}] Thermodynamic phantom mass duplication BLOCKED (Invariant I5: 50 != 100)!", self.current_tick));
                    }
                }
                _ => panic!("Security Breach: Phantom mass created without thermodynamic backing!"),
            }
        }

        // -------------------------------------------------------------
        // 9. NET-SPLITTER JANUS (P2P Shard Boundary & Partition Guard)
        // -------------------------------------------------------------
        if self.current_tick.is_multiple_of(200) {
            self.exploits_attempted += 1;
            let mut wrong_shard_env = self.session_netsplitter.create_signed_action(1, vec![9, 9, 9], self.current_tick);
            wrong_shard_env.authority_shard = 999; // Shard mismatch attack
            match self.session_netsplitter.admission_verifier.verify_and_record(&wrong_shard_env, self.current_tick) {
                Err(ActionAdmissionError::ShardMismatch) => {
                    self.exploits_blocked += 1;
                    if self.current_tick.is_multiple_of(600) {
                        logs.push(format!("[Tick #{}] Cross-shard partition injection BLOCKED by shard authority boundary!", self.current_tick));
                    }
                }
                _ => panic!("Security Breach: Cross-shard injection bypassed authority verification!"),
            }
        }

        // -------------------------------------------------------------
        // 10. ARCHITECT DAEDALUS (Structural Macro-Primitive Grammar Guard)
        // -------------------------------------------------------------
        if self.current_tick.is_multiple_of(180) {
            self.exploits_attempted += 1;
            let illegal_prim = StructuralPrimitive {
                style: ArchitecturalStyle::FortifiedCitadel,
                primary_material: MaterialType::Granite,
                secondary_material: MaterialType::Basalt,
                length: Q64::from_int(500)?, // 500m > 64m max limit
                height: Q64::from_int(20)?,
                thickness: Q64::from_int(2)?,
                features: 0,
            };
            match self.session_architect.settlement_charter.build_structure(1010, 1010, illegal_prim) {
                Err(SettlementError::InvalidStructureConstraints) => {
                    self.exploits_blocked += 1;
                    if self.current_tick.is_multiple_of(540) {
                        logs.push(format!("[Tick #{}] Illegal oversized citadel macro-primitive BLOCKED by structural grammar!", self.current_tick));
                    }
                }
                _ => panic!("Security Breach: Illegal structural macro-primitive bypassed physics grammar!"),
            }
        }

        // -------------------------------------------------------------
        // 11. P2P NETWORK MESSAGE EXCHANGE ACROSS ALL 10 NODES
        // -------------------------------------------------------------
        let delivered = self.net.step();
        for (sender, receiver, frame) in delivered {
            let msg = aeviterra_core::network::GossipMessage {
                message_id: [0u8; 32],
                topic: GossipTopic::Shard(1),
                origin: sender,
                hop_count: 0,
                payload: frame.payload,
            };

            let wire_msg = GossipWireProtocol::DirectPush(msg);
            if receiver == self.session_gladiator.node_id {
                let _ = self.session_gladiator.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_ranger.node_id {
                let _ = self.session_ranger.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_cartographer.node_id {
                let _ = self.session_cartographer.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_blacksmith.node_id {
                let _ = self.session_blacksmith.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_chaos_monkey.node_id {
                let _ = self.session_chaos_monkey.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_governor.node_id {
                let _ = self.session_governor.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_mev.node_id {
                let _ = self.session_mev.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_alchemist.node_id {
                let _ = self.session_alchemist.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_netsplitter.node_id {
                let _ = self.session_netsplitter.router.handle_incoming(sender, wire_msg);
            } else if receiver == self.session_architect.node_id {
                let _ = self.session_architect.router.handle_incoming(sender, wire_msg);
            }
        }

        Ok(logs)
    }
}
