use super::peer::NodeId;
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_GOSSIP_HOPS: u8 = 8;
pub const DIRECT_PUSH_MAX_PAYLOAD: usize = 1024; // 1 KB threshold for IHAVE/IWANT
pub const DEDUPLICATION_CACHE_CAPACITY: usize = 10_000;

pub type MessageId = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GossipTopic {
    Consensus(u32),
    Shard(u32),
    Halo { chunk_x: i64, chunk_z: i64 },
    TwoPhaseCommit(u64),
    GlobalBeacon,
}

impl GossipTopic {
    pub fn to_topic_string(&self) -> String {
        match self {
            GossipTopic::Consensus(shard) => format!("aeviterra/consensus/{}", shard),
            GossipTopic::Shard(shard) => format!("aeviterra/shard/{}", shard),
            GossipTopic::Halo { chunk_x, chunk_z } => format!("aeviterra/halo/{}_{}", chunk_x, chunk_z),
            GossipTopic::TwoPhaseCommit(tx_id) => format!("aeviterra/2pc/{}", tx_id),
            GossipTopic::GlobalBeacon => "aeviterra/global/beacon".to_string(),
        }
    }
}

pub fn compute_message_id(topic: &GossipTopic, payload: &[u8]) -> MessageId {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_GOSSIP_MSG_ID_V1");
    hasher.update(topic.to_topic_string().as_bytes());
    hasher.update(payload);
    hasher.finalize().into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GossipMessage {
    pub message_id: MessageId,
    pub topic: GossipTopic,
    pub origin: NodeId,
    pub hop_count: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GossipWireProtocol {
    DirectPush(GossipMessage),
    IHave { topic: GossipTopic, message_id: MessageId },
    IWant { topic: GossipTopic, message_id: MessageId },
}

/// Two-Tier Deduplication Cache.
/// Eliminates gossip amplification loops and broadcast storms.
#[derive(Debug, Clone)]
pub struct MessageDeduplicator {
    seen_set: BTreeSet<MessageId>,
    order_queue: VecDeque<MessageId>,
    capacity: usize,
}

impl Default for MessageDeduplicator {
    fn default() -> Self {
        Self::new(DEDUPLICATION_CACHE_CAPACITY)
    }
}

impl MessageDeduplicator {
    pub fn new(capacity: usize) -> Self {
        Self {
            seen_set: BTreeSet::new(),
            order_queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Check if a message has already been processed. If not, record it as seen.
    pub fn is_seen_or_record(&mut self, msg_id: &MessageId) -> bool {
        if self.seen_set.contains(msg_id) {
            return true;
        }

        if self.order_queue.len() >= self.capacity
            && let Some(oldest) = self.order_queue.pop_front() {
            self.seen_set.remove(&oldest);
        }

        self.seen_set.insert(*msg_id);
        self.order_queue.push_back(*msg_id);
        false
    }

    pub fn contains(&self, msg_id: &MessageId) -> bool {
        self.seen_set.contains(msg_id)
    }
}

/// Dynamic Topic-Based Gossip Router.
/// Routes shard, halo, and consensus messages across registered peer subscriptions with anti-echo.
#[derive(Debug, Clone)]
pub struct GossipRouter {
    pub local_node_id: NodeId,
    pub deduplicator: MessageDeduplicator,
    /// Topic -> Set of interested Peer NodeIds
    pub topic_subscriptions: BTreeMap<GossipTopic, BTreeSet<NodeId>>,
    /// Staged payload storage for IWANT queries (MessageId -> GossipMessage)
    pub payload_cache: BTreeMap<MessageId, GossipMessage>,
}

impl GossipRouter {
    pub fn new(local_node_id: NodeId) -> Self {
        Self {
            local_node_id,
            deduplicator: MessageDeduplicator::default(),
            topic_subscriptions: BTreeMap::new(),
            payload_cache: BTreeMap::new(),
        }
    }

    /// Register peer subscription to a topic.
    pub fn subscribe_peer(&mut self, peer_id: NodeId, topic: GossipTopic) {
        self.topic_subscriptions.entry(topic).or_default().insert(peer_id);
    }

    /// Unregister peer subscription.
    pub fn unsubscribe_peer(&mut self, peer_id: &NodeId, topic: &GossipTopic) {
        if let Some(peers) = self.topic_subscriptions.get_mut(topic) {
            peers.remove(peer_id);
            if peers.is_empty() {
                self.topic_subscriptions.remove(topic);
            }
        }
    }

    /// Remove peer from all subscriptions.
    pub fn remove_peer_all_topics(&mut self, peer_id: &NodeId) {
        for peers in self.topic_subscriptions.values_mut() {
            peers.remove(peer_id);
        }
        self.topic_subscriptions.retain(|_, peers| !peers.is_empty());
    }

    /// Publish a local message into the gossip network.
    /// Returns vector of (recipient_node_id, protocol_message).
    pub fn publish_local(
        &mut self,
        topic: GossipTopic,
        payload: Vec<u8>,
    ) -> Vec<(NodeId, GossipWireProtocol)> {
        let msg_id = compute_message_id(&topic, &payload);
        let msg = GossipMessage {
            message_id: msg_id,
            topic: topic.clone(),
            origin: self.local_node_id,
            hop_count: 0,
            payload,
        };

        self.deduplicator.is_seen_or_record(&msg_id);
        self.payload_cache.insert(msg_id, msg.clone());

        let mut dispatches = Vec::new();
        if let Some(subscribers) = self.topic_subscriptions.get(&topic) {
            for &peer in subscribers {
                if peer != self.local_node_id {
                    if msg.payload.len() <= DIRECT_PUSH_MAX_PAYLOAD {
                        dispatches.push((peer, GossipWireProtocol::DirectPush(msg.clone())));
                    } else {
                        dispatches.push((peer, GossipWireProtocol::IHave {
                            topic: topic.clone(),
                            message_id: msg_id,
                        }));
                    }
                }
            }
        }

        dispatches
    }

    /// Handle incoming gossip protocol message from a peer.
    /// Returns:
    /// - Optional message to deliver to the local application layer
    /// - Vector of forwarded messages to peer nodes
    pub fn handle_incoming(
        &mut self,
        sender: NodeId,
        protocol_msg: GossipWireProtocol,
    ) -> (Option<GossipMessage>, Vec<(NodeId, GossipWireProtocol)>) {
        match protocol_msg {
            GossipWireProtocol::DirectPush(msg) => {
                // 1. Hop limit check
                if msg.hop_count >= MAX_GOSSIP_HOPS {
                    return (None, Vec::new());
                }

                // 2. Deduplication check
                if self.deduplicator.is_seen_or_record(&msg.message_id) {
                    return (None, Vec::new());
                }

                self.payload_cache.insert(msg.message_id, msg.clone());

                // 3. Forward to other subscribers with anti-echo (do not send back to sender or origin)
                let mut forwards = Vec::new();
                if let Some(subscribers) = self.topic_subscriptions.get(&msg.topic) {
                    let mut forwarded_msg = msg.clone();
                    forwarded_msg.hop_count += 1;

                    for &peer in subscribers {
                        if peer != sender && peer != msg.origin && peer != self.local_node_id {
                            if forwarded_msg.payload.len() <= DIRECT_PUSH_MAX_PAYLOAD {
                                forwards.push((peer, GossipWireProtocol::DirectPush(forwarded_msg.clone())));
                            } else {
                                forwards.push((peer, GossipWireProtocol::IHave {
                                    topic: forwarded_msg.topic.clone(),
                                    message_id: forwarded_msg.message_id,
                                }));
                            }
                        }
                    }
                }

                (Some(msg), forwards)
            }
            GossipWireProtocol::IHave { topic, message_id } => {
                // If unseen, request payload with IWANT
                if !self.deduplicator.contains(&message_id) {
                    let req = (sender, GossipWireProtocol::IWant { topic, message_id });
                    (None, vec![req])
                } else {
                    (None, Vec::new())
                }
            }
            GossipWireProtocol::IWant { topic: _, message_id } => {
                // If we have cached payload, reply with DirectPush
                if let Some(msg) = self.payload_cache.get(&message_id) {
                    let reply = (sender, GossipWireProtocol::DirectPush(msg.clone()));
                    (None, vec![reply])
                } else {
                    (None, Vec::new())
                }
            }
        }
    }
}
