use crate::assets::grammar::StructuralPrimitive;
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

pub const MIN_TENURE_TICKS: u64 = 518_400; // ~30 days at 20 ticks/sec
pub const MIN_TAX_BOND_MARKS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomesteadPlot {
    pub plot_id: u64,
    pub settlement_id: u32,
    pub owner_id: u64,
    pub built_structures: Vec<StructuralPrimitive>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidentRecord {
    pub actor_id: u64,
    pub settlement_id: u32,
    pub registration_tick: u64,
    pub cumulative_taxes_paid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementCharter {
    pub settlement_id: u32,
    pub name: String,
    pub mayor_id: u64,
    pub treasury_marks: u64,
    pub plots: BTreeMap<u64, HomesteadPlot>,
    pub residents: BTreeMap<u64, ResidentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementError {
    PlotAlreadyClaimed,
    PlotNotFound,
    NotPlotOwner,
    InvalidStructureConstraints,
    SybilVotingDisqualified,
    InsufficientTenure,
    InsufficientTreasuryFunds,
    TaxCalculationOverflow,
}

impl SettlementCharter {
    pub fn new(settlement_id: u32, name: String, mayor_id: u64) -> Self {
        SettlementCharter {
            settlement_id,
            name,
            mayor_id,
            treasury_marks: 0,
            plots: BTreeMap::new(),
            residents: BTreeMap::new(),
        }
    }

    /// Claim homestead plot (V4 §10, housing is a guaranteed right).
    pub fn claim_plot(&mut self, plot_id: u64, owner_id: u64, current_tick: u64) -> Result<(), SettlementError> {
        if self.plots.contains_key(&plot_id) {
            return Err(SettlementError::PlotAlreadyClaimed);
        }

        let plot = HomesteadPlot {
            plot_id,
            settlement_id: self.settlement_id,
            owner_id,
            built_structures: Vec::new(),
        };

        self.plots.insert(plot_id, plot);
        self.residents.entry(owner_id).or_insert(ResidentRecord {
            actor_id: owner_id,
            settlement_id: self.settlement_id,
            registration_tick: current_tick,
            cumulative_taxes_paid: 0,
        });

        Ok(())
    }

    /// Construct a verified structural primitive on player's homestead plot.
    pub fn build_structure(&mut self, plot_id: u64, owner_id: u64, structure: StructuralPrimitive) -> Result<(), SettlementError> {
        let plot = self.plots.get_mut(&plot_id).ok_or(SettlementError::PlotNotFound)?;
        if plot.owner_id != owner_id {
            return Err(SettlementError::NotPlotOwner);
        }
        if !structure.validate_constraints() {
            return Err(SettlementError::InvalidStructureConstraints);
        }

        plot.built_structures.push(structure);
        Ok(())
    }

    /// Municipal Anti-Sybil Mayoral Voting Eligibility Check (V4.1 G5, P04).
    /// Requires proven economic tenure: 30 days residency + tax contribution.
    /// Prevents rival guilds from deploying 1,000 bot accounts to seize towns.
    pub fn can_vote_for_mayor(&self, actor_id: u64, current_tick: u64) -> Result<bool, SettlementError> {
        let resident = self.residents.get(&actor_id).ok_or(SettlementError::SybilVotingDisqualified)?;

        if current_tick < resident.registration_tick + MIN_TENURE_TICKS {
            return Err(SettlementError::InsufficientTenure);
        }
        if resident.cumulative_taxes_paid < MIN_TAX_BOND_MARKS {
            return Err(SettlementError::SybilVotingDisqualified);
        }

        Ok(true)
    }

    /// Deposit tax or municipal bond payment into the settlement treasury.
    /// Updates the resident's cumulative tax record and the municipal treasury balance.
    pub fn pay_tax(&mut self, actor_id: u64, amount: u64) -> Result<(), SettlementError> {
        let resident = self.residents.get_mut(&actor_id).ok_or(SettlementError::SybilVotingDisqualified)?;
        resident.cumulative_taxes_paid = resident.cumulative_taxes_paid.checked_add(amount)
            .ok_or(SettlementError::TaxCalculationOverflow)?;
        self.treasury_marks = self.treasury_marks.checked_add(amount)
            .ok_or(SettlementError::TaxCalculationOverflow)?;
        Ok(())
    }

    /// Disburse funds from municipal treasury for public infrastructure or defense.
    pub fn disburse_treasury(&mut self, amount: u64) -> Result<(), SettlementError> {
        if self.treasury_marks < amount {
            return Err(SettlementError::InsufficientTreasuryFunds);
        }
        self.treasury_marks -= amount;
        Ok(())
    }

    /// Elect or transfer mayorship to an eligible resident candidate.
    pub fn elect_mayor(&mut self, candidate_id: u64, current_tick: u64) -> Result<(), SettlementError> {
        self.can_vote_for_mayor(candidate_id, current_tick)?;
        self.mayor_id = candidate_id;
        Ok(())
    }
}
