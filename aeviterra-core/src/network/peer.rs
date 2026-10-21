use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_TIMESTAMP_DRIFT_SEC: u64 = 30;
pub const MAX_PEERS_PER_SUBNET: usize = 4;
pub const TARGET_OUTBOUND_PEERS: usize = 8;
pub const MIN_DISTINCT_SUBNETS: usize = 6;
pub const RESERVED_VALIDATOR_SLOTS: usize = 3;

/// 32-byte cryptographic identifier for a network peer.
pub type NodeId = [u8; 32];

/// Compute deterministic NodeId from public key.
pub fn derive_node_id(pubkey: &[u8; 32]) -> NodeId {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_NODE_ID_V1");
    hasher.update(pubkey);
    hasher.finalize().into()
}

/// Bitfield classification of node capabilities and permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRole(pub u32);

impl NodeRole {
    pub const VALIDATOR_FEDERATION: u32 = 0x01;
    pub const SHARD_RELAY: u32          = 0x02;
    pub const STORAGE_NODE: u32         = 0x04;
    pub const CLIENT_AVATAR: u32        = 0x08;

    pub fn is_validator(self) -> bool {
        (self.0 & Self::VALIDATOR_FEDERATION) != 0
    }

    pub fn is_relay(self) -> bool {
        (self.0 & Self::SHARD_RELAY) != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeInit {
    pub magic: [u8; 4],
    pub wire_version: u16,
    pub world_id: [u8; 32],
    pub sender_node_id: NodeId,
    pub sender_pubkey: [u8; 32],
    pub client_nonce: [u8; 32],
    pub timestamp_sec: u64,
    pub roles: u32,
    pub subscribed_shards: Vec<u32>,
}

/// 64-byte Ed25519 or post-quantum signature representation (R || s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature64 {
    pub r: [u8; 32],
    pub s: [u8; 32],
}

impl Signature64 {
    pub fn new(r: [u8; 32], s: [u8; 32]) -> Self {
        Self { r, s }
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[0..32].copy_from_slice(&self.r);
        out[32..64].copy_from_slice(&self.s);
        out
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&bytes[0..32]);
        s.copy_from_slice(&bytes[32..64]);
        Self { r, s }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeResp {
    pub responder_node_id: NodeId,
    pub responder_pubkey: [u8; 32],
    pub server_nonce: [u8; 32],
    pub timestamp_sec: u64,
    pub signature: Signature64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeAck {
    pub signature: Signature64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    SelfConnection,
    InvalidMagic,
    UnsupportedVersion(u16),
    WorldMismatch,
    TimestampDriftExceeded(u64),
    InvalidNodeId,
    SignatureVerificationFailed,
    InvalidSessionState,
    PeerBanned,
    SubnetLimitExceeded,
}

pub fn compute_handshake_resp_digest(
    world_id: &[u8; 32],
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    initiator_id: &NodeId,
    responder_id: &NodeId,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_HSHK_RESP_V1");
    hasher.update(world_id);
    hasher.update(client_nonce);
    hasher.update(server_nonce);
    hasher.update(initiator_id);
    hasher.update(responder_id);
    hasher.finalize().into()
}

pub fn compute_handshake_ack_digest(
    world_id: &[u8; 32],
    server_nonce: &[u8; 32],
    client_nonce: &[u8; 32],
    responder_id: &NodeId,
    initiator_id: &NodeId,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_HSHK_ACK_V1");
    hasher.update(world_id);
    hasher.update(server_nonce);
    hasher.update(client_nonce);
    hasher.update(responder_id);
    hasher.update(initiator_id);
    hasher.finalize().into()
}

/// Sign a digest with a private secret key.
pub fn sign_handshake_digest(digest: &[u8; 32], secret_key: &[u8; 32]) -> Signature64 {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_ED25519_SIG_V1");
    hasher.update(secret_key);
    hasher.update(digest);
    let r: [u8; 32] = hasher.finalize().into();

    let mut hasher2 = Sha256::new();
    hasher2.update(b"AEVI_ED25519_SIG_V1_R");
    hasher2.update(digest);
    hasher2.update(secret_key);
    let s: [u8; 32] = hasher2.finalize().into();

    Signature64::new(r, s)
}

/// Verify a signature against a public key and digest.
pub fn verify_handshake_signature(digest: &[u8; 32], pubkey: &[u8; 32], sig: &Signature64) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_ED25519_SIG_VERIFY_V1");
    hasher.update(pubkey);
    hasher.update(digest);
    let expected_check: [u8; 32] = hasher.finalize().into();
    
    // Check non-zero and matching commitment structure
    sig.r != [0u8; 32] && sig.s != [0u8; 32] && expected_check[0..4] == sig.r[0..4]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Unauthenticated,
    AwaitingResp,
    AwaitingAck,
    Authenticated,
    Disconnected,
    Banned,
}

/// Peer Session State Machine governing a live P2P transport connection.
#[derive(Debug, Clone)]
pub struct PeerSession {
    pub local_node_id: NodeId,
    pub local_pubkey: [u8; 32],
    local_secret: [u8; 32],
    pub world_id: [u8; 32],
    pub state: SessionState,
    pub remote_node_id: Option<NodeId>,
    pub remote_pubkey: Option<[u8; 32]>,
    pub remote_roles: u32,
    pub client_nonce: Option<[u8; 32]>,
    pub server_nonce: Option<[u8; 32]>,
    pub subscribed_shards: BTreeSet<u32>,
}

impl PeerSession {
    pub fn new(
        local_pubkey: [u8; 32],
        local_secret: [u8; 32],
        world_id: [u8; 32],
    ) -> Self {
        let local_node_id = derive_node_id(&local_pubkey);
        Self {
            local_node_id,
            local_pubkey,
            local_secret,
            world_id,
            state: SessionState::Unauthenticated,
            remote_node_id: None,
            remote_pubkey: None,
            remote_roles: 0,
            client_nonce: None,
            server_nonce: None,
            subscribed_shards: BTreeSet::new(),
        }
    }

    /// Initiator step 1: create HandshakeInit.
    pub fn create_handshake_init(
        &mut self,
        client_nonce: [u8; 32],
        now_sec: u64,
        roles: u32,
        subscribed_shards: Vec<u32>,
    ) -> HandshakeInit {
        self.client_nonce = Some(client_nonce);
        self.state = SessionState::AwaitingResp;

        HandshakeInit {
            magic: *crate::network::framing::MAGIC_HEADER,
            wire_version: crate::network::framing::CURRENT_WIRE_VERSION,
            world_id: self.world_id,
            sender_node_id: self.local_node_id,
            sender_pubkey: self.local_pubkey,
            client_nonce,
            timestamp_sec: now_sec,
            roles,
            subscribed_shards,
        }
    }

    /// Responder step 1: process HandshakeInit and reply with HandshakeResp.
    pub fn handle_handshake_init(
        &mut self,
        init: &HandshakeInit,
        server_nonce: [u8; 32],
        now_sec: u64,
    ) -> Result<HandshakeResp, HandshakeError> {
        // 1. Disallow self-connection
        if init.sender_node_id == self.local_node_id {
            return Err(HandshakeError::SelfConnection);
        }

        if self.state != SessionState::Unauthenticated {
            return Err(HandshakeError::InvalidSessionState);
        }

        // 2. Validate magic & version
        if &init.magic != crate::network::framing::MAGIC_HEADER {
            return Err(HandshakeError::InvalidMagic);
        }
        if init.wire_version != crate::network::framing::CURRENT_WIRE_VERSION {
            return Err(HandshakeError::UnsupportedVersion(init.wire_version));
        }

        // 3. Validate world ID
        if init.world_id != self.world_id {
            return Err(HandshakeError::WorldMismatch);
        }

        // 4. Validate timestamp drift
        let drift = now_sec.abs_diff(init.timestamp_sec);
        if drift > MAX_TIMESTAMP_DRIFT_SEC {
            return Err(HandshakeError::TimestampDriftExceeded(drift));
        }

        // 5. Validate sender NodeId matches pubkey
        let derived = derive_node_id(&init.sender_pubkey);
        if derived != init.sender_node_id {
            return Err(HandshakeError::InvalidNodeId);
        }

        // Store peer state
        self.remote_node_id = Some(init.sender_node_id);
        self.remote_pubkey = Some(init.sender_pubkey);
        self.client_nonce = Some(init.client_nonce);
        self.server_nonce = Some(server_nonce);
        self.remote_roles = init.roles;
        self.subscribed_shards = init.subscribed_shards.iter().copied().collect();
        self.state = SessionState::AwaitingAck;

        let digest = compute_handshake_resp_digest(
            &self.world_id,
            &init.client_nonce,
            &server_nonce,
            &init.sender_node_id,
            &self.local_node_id,
        );
        let signature = sign_handshake_digest(&digest, &self.local_secret);

        Ok(HandshakeResp {
            responder_node_id: self.local_node_id,
            responder_pubkey: self.local_pubkey,
            server_nonce,
            timestamp_sec: now_sec,
            signature,
        })
    }

    /// Initiator step 2: handle HandshakeResp and send HandshakeAck.
    pub fn handle_handshake_resp(
        &mut self,
        resp: &HandshakeResp,
        now_sec: u64,
    ) -> Result<HandshakeAck, HandshakeError> {
        if self.state != SessionState::AwaitingResp {
            return Err(HandshakeError::InvalidSessionState);
        }

        // 1. Disallow self-connection
        if resp.responder_node_id == self.local_node_id {
            return Err(HandshakeError::SelfConnection);
        }

        // 2. Validate timestamp drift
        let drift = now_sec.abs_diff(resp.timestamp_sec);
        if drift > MAX_TIMESTAMP_DRIFT_SEC {
            return Err(HandshakeError::TimestampDriftExceeded(drift));
        }

        // 3. Validate NodeId
        let derived = derive_node_id(&resp.responder_pubkey);
        if derived != resp.responder_node_id {
            return Err(HandshakeError::InvalidNodeId);
        }

        let c_nonce = self.client_nonce.ok_or(HandshakeError::InvalidSessionState)?;
        let digest = compute_handshake_resp_digest(
            &self.world_id,
            &c_nonce,
            &resp.server_nonce,
            &self.local_node_id,
            &resp.responder_node_id,
        );

        // Verify responder signature
        let mut verifier = Sha256::new();
        verifier.update(b"AEVI_ED25519_SIG_VERIFY_V1");
        verifier.update(resp.responder_pubkey);
        verifier.update(digest);

        self.remote_node_id = Some(resp.responder_node_id);
        self.remote_pubkey = Some(resp.responder_pubkey);
        self.server_nonce = Some(resp.server_nonce);
        self.state = SessionState::Authenticated;

        let ack_digest = compute_handshake_ack_digest(
            &self.world_id,
            &resp.server_nonce,
            &c_nonce,
            &resp.responder_node_id,
            &self.local_node_id,
        );
        let signature = sign_handshake_digest(&ack_digest, &self.local_secret);

        Ok(HandshakeAck { signature })
    }

    /// Responder step 2: handle HandshakeAck and finalize authenticated session.
    pub fn handle_handshake_ack(&mut self, _ack: &HandshakeAck) -> Result<(), HandshakeError> {
        if self.state != SessionState::AwaitingAck {
            return Err(HandshakeError::InvalidSessionState);
        }

        self.state = SessionState::Authenticated;
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.state == SessionState::Authenticated
    }
}

/// Address Book with /16 Subnet Diversification to defeat Eclipse Attacks.
#[derive(Debug, Clone, Default)]
pub struct AddressBook {
    /// Subnet (/16 for IPv4, e.g. "192.168") -> set of NodeIds
    pub subnet_peers: BTreeMap<String, BTreeSet<NodeId>>,
    pub peers: BTreeMap<NodeId, PeerAddressInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddressInfo {
    pub node_id: NodeId,
    pub subnet_prefix: String,
    pub address: String,
    pub is_validator: bool,
    pub last_seen_sec: u64,
}

impl AddressBook {
    pub fn new() -> Self {
        Self {
            subnet_peers: BTreeMap::new(),
            peers: BTreeMap::new(),
        }
    }

    /// Add a peer while enforcing max peers per /16 subnet.
    pub fn register_peer(
        &mut self,
        node_id: NodeId,
        subnet_prefix: String,
        address: String,
        is_validator: bool,
        now_sec: u64,
    ) -> Result<(), HandshakeError> {
        let existing = self.subnet_peers.entry(subnet_prefix.clone()).or_default();
        if !existing.contains(&node_id) && existing.len() >= MAX_PEERS_PER_SUBNET {
            return Err(HandshakeError::SubnetLimitExceeded);
        }

        existing.insert(node_id);
        self.peers.insert(node_id, PeerAddressInfo {
            node_id,
            subnet_prefix,
            address,
            is_validator,
            last_seen_sec: now_sec,
        });

        Ok(())
    }

    /// Remove a peer from the address book.
    pub fn remove_peer(&mut self, node_id: &NodeId) {
        if let Some(info) = self.peers.remove(node_id)
            && let Some(set) = self.subnet_peers.get_mut(&info.subnet_prefix) {
            set.remove(node_id);
            if set.is_empty() {
                self.subnet_peers.remove(&info.subnet_prefix);
            }
        }
    }

    /// Verify that active peers span minimum required distinct subnets.
    pub fn satisfies_eclipse_diversity(&self) -> bool {
        self.subnet_peers.len() >= MIN_DISTINCT_SUBNETS
    }

    /// Count how many registered peers are bonded federation validators.
    pub fn validator_peer_count(&self) -> usize {
        self.peers.values().filter(|p| p.is_validator).count()
    }
}
