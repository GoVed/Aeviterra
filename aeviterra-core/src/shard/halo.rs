use crate::math::fixed::{Q64, Vec3Fixed, AabbFixed, SweptCapsule, MathError};
use serde::{Serialize, Deserialize};

pub const CHUNK_SIZE_METERS: i64 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChunkCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        ChunkCoord { x, y, z }
    }

    /// Convert continuous world position to discrete chunk coordinate using exact floor division (F11, F14).
    /// Ensures negative coordinates (e.g., -0.5m) map to chunk -1 rather than truncating toward zero into chunk 0.
    pub fn from_world_pos(pos: Vec3Fixed) -> Self {
        let chunk_size = CHUNK_SIZE_METERS;
        let cx = pos.x.to_int_floor().div_euclid(chunk_size) as i32;
        let cy = pos.y.to_int_floor().div_euclid(chunk_size) as i32;
        let cz = pos.z.to_int_floor().div_euclid(chunk_size) as i32;
        ChunkCoord { x: cx, y: cy, z: cz }
    }

    /// Compute the world-space bounding box for this chunk.
    pub fn aabb(&self) -> Result<AabbFixed, MathError> {
        let min_x = Q64::from_int((self.x as i64) * CHUNK_SIZE_METERS)?;
        let min_y = Q64::from_int((self.y as i64) * CHUNK_SIZE_METERS)?;
        let min_z = Q64::from_int((self.z as i64) * CHUNK_SIZE_METERS)?;

        let max_x = min_x.checked_add(Q64::from_int(CHUNK_SIZE_METERS)?)?;
        let max_y = min_y.checked_add(Q64::from_int(CHUNK_SIZE_METERS)?)?;
        let max_z = min_z.checked_add(Q64::from_int(CHUNK_SIZE_METERS)?)?;

        Ok(AabbFixed::new(
            Vec3Fixed::new(min_x, min_y, min_z),
            Vec3Fixed::new(max_x, max_y, max_z),
        ))
    }
}

/// Dynamic Swept-Volume Halo Boundary Calculator.
/// Replaces the vulnerable static 16m halo with velocity/rollback-aware margin:
/// halo_radius >= r_interact + v_max * dt + 0.5 * a_max * dt^2 + eps
pub struct DynamicHaloCalculator {
    pub default_interact_radius: Q64,
    pub numerical_margin: Q64,
}

impl DynamicHaloCalculator {
    pub fn new(default_interact_radius: Q64, numerical_margin: Q64) -> Self {
        DynamicHaloCalculator {
            default_interact_radius,
            numerical_margin,
        }
    }

    /// Compute conservative halo margin required for an entity with given physical parameters.
    pub fn compute_required_margin(
        &self,
        interaction_radius: Q64,
        max_velocity: Q64,
        max_accel: Q64,
        rollback_horizon_seconds: Q64,
    ) -> Result<Q64, MathError> {
        if interaction_radius.0 < 0
            || max_velocity.0 < 0
            || max_accel.0 < 0
            || rollback_horizon_seconds.0 < 0
            || self.numerical_margin.0 < 0
        {
            return Err(MathError::InvalidDomain);
        }

        // v * dt
        let v_dt = max_velocity.checked_mul(rollback_horizon_seconds)?;
        
        // 0.5 * a * dt^2
        let dt_sq = rollback_horizon_seconds.checked_mul(rollback_horizon_seconds)?;
        let a_dt_sq = max_accel.checked_mul(dt_sq)?;
        let half_a_dt_sq = a_dt_sq.checked_mul(Q64::HALF)?;

        // Sum components
        let dynamic_range = v_dt.checked_add(half_a_dt_sq)?;
        let total_margin = interaction_radius
            .checked_add(dynamic_range)?
            .checked_add(self.numerical_margin)?;

        Ok(total_margin)
    }

    /// Determine which chunks overlap with a continuous swept trajectory.
    pub fn calculate_affected_chunks(
        &self,
        trajectory: &SweptCapsule,
    ) -> Result<Vec<ChunkCoord>, MathError> {
        let swept_aabb = trajectory.bounding_box()?;
        
        let min_chunk = ChunkCoord::from_world_pos(swept_aabb.min);
        let max_chunk = ChunkCoord::from_world_pos(swept_aabb.max);

        let mut affected = Vec::new();
        for cx in min_chunk.x..=max_chunk.x {
            for cy in min_chunk.y..=max_chunk.y {
                for cz in min_chunk.z..=max_chunk.z {
                    let chunk = ChunkCoord::new(cx, cy, cz);
                    if chunk.aabb()?.intersects(&swept_aabb) {
                        affected.push(chunk);
                    }
                }
            }
        }

        Ok(affected)
    }
}

/// Octree Split/Merge Hysteresis Controller (F23, T06).
/// Prevents boundary oscillation thrashing when crowds move back and forth across chunk seams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardHysteresisController {
    pub shard_id: u32,
    pub max_capacity_entities: u32,
    pub split_threshold_pct: u32, // Normative: 80%
    pub merge_threshold_pct: u32, // Normative: 20%
    pub min_lifetime_ticks: u64,  // Normative: 300 ticks
    pub last_transition_tick: u64,
    pub is_split: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologyDecision {
    Stable,
    SplitIntoChildren,
    MergeIntoParent,
}

impl ShardHysteresisController {
    pub fn new(shard_id: u32, max_capacity_entities: u32, creation_tick: u64) -> Self {
        ShardHysteresisController {
            shard_id,
            max_capacity_entities: max_capacity_entities.max(1),
            split_threshold_pct: 80,
            merge_threshold_pct: 20,
            min_lifetime_ticks: 300,
            last_transition_tick: creation_tick,
            is_split: false,
        }
    }

    /// Evaluate whether the shard should split, merge, or remain stable at `current_tick`.
    pub fn evaluate_topology(&mut self, current_load: u32, current_tick: u64) -> TopologyDecision {
        if current_tick < self.last_transition_tick.saturating_add(self.min_lifetime_ticks) {
            return TopologyDecision::Stable;
        }

        let load_pct = (current_load as u64 * 100) / (self.max_capacity_entities as u64);

        if !self.is_split && load_pct >= self.split_threshold_pct as u64 {
            self.is_split = true;
            self.last_transition_tick = current_tick;
            TopologyDecision::SplitIntoChildren
        } else if self.is_split && load_pct <= self.merge_threshold_pct as u64 {
            self.is_split = false;
            self.last_transition_tick = current_tick;
            TopologyDecision::MergeIntoParent
        } else {
            TopologyDecision::Stable
        }
    }
}
