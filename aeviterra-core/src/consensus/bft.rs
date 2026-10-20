use super::envelope::{BftProposal, BftVote, BftPhase, BftCommitCertificate, Hash, Signature};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsensusError {
    EquivocationDetected,
    UnauthorizedValidator,
    InvalidPhase,
    InsufficientVotes,
    DuplicateVote,
    ProposalMismatch,
    StaleView,
}

/// Durable Validator Signing Log (WAL) to prevent double-signing across crashes (F27).
#[derive(Debug, Default, Clone)]
pub struct ValidatorWal {
    pub last_signed_view: u64,
    pub last_signed_height: u64,
    pub signed_proposals: BTreeMap<(u64, u64), Hash>, // (height, view) -> proposal_hash
}

impl ValidatorWal {
    pub fn record_vote(&mut self, height: u64, view: u64, proposal_hash: Hash) -> Result<(), ConsensusError> {
        if let Some(&existing) = self.signed_proposals.get(&(height, view))
            && existing != proposal_hash {
                return Err(ConsensusError::EquivocationDetected);
            }
        self.last_signed_height = height;
        self.last_signed_view = view;
        self.signed_proposals.insert((height, view), proposal_hash);
        Ok(())
    }
}

pub type VoteMap = BTreeMap<(BftPhase, u64, u64), BTreeMap<u64, (BftVote, Signature)>>;

/// Shard BFT Consensus State Machine.
/// Enforces admitted validator membership (F01), proposal hash binding (F02),
/// equivocation detection (F27), and 2f + 1 quorum finality (T01, T02).
pub struct ShardBftEngine {
    pub shard_id: u32,
    pub committee_size: usize,
    pub admitted_validators: BTreeSet<u64>,
    pub current_height: u64,
    pub current_view: u64,
    pub active_proposal: Option<BftProposal>,
    pub votes: VoteMap, // (phase, height, view) -> validator_id -> vote
    pub committed_certificates: BTreeMap<u64, BftCommitCertificate>,
    pub wal: ValidatorWal,
}

impl ShardBftEngine {
    pub fn new(shard_id: u32, committee_size: usize) -> Self {
        let size = committee_size.max(1);
        let admitted_validators = (1..=(size as u64)).collect();
        ShardBftEngine {
            shard_id,
            committee_size: size,
            admitted_validators,
            current_height: 1,
            current_view: 0,
            active_proposal: None,
            votes: BTreeMap::new(),
            committed_certificates: BTreeMap::new(),
            wal: ValidatorWal::default(),
        }
    }

    pub fn with_committee(shard_id: u32, admitted_validators: BTreeSet<u64>) -> Self {
        let size = admitted_validators.len().max(1);
        ShardBftEngine {
            shard_id,
            committee_size: size,
            admitted_validators,
            current_height: 1,
            current_view: 0,
            active_proposal: None,
            votes: BTreeMap::new(),
            committed_certificates: BTreeMap::new(),
            wal: ValidatorWal::default(),
        }
    }

    pub fn set_proposal(&mut self, proposal: BftProposal) -> Result<(), ConsensusError> {
        if proposal.height != self.current_height || proposal.view != self.current_view {
            return Err(ConsensusError::StaleView);
        }
        if proposal.shard_id != self.shard_id {
            return Err(ConsensusError::ProposalMismatch);
        }
        self.active_proposal = Some(proposal);
        Ok(())
    }

    pub fn register_vote(&mut self, vote: BftVote, sig: Signature) -> Result<Option<BftCommitCertificate>, ConsensusError> {
        if vote.height != self.current_height || vote.view != self.current_view {
            return Err(ConsensusError::StaleView);
        }

        // F01 / T01: Reject votes from non-admitted / Sybil player identities
        if !self.admitted_validators.contains(&vote.validator_id) {
            return Err(ConsensusError::UnauthorizedValidator);
        }

        let key = (vote.phase, vote.height, vote.view);
        if let Some(phase_votes) = self.votes.get(&key)
            && let Some((existing_vote, _)) = phase_votes.get(&vote.validator_id) {
                if existing_vote.proposal_hash != vote.proposal_hash {
                    return Err(ConsensusError::EquivocationDetected);
                }
                return Err(ConsensusError::DuplicateVote);
            }

        // Verify proposal matches the active proposal for this height/view
        let active_hash = match &self.active_proposal {
            Some(prop) => prop.compute_hash(),
            None => return Err(ConsensusError::ProposalMismatch),
        };
        if vote.proposal_hash != active_hash {
            return Err(ConsensusError::ProposalMismatch);
        }

        let phase_votes = self.votes.entry(key).or_default();
        phase_votes.insert(vote.validator_id, (vote.clone(), sig));

        let f = (self.committee_size - 1) / 3;
        let quorum = 2 * f + 1;

        if vote.phase == BftPhase::Commit {
            let matching_votes: Vec<(u64, Signature)> = phase_votes
                .iter()
                .filter(|(_, (v, _))| v.proposal_hash == active_hash)
                .map(|(&v_id, (_, s))| (v_id, *s))
                .collect();

            if matching_votes.len() >= quorum {
                let signers = matching_votes.iter().map(|(id, _)| *id).collect();
                let signatures = matching_votes.iter().map(|(_, s)| *s).collect();

                let cert = BftCommitCertificate {
                    proposal_hash: active_hash,
                    view: vote.view,
                    height: vote.height,
                    signers,
                    signatures,
                };

                self.committed_certificates.insert(self.current_height, cert.clone());
                self.current_height += 1;
                self.current_view = 0;
                self.active_proposal = None;

                return Ok(Some(cert));
            }
        }

        Ok(None)
    }
}
