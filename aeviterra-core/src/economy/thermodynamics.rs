use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

pub type ResourceId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EconomicError {
    MassConservationViolation,
    InsufficientBalance,
    CapabilityBudgetExceeded,
    UnauthorizedAction,
    ArithmeticOverflow,
    ResourceDepleted,
}

/// Strictly Conserved Material Ledger.
/// Every mass debit, credit, conversion, and sink balances in exact integer micro-units (F05, F26).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThermodynamicLedger {
    pub total_world_mass_microunits: u128,
    pub entity_balances: BTreeMap<(u64, ResourceId), u64>,
    pub world_sinks: BTreeMap<ResourceId, u64>,
}

impl ThermodynamicLedger {
    pub fn new(initial_world_mass: u128) -> Self {
        ThermodynamicLedger {
            total_world_mass_microunits: initial_world_mass,
            entity_balances: BTreeMap::new(),
            world_sinks: BTreeMap::new(),
        }
    }

    pub fn get_balance(&self, entity_id: u64, resource_id: ResourceId) -> u64 {
        self.entity_balances.get(&(entity_id, resource_id)).copied().unwrap_or(0)
    }

    /// Execute a recipe with zero mass leak: input_mass == output_mass + residue_mass.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_recipe(
        &mut self,
        crafter_id: u64,
        input_resource: ResourceId,
        input_amount: u64,
        output_resource: ResourceId,
        output_amount: u64,
        residue_resource: ResourceId,
        residue_amount: u64,
    ) -> Result<(), EconomicError> {
        // Invariant I5: Mass conservation
        let input_total = input_amount as u128;
        let output_total = (output_amount as u128)
            .checked_add(residue_amount as u128)
            .ok_or(EconomicError::ArithmeticOverflow)?;

        if input_total != output_total {
            return Err(EconomicError::MassConservationViolation);
        }

        // Check crafter balance
        let current_bal = self.get_balance(crafter_id, input_resource);
        if current_bal < input_amount {
            return Err(EconomicError::InsufficientBalance);
        }

        let out_bal = self.get_balance(crafter_id, output_resource);
        let new_out = out_bal.checked_add(output_amount).ok_or(EconomicError::ArithmeticOverflow)?;

        // Deposit residue to world sink (e.g. slag, ash, environmental matter)
        let sink_bal = self.world_sinks.get(&residue_resource).copied().unwrap_or(0);
        let new_sink = sink_bal.checked_add(residue_amount).ok_or(EconomicError::ArithmeticOverflow)?;

        // Atomic commit: Apply debit, credit, and sink deposit only after all invariant checks pass
        self.entity_balances.insert((crafter_id, input_resource), current_bal - input_amount);
        self.entity_balances.insert((crafter_id, output_resource), new_out);
        self.world_sinks.insert(residue_resource, new_sink);

        Ok(())
    }
}

/// Normative 180-day inactivity threshold at 20 TPS (180 * 86,400 * 20 = 311,040,000 ticks)
/// before stagnant Sovereign vaults incur algorithmic liquidity demurrage (V4.1 G6).
pub const DEMURRAGE_INACTIVITY_TICKS: u64 = 180 * 86_400 * 20;
pub const DEMURRAGE_RATE_BPS: u64 = 500; // 5.00% (500 basis points) redirected to public infrastructure

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SovereignVault {
    pub owner_id: u64,
    pub balance_sovereigns: u64,
    pub last_active_tick: u64,
}

/// Closed Dual-Currency & Demurrage Velocity Ledger (V4.1 §3 & §4 G6).
/// Prevents early cartels from monopolizing and stalling hard currency (Sovereigns) velocity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SovereignDemurrageLedger {
    pub total_sovereign_supply: u64,
    pub public_infrastructure_pool: u64,
    pub vaults: BTreeMap<u64, SovereignVault>,
}

impl SovereignDemurrageLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mint_certified_faucet(&mut self, owner_id: u64, amount: u64, current_tick: u64) -> Result<(), EconomicError> {
        self.total_sovereign_supply = self.total_sovereign_supply
            .checked_add(amount)
            .ok_or(EconomicError::ArithmeticOverflow)?;
        let vault = self.vaults.entry(owner_id).or_insert(SovereignVault {
            owner_id,
            balance_sovereigns: 0,
            last_active_tick: current_tick,
        });
        vault.balance_sovereigns = vault.balance_sovereigns
            .checked_add(amount)
            .ok_or(EconomicError::ArithmeticOverflow)?;
        vault.last_active_tick = current_tick;
        Ok(())
    }

    pub fn transfer(&mut self, from_id: u64, to_id: u64, amount: u64, current_tick: u64) -> Result<(), EconomicError> {
        let from_bal = self.vaults.get(&from_id).map(|v| v.balance_sovereigns).unwrap_or(0);
        if from_bal < amount {
            return Err(EconomicError::InsufficientBalance);
        }
        if let Some(v) = self.vaults.get_mut(&from_id) {
            v.balance_sovereigns -= amount;
            v.last_active_tick = current_tick;
        }
        let to_vault = self.vaults.entry(to_id).or_insert(SovereignVault {
            owner_id: to_id,
            balance_sovereigns: 0,
            last_active_tick: current_tick,
        });
        to_vault.balance_sovereigns = to_vault.balance_sovereigns
            .checked_add(amount)
            .ok_or(EconomicError::ArithmeticOverflow)?;
        to_vault.last_active_tick = current_tick;
        Ok(())
    }

    /// Apply V4.1 G6 demurrage decay to vaults uncirculated for > 180 days.
    /// Exact integer conservation: total_sovereign_supply == sum(vaults) + public_infrastructure_pool.
    pub fn apply_demurrage(&mut self, current_tick: u64) -> u64 {
        let mut total_collected = 0u64;
        for vault in self.vaults.values_mut() {
            if current_tick > vault.last_active_tick.saturating_add(DEMURRAGE_INACTIVITY_TICKS)
                && vault.balance_sovereigns > 0
            {
                let fee = ((vault.balance_sovereigns as u128 * DEMURRAGE_RATE_BPS as u128) / 10_000) as u64;
                let effective_fee = fee.max(1).min(vault.balance_sovereigns);
                vault.balance_sovereigns -= effective_fee;
                vault.last_active_tick = current_tick;
                self.public_infrastructure_pool = self.public_infrastructure_pool.saturating_add(effective_fee);
                total_collected = total_collected.saturating_add(effective_fee);
            }
        }
        total_collected
    }
}

/// Bounded Capability Envelope for Autonomous AI Agents (F25).
/// Prevents compromised or hallucinating planners from draining inventories or hijacking state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilityEnvelope {
    pub agent_id: u64,
    pub max_spend_per_epoch: u64,
    pub current_spent_in_epoch: u64,
    pub allowed_action_mask: u32,
    pub valid_until_epoch: u64,
    pub revoked: bool,
}

impl AgentCapabilityEnvelope {
    pub const ACTION_MOVE: u32 = 1 << 0;
    pub const ACTION_HARVEST: u32 = 1 << 1;
    pub const ACTION_CRAFT: u32 = 1 << 2;
    pub const ACTION_TRADE: u32 = 1 << 3;

    pub fn authorize_action(
        &mut self,
        action_flag: u32,
        spend_amount: u64,
        current_epoch: u64,
    ) -> Result<(), EconomicError> {
        if self.revoked || current_epoch > self.valid_until_epoch {
            return Err(EconomicError::UnauthorizedAction);
        }
        if (self.allowed_action_mask & action_flag) == 0 {
            return Err(EconomicError::UnauthorizedAction);
        }

        let new_spend = self.current_spent_in_epoch
            .checked_add(spend_amount)
            .ok_or(EconomicError::ArithmeticOverflow)?;

        if new_spend > self.max_spend_per_epoch {
            return Err(EconomicError::CapabilityBudgetExceeded);
        }

        self.current_spent_in_epoch = new_spend;
        Ok(())
    }

    pub fn reset_epoch(&mut self) {
        self.current_spent_in_epoch = 0;
    }
}
