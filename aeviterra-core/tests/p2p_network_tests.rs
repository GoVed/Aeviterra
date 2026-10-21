use aeviterra_core::network::*;

#[test]
fn test_streaming_codec_fragmentation_and_checksum() {
    let mut codec = StreamingFrameCodec::new();

    let original_frame = WireFrame {
        magic: *MAGIC_HEADER,
        version: CURRENT_WIRE_VERSION,
        queue: PriorityQueue::Gameplay,
        payload: b"deterministic_action_envelope_payload_bytes".to_vec(),
    };

    let encoded = StreamingFrameCodec::encode_frame(&original_frame).expect("encode frame");
    assert!(encoded.len() > 11);

    // Feed in 3 fragmented pieces (e.g. realistic TCP chunking)
    let chunk1 = &encoded[0..5];
    let chunk2 = &encoded[5..15];
    let chunk3 = &encoded[15..];

    codec.feed(chunk1).expect("feed chunk1");
    assert_eq!(codec.pop_frame().expect("pop"), None);

    codec.feed(chunk2).expect("feed chunk2");
    assert_eq!(codec.pop_frame().expect("pop"), None);

    codec.feed(chunk3).expect("feed chunk3");
    let popped = codec.pop_frame().expect("pop").expect("frame available");
    assert_eq!(popped.magic, original_frame.magic);
    assert_eq!(popped.version, original_frame.version);
    assert_eq!(popped.queue, original_frame.queue);
    assert_eq!(popped.payload, original_frame.payload);

    // Verify corruption detection
    let mut corrupted = encoded.clone();
    let last_idx = corrupted.len() - 1;
    corrupted[last_idx] ^= 0xFF; // Invert checksum bit

    let mut bad_codec = StreamingFrameCodec::new();
    bad_codec.feed(&corrupted).expect("feed corrupted");
    let err = bad_codec.pop_frame().expect_err("must detect checksum mismatch");
    assert_eq!(err, FramingError::CommitmentMismatch);
}

#[test]
fn test_peer_handshake_mutual_auth_and_replay() {
    let world_id = [42u8; 32];
    let now = 1_700_000_000u64;

    let alice_pub = [1u8; 32];
    let alice_sec = [2u8; 32];
    let bob_pub = [3u8; 32];
    let bob_sec = [4u8; 32];

    let mut alice = PeerSession::new(alice_pub, alice_sec, world_id);
    let mut bob = PeerSession::new(bob_pub, bob_sec, world_id);

    // Alice -> Bob: HandshakeInit
    let client_nonce = [11u8; 32];
    let init = alice.create_handshake_init(
        client_nonce,
        now,
        NodeRole::VALIDATOR_FEDERATION,
        vec![1, 2],
    );

    // Bob processes init -> sends HandshakeResp
    let server_nonce = [22u8; 32];
    let resp = bob.handle_handshake_init(&init, server_nonce, now).expect("bob accepts init");

    // Alice processes resp -> sends HandshakeAck
    let ack = alice.handle_handshake_resp(&resp, now).expect("alice accepts resp");
    assert!(alice.is_authenticated());

    // Bob processes ack -> authenticated!
    bob.handle_handshake_ack(&ack).expect("bob accepts ack");
    assert!(bob.is_authenticated());

    assert_eq!(alice.remote_node_id.unwrap(), bob.local_node_id);
    assert_eq!(bob.remote_node_id.unwrap(), alice.local_node_id);

    // Test rejection: self-connection
    let mut self_alice = PeerSession::new(alice_pub, alice_sec, world_id);
    let self_init = self_alice.create_handshake_init(client_nonce, now, 0, vec![]);
    let err = self_alice.handle_handshake_init(&self_init, server_nonce, now).expect_err("self connect");
    assert_eq!(err, HandshakeError::SelfConnection);

    // Test rejection: timestamp drift > 30s
    let mut stale_alice = PeerSession::new(alice_pub, alice_sec, world_id);
    let stale_init = stale_alice.create_handshake_init(client_nonce, now - 35, 0, vec![]);
    let mut fresh_bob = PeerSession::new(bob_pub, bob_sec, world_id);
    let err = fresh_bob.handle_handshake_init(&stale_init, server_nonce, now).expect_err("stale timestamp");
    assert_eq!(err, HandshakeError::TimestampDriftExceeded(35));
}

#[test]
fn test_address_book_subnet_diversity_and_eclipse_defense() {
    let mut book = AddressBook::new();
    let now = 1_000;

    let subnets = ["192.168", "10.0", "172.16", "24.50", "45.33", "88.99"];

    // Add peers across 6 distinct subnets
    for (i, &subnet) in subnets.iter().enumerate() {
        let node_id = derive_node_id(&[(i + 1) as u8; 32]);
        book.register_peer(
            node_id,
            subnet.to_string(),
            format!("{}.0.{}:9000", subnet, i),
            i < 3, // first 3 are validators
            now,
        ).expect("register peer");
    }

    assert!(book.satisfies_eclipse_diversity());
    assert_eq!(book.validator_peer_count(), 3);

    // Attempt to register 4 more peers into "192.168" (total 5)
    for i in 10..13 {
        let node_id = derive_node_id(&[i as u8; 32]);
        book.register_peer(
            node_id,
            "192.168".to_string(),
            format!("192.168.1.{}:9000", i),
            false,
            now,
        ).expect("register peer up to limit");
    }

    // 5th peer on "192.168" must be rejected
    let excess_id = derive_node_id(&[99u8; 32]);
    let err = book.register_peer(
        excess_id,
        "192.168".to_string(),
        "192.168.1.99:9000".to_string(),
        false,
        now,
    ).expect_err("exceed subnet limit");
    assert_eq!(err, HandshakeError::SubnetLimitExceeded);
}

#[test]
fn test_multi_queue_scheduler_anti_starvation_and_preemption() {
    let mut scheduler = MultiQueueScheduler::new();

    // 1. Enqueue 20 gameplay frames and 5 recovery frames at tick 10
    for i in 0..20 {
        scheduler.enqueue(WireFrame {
            magic: *MAGIC_HEADER,
            version: CURRENT_WIRE_VERSION,
            queue: PriorityQueue::Gameplay,
            payload: vec![i as u8; 500],
        }, 10);
    }

    for i in 0..5 {
        scheduler.enqueue(WireFrame {
            magic: *MAGIC_HEADER,
            version: CURRENT_WIRE_VERSION,
            queue: PriorityQueue::Recovery,
            payload: vec![i as u8; 200],
        }, 10);
    }

    // 2. Enqueue an urgent Consensus frame with timestamp 5 (delayed by 6 ticks at current tick 11)
    scheduler.enqueue(WireFrame {
        magic: *MAGIC_HEADER,
        version: CURRENT_WIRE_VERSION,
        queue: PriorityQueue::Consensus,
        payload: b"urgent_bft_quorum_vote".to_vec(),
    }, 5);

    // 3. Dispatch at tick 11: Consensus frame MUST preempt immediately
    let first = scheduler.dispatch_next(11).expect("dispatch");
    assert_eq!(first.frame.queue, PriorityQueue::Consensus);
    assert_eq!(&first.frame.payload, b"urgent_bft_quorum_vote");

    // 4. Over several dispatch cycles, verify Recovery queue is NEVER starved
    let mut saw_recovery = false;
    let mut saw_gameplay = false;

    for _ in 0..25 {
        if let Some(qf) = scheduler.dispatch_next(12) {
            if qf.frame.queue == PriorityQueue::Recovery {
                saw_recovery = true;
            }
            if qf.frame.queue == PriorityQueue::Gameplay {
                saw_gameplay = true;
            }
        }
    }

    assert!(saw_recovery, "Recovery queue must not be starved");
    assert!(saw_gameplay, "Gameplay queue must be served");
}

#[test]
fn test_gossip_pubsub_deduplication_and_ihave_iwant() {
    let node_a = derive_node_id(&[1u8; 32]);
    let node_b = derive_node_id(&[2u8; 32]);
    let node_c = derive_node_id(&[3u8; 32]);

    let mut router = GossipRouter::new(node_a);
    let halo_topic = GossipTopic::Halo { chunk_x: 10, chunk_z: 20 };

    router.subscribe_peer(node_b, halo_topic.clone());
    router.subscribe_peer(node_c, halo_topic.clone());

    // 1. Publish small message <= 1KB -> direct push
    let small_payload = vec![0xAA; 256];
    let dispatches = router.publish_local(halo_topic.clone(), small_payload);
    assert_eq!(dispatches.len(), 2);
    for (_, msg) in dispatches {
        match msg {
            GossipWireProtocol::DirectPush(gm) => assert_eq!(gm.topic, halo_topic),
            _ => panic!("Expected DirectPush for small payload"),
        }
    }

    // 2. Publish large message > 1KB -> IHAVE announcement
    let large_payload = vec![0xBB; 2048];
    let dispatches_large = router.publish_local(halo_topic.clone(), large_payload.clone());
    assert_eq!(dispatches_large.len(), 2);
    let mut advertised_msg_id = [0u8; 32];
    for (_, msg) in dispatches_large {
        match msg {
            GossipWireProtocol::IHave { topic, message_id } => {
                assert_eq!(topic, halo_topic);
                advertised_msg_id = message_id;
            }
            _ => panic!("Expected IHave for large payload"),
        }
    }

    // 3. Node B requests with IWANT -> Node A responds with DirectPush
    let (delivered, replies) = router.handle_incoming(
        node_b,
        GossipWireProtocol::IWant { topic: halo_topic.clone(), message_id: advertised_msg_id },
    );
    assert_eq!(delivered, None);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0, node_b);
    match &replies[0].1 {
        GossipWireProtocol::DirectPush(msg) => {
            assert_eq!(msg.message_id, advertised_msg_id);
            assert_eq!(msg.payload, large_payload);
        }
        _ => panic!("Expected DirectPush reply to IWANT"),
    }

    // 4. Duplicate push handling
    let test_msg = GossipMessage {
        message_id: advertised_msg_id,
        topic: halo_topic.clone(),
        origin: node_c,
        hop_count: 1,
        payload: large_payload,
    };
    let (delivered_dup, forwards) = router.handle_incoming(
        node_c,
        GossipWireProtocol::DirectPush(test_msg),
    );
    assert_eq!(delivered_dup, None, "Duplicate message must be rejected");
    assert!(forwards.is_empty(), "Duplicate message must not be forwarded");
}

#[test]
fn test_peer_reputation_quarantine_and_ban() {
    let mut ledger = PeerReputationLedger::new();
    let node_id = derive_node_id(&[55u8; 32]);
    let subnet = "198.51.100".to_string();
    let now = 10_000u64;

    // Normal valid events -> FullPeer
    let tier = ledger.record_event(node_id, subnet.clone(), ReputationEvent::ValidBftVote, now);
    assert_eq!(tier, PeerTier::FullPeer);

    // Rate limit violations
    ledger.record_event(node_id, subnet.clone(), ReputationEvent::GameplayRateLimitExceeded, now);
    ledger.record_event(node_id, subnet.clone(), ReputationEvent::GameplayRateLimitExceeded, now);

    // Malformed frames (-200 each)
    ledger.record_event(node_id, subnet.clone(), ReputationEvent::MalformedWireFrame, now);
    ledger.record_event(node_id, subnet.clone(), ReputationEvent::MalformedWireFrame, now);
    let tier_throttled = ledger.record_event(node_id, subnet.clone(), ReputationEvent::MalformedWireFrame, now);
    assert_eq!(tier_throttled, PeerTier::Quarantined);

    // Equivocation (-10,000) -> Instant Permaban
    let tier_banned = ledger.record_event(node_id, subnet.clone(), ReputationEvent::EquivocationOrForgedProof, now);
    assert_eq!(tier_banned, PeerTier::Banned);

    // Subnet is also banned
    assert!(ledger.is_banned(&node_id, &subnet, now + 100));
}

#[test]
fn test_simulated_network_latency_partition_and_heal() {
    let mut net = SimulatedNetwork::new(2, 12345); // 2 ticks base latency

    let node_a = derive_node_id(&[1u8; 32]);
    let node_b = derive_node_id(&[2u8; 32]);
    let node_c = derive_node_id(&[3u8; 32]);

    let frame = WireFrame {
        magic: *MAGIC_HEADER,
        version: CURRENT_WIRE_VERSION,
        queue: PriorityQueue::Consensus,
        payload: b"bft_vote".to_vec(),
    };

    // A sends to B at tick 0
    net.send_packet(node_a, node_b, frame.clone());
    assert_eq!(net.in_flight_count(), 1);

    // Advance 1 tick -> not yet arrived (min latency 2)
    let delivered_tick1 = net.step();
    assert!(delivered_tick1.is_empty());

    // Advance more ticks -> arrives
    let mut delivered_all = Vec::new();
    for _ in 0..5 {
        delivered_all.extend(net.step());
    }
    assert_eq!(delivered_all.len(), 1);
    assert_eq!(delivered_all[0].0, node_a);
    assert_eq!(delivered_all[0].1, node_b);

    // Partition test: partition node_c from node_a and node_b
    net.add_partition(node_a, node_c);
    net.send_packet(node_a, node_c, frame.clone());
    // Packet is immediately dropped by partition
    assert_eq!(net.in_flight_count(), 0);

    // Heal partition
    net.heal_partition(node_a, node_c);
    net.send_packet(node_a, node_c, frame);
    assert_eq!(net.in_flight_count(), 1);
}
