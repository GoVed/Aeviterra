use super::peer::NodeId;
use std::collections::BTreeMap;

pub const MAX_REPUTATION_SCORE: i32 = 1_000;
pub const MIN_REPUTATION_SCORE: i32 = -10_000;

pub const THRESHOLD_THROTTLED: i32    = 0;
pub const THRESHOLD_QUARANTINED: i32  = -500;
pub const THRESHOLD_PERMABAN: i32     = -1_000;

pub const BASE_BAN_DURATION_SEC: u64 = 900; // 15 minutes base ban
pub const HALF_LIFE_DECAY_SEC: u64   = 1_800; // 30 minutes score decay toward 0

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerTier {
    FullPeer,
    Throttled,
    Quarantined,
    Banned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReputationEvent {
    ValidBftVote,
    ValidActionEnvelope,
    GameplayRateLimitExceeded,
    MalformedWireFrame,
    StaleSequenceReplay,
    EquivocationOrForgedProof,
}

impl ReputationEvent {
    pub fn delta(self) -> i32 {
        match self {
            ReputationEvent::ValidBftVote => 1,
            ReputationEvent::ValidActionEnvelope => 1,
            ReputationEvent::GameplayRateLimitExceeded => -50,
            ReputationEvent::MalformedWireFrame => -200,
            ReputationEvent::StaleSequenceReplay => -500,
            ReputationEvent::EquivocationOrForgedProof => -10_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub node_id: NodeId,
    pub subnet_prefix: String,
    pub score: i32,
    pub last_update_sec: u64,
    pub infraction_count: u32,
    pub banned_until_sec: Option<u64>,
}

/// Peer Reputation & Automated Banning Ledger.
/// Tracks peer behavior, applies exponential score decay, and executes progressive subnet quarantines.
#[derive(Debug, Clone, Default)]
pub struct PeerReputationLedger {
    pub records: BTreeMap<NodeId, PeerRecord>,
    pub subnet_bans: BTreeMap<String, u64>, // subnet -> banned_until_sec
}

impl PeerReputationLedger {
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
            subnet_bans: BTreeMap::new(),
        }
    }

    /// Retrieve or initialize record for peer.
    pub fn get_or_create(&mut self, node_id: NodeId, subnet_prefix: String, now_sec: u64) -> &mut PeerRecord {
        self.records.entry(node_id).or_insert_with(|| PeerRecord {
            node_id,
            subnet_prefix,
            score: 0,
            last_update_sec: now_sec,
            infraction_count: 0,
            banned_until_sec: None,
        })
    }

    /// Record a reputation event, updating score and checking ban thresholds.
    pub fn record_event(
        &mut self,
        node_id: NodeId,
        subnet_prefix: String,
        event: ReputationEvent,
        now_sec: u64,
    ) -> PeerTier {
        let delta = event.delta();
        let record = self.get_or_create(node_id, subnet_prefix.clone(), now_sec);

        // Apply decay before adding delta
        Self::apply_decay(record, now_sec);

        record.score = (record.score + delta).clamp(MIN_REPUTATION_SCORE, MAX_REPUTATION_SCORE);
        record.last_update_sec = now_sec;

        if delta < 0 {
            record.infraction_count = record.infraction_count.saturating_add(1);
        }

        let tier = Self::evaluate_tier(record.score);
        if tier == PeerTier::Banned {
            let shift = record.infraction_count.min(6);
            let multiplier = 1u64 << shift;
            let ban_duration = BASE_BAN_DURATION_SEC.saturating_mul(multiplier);
            let banned_until = now_sec.saturating_add(ban_duration);

            record.banned_until_sec = Some(banned_until);
            self.subnet_bans.insert(subnet_prefix, banned_until);
        }

        tier
    }

    /// Query current status of peer.
    pub fn peer_tier(&mut self, node_id: &NodeId, subnet_prefix: &str, now_sec: u64) -> PeerTier {
        // 1. Check subnet-wide ban
        if let Some(&banned_until) = self.subnet_bans.get(subnet_prefix)
            && now_sec < banned_until {
            return PeerTier::Banned;
        }

        // 2. Check individual peer record
        if let Some(record) = self.records.get_mut(node_id) {
            if let Some(banned_until) = record.banned_until_sec {
                if now_sec < banned_until {
                    return PeerTier::Banned;
                } else {
                    record.banned_until_sec = None;
                    record.score = THRESHOLD_QUARANTINED; // reinstate with quarantine score
                }
            }

            Self::apply_decay(record, now_sec);
            Self::evaluate_tier(record.score)
        } else {
            PeerTier::FullPeer
        }
    }

    pub fn is_banned(&mut self, node_id: &NodeId, subnet_prefix: &str, now_sec: u64) -> bool {
        self.peer_tier(node_id, subnet_prefix, now_sec) == PeerTier::Banned
    }

    fn apply_decay(record: &mut PeerRecord, now_sec: u64) {
        if now_sec <= record.last_update_sec {
            return;
        }

        let elapsed = now_sec - record.last_update_sec;
        let half_lives = elapsed / HALF_LIFE_DECAY_SEC;

        if half_lives > 0 {
            // Decay toward neutral (0)
            record.score /= 1 << half_lives.min(10);
            record.last_update_sec = now_sec;
        }
    }

    fn evaluate_tier(score: i32) -> PeerTier {
        if score <= THRESHOLD_PERMABAN {
            PeerTier::Banned
        } else if score <= THRESHOLD_QUARANTINED {
            PeerTier::Quarantined
        } else if score < THRESHOLD_THROTTLED {
            PeerTier::Throttled
        } else {
            PeerTier::FullPeer
        }
    }
}
