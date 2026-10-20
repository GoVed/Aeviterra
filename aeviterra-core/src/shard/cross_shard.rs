use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, BTreeSet};

pub type TxId = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    Declared,
    Prepared,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossShardTx {
    pub tx_id: TxId,
    pub src_shard: u32,
    pub dst_shard: u32,
    pub entity_id: u64,
    pub expected_version: u64,
    pub declare_tick: u64,
    pub expiry_tick: u64,
    pub status: TxStatus,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxError {
    AlreadyExists,
    NotFound,
    InvalidStateTransition,
    VersionMismatch,
    Expired,
    EntityLocked,
    ReplayDetected,
}

/// Asynchronous Non-Blocking 2-Phase Commit Manager with Durable Journal Decisions.
/// Solves F04 (cross-shard double-spends) and F12 (stranded locks / unilateral unlock races).
pub struct CrossShardManager {
    pub shard_id: u32,
    pub active_txs: BTreeMap<TxId, CrossShardTx>,
    pub locked_entities: BTreeMap<u64, TxId>,
    pub entity_versions: BTreeMap<u64, u64>,
    pub consumed_tx_ids: BTreeSet<TxId>,
}

impl CrossShardManager {
    pub fn new(shard_id: u32) -> Self {
        CrossShardManager {
            shard_id,
            active_txs: BTreeMap::new(),
            locked_entities: BTreeMap::new(),
            entity_versions: BTreeMap::new(),
            consumed_tx_ids: BTreeSet::new(),
        }
    }

    /// Register or update the authoritative ownership version for an entity on this shard.
    pub fn register_entity_version(&mut self, entity_id: u64, version: u64) {
        self.entity_versions.insert(entity_id, version);
    }

    /// Step 1: Declare a transaction on the source shard.
    #[allow(clippy::too_many_arguments)]
    pub fn declare_tx(
        &mut self,
        tx_id: TxId,
        dst_shard: u32,
        entity_id: u64,
        entity_version: u64,
        current_tick: u64,
        timeout_ticks: u64,
        payload: Vec<u8>,
    ) -> Result<&CrossShardTx, TxError> {
        if self.consumed_tx_ids.contains(&tx_id) {
            return Err(TxError::ReplayDetected);
        }
        if self.active_txs.contains_key(&tx_id) {
            return Err(TxError::AlreadyExists);
        }
        if self.locked_entities.contains_key(&entity_id) {
            return Err(TxError::EntityLocked);
        }
        if let Some(&auth_ver) = self.entity_versions.get(&entity_id) {
            if auth_ver != entity_version {
                return Err(TxError::VersionMismatch);
            }
        } else {
            self.entity_versions.insert(entity_id, entity_version);
        }

        let expiry_tick = current_tick.saturating_add(timeout_ticks);
        let tx = CrossShardTx {
            tx_id,
            src_shard: self.shard_id,
            dst_shard,
            entity_id,
            expected_version: entity_version,
            declare_tick: current_tick,
            expiry_tick,
            status: TxStatus::Declared,
            payload,
        };

        self.locked_entities.insert(entity_id, tx_id);
        self.active_txs.insert(tx_id, tx);
        Ok(self.active_txs.get(&tx_id).unwrap())
    }

    /// Step 2: Prepare phase on destination shard.
    pub fn prepare_tx(&mut self, mut tx: CrossShardTx, current_tick: u64) -> Result<(), TxError> {
        if self.consumed_tx_ids.contains(&tx.tx_id) {
            return Err(TxError::ReplayDetected);
        }
        if current_tick > tx.expiry_tick {
            return Err(TxError::Expired);
        }
        if self.locked_entities.contains_key(&tx.entity_id) {
            return Err(TxError::EntityLocked);
        }
        if let Some(&auth_ver) = self.entity_versions.get(&tx.entity_id) {
            if auth_ver != tx.expected_version {
                return Err(TxError::VersionMismatch);
            }
        } else {
            self.entity_versions.insert(tx.entity_id, tx.expected_version);
        }

        tx.status = TxStatus::Prepared;
        self.locked_entities.insert(tx.entity_id, tx.tx_id);
        self.active_txs.insert(tx.tx_id, tx);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalDecisionType {
    Commit,
    Abort,
}

/// Durable, consensus-certified decision from the Control Journal (V4 §11.2).
/// Eliminates unilateral-unlock races (F12) across partitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedJournalDecision {
    pub tx_id: TxId,
    pub decision: JournalDecisionType,
    pub journal_height: u64,
    pub certificate_digest: [u8; 32],
}

impl CrossShardManager {
    /// Step 3/4: Apply a durable, consensus-certified decision from the Control Journal.
    /// Expiry is resolved solely by the journal at a named height; no local clock unlocks anything.
    /// Idempotent on duplicate delivery of the same certified decision (T03).
    pub fn apply_journal_decision(&mut self, decision: &CertifiedJournalDecision) -> Result<(), TxError> {
        let tx = self.active_txs.get_mut(&decision.tx_id).ok_or(TxError::NotFound)?;
        
        match decision.decision {
            JournalDecisionType::Commit => {
                if tx.status == TxStatus::Committed {
                    return Ok(()); // Idempotent retry delivery
                }
                if tx.status != TxStatus::Prepared && tx.status != TxStatus::Declared {
                    return Err(TxError::InvalidStateTransition);
                }
                tx.status = TxStatus::Committed;
                let next_ver = tx.expected_version.saturating_add(1);
                self.entity_versions.insert(tx.entity_id, next_ver);
            }
            JournalDecisionType::Abort => {
                if tx.status == TxStatus::Aborted {
                    return Ok(()); // Idempotent retry delivery
                }
                if tx.status == TxStatus::Committed {
                    return Err(TxError::InvalidStateTransition);
                }
                tx.status = TxStatus::Aborted;
            }
        }

        self.locked_entities.remove(&tx.entity_id);
        self.consumed_tx_ids.insert(decision.tx_id);
        Ok(())
    }
}
