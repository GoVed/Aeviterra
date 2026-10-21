use aeviterra_core::math::fixed::{Q64, Vec3Fixed, MathError};
use aeviterra_core::game::combat::WeaponFamily;
use aeviterra_core::game::settlement::SettlementCharter;
use aeviterra_core::game::quest::{QuestEngine, QuestTemplate};
use aeviterra_core::consensus::envelope::{
    CanonicalActionEnvelope, ActionAdmissionVerifier, Signature, Hash,
};
use aeviterra_core::network::{
    StreamingFrameCodec, MultiQueueScheduler, GossipRouter, GossipTopic, GossipWireProtocol,
    PeerSession, PeerReputationLedger, SimulatedNetwork, WireFrame, PriorityQueue,
    MAGIC_HEADER, CURRENT_WIRE_VERSION, derive_node_id, NodeId, NodeRole,
};
use crate::simulation::WorldSimulation;

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
