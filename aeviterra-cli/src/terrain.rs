use aeviterra_core::math::fixed::{Q64, Vec3Fixed, MathError};
use aeviterra_core::shard::halo::ChunkCoord;
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BiomeType {
    Heartlands,
    FrontierWilds,
    DeepWilds,
    MountainPeak,
    UnderdeepEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceNode {
    pub node_id: u64,
    pub chunk: ChunkCoord,
    pub world_pos: Vec3Fixed,
    pub resource_id: u32, // 1 = Iron Ore, 2 = Oak Timber, 3 = Silver
    pub remaining_amount: u64,
    pub max_amount: u64,
}

/// Deterministic Procedural Continental Terrain Generator.
/// Derived strictly from an immutable 32-byte Genesis Seed (V4.1 §1).
/// Continental geography requires 0 persistent bytes on disk.
pub struct GenesisTerrainGenerator {
    pub seed: [u8; 32],
}

impl GenesisTerrainGenerator {
    pub fn new(seed: [u8; 32]) -> Self {
        GenesisTerrainGenerator { seed }
    }

    /// Hash coordinate pair with seed to generate deterministic pseudo-random unit value.
    fn hash_coords(&self, x: i64, z: i64) -> u64 {
        let mut hasher = Sha256::new();
        hasher.update(b"AEVITERRA_GENESIS_NOISE_V1");
        hasher.update(self.seed);
        hasher.update(x.to_be_bytes());
        hasher.update(z.to_be_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        u64::from_be_bytes(digest[0..8].try_into().unwrap())
    }

    /// Deterministic 2D Value Noise with Cubic Spline Interpolation in Q64.64.
    pub fn noise_2d(&self, world_x: Q64, world_z: Q64, scale: i64) -> Result<Q64, MathError> {
        let scale_q64 = Q64::from_int(scale)?;
        let sx = world_x.checked_div(scale_q64)?;
        let sz = world_z.checked_div(scale_q64)?;

        let x0 = sx.to_int_floor();
        let z0 = sz.to_int_floor();
        let x1 = x0.saturating_add(1);
        let z1 = z0.saturating_add(1);

        // Fractional offsets in [0.0, 1.0) even for negative world coordinates (F14)
        let fx = sx.checked_sub(Q64::from_int(x0)?)?;
        let fz = sz.checked_sub(Q64::from_int(z0)?)?;

        // Smoothstep interpolation weights: 3*t^2 - 2*t^3
        let wx = self.smoothstep(fx)?;
        let wz = self.smoothstep(fz)?;

        // Lattice corner values normalized to [0, 1] in Q64.64
        let v00 = self.normalized_hash(x0, z0);
        let v10 = self.normalized_hash(x1, z0);
        let v01 = self.normalized_hash(x0, z1);
        let v11 = self.normalized_hash(x1, z1);

        // Bilinear interpolation
        let top = self.lerp(v00, v10, wx)?;
        let bottom = self.lerp(v01, v11, wx)?;
        self.lerp(top, bottom, wz)
    }

    fn smoothstep(&self, t: Q64) -> Result<Q64, MathError> {
        let three = Q64::from_int(3)?;
        let two = Q64::from_int(2)?;
        let t_sq = t.checked_mul(t)?;
        let t_cube = t_sq.checked_mul(t)?;
        
        let p1 = three.checked_mul(t_sq)?;
        let p2 = two.checked_mul(t_cube)?;
        p1.checked_sub(p2)
    }

    fn lerp(&self, a: Q64, b: Q64, t: Q64) -> Result<Q64, MathError> {
        let diff = b.checked_sub(a)?;
        let scaled = diff.checked_mul(t)?;
        a.checked_add(scaled)
    }

    fn normalized_hash(&self, x: i64, z: i64) -> Q64 {
        let h = self.hash_coords(x, z);
        // Map top 63 bits to positive Q64.64 fraction [0.0, 1.0)
        let raw = ((h >> 1) as i128) % Q64::ONE_RAW;
        Q64::from_raw(raw)
    }

    /// Compute continental elevation (in meters) at continuous coordinate (x, z).
    pub fn elevation_at(&self, x: Q64, z: Q64) -> Result<Q64, MathError> {
        // Multi-octave fractional Brownian motion (fBm)
        let oct1 = self.noise_2d(x, z, 512)?; // Continental landmasses
        let oct2 = self.noise_2d(x, z, 128)?; // Regional hills
        let oct3 = self.noise_2d(x, z, 32)?;  // Local boulders/roughness

        let h1 = oct1.checked_mul(Q64::from_int(120)?)?; // 0 to 120m
        let h2 = oct2.checked_mul(Q64::from_int(40)?)?;  // 0 to 40m
        let h3 = oct3.checked_mul(Q64::from_int(10)?)?;  // 0 to 10m

        h1.checked_add(h2)?.checked_add(h3)
    }

    /// Determine biome classification based on elevation and moisture.
    pub fn biome_at(&self, x: Q64, z: Q64) -> Result<BiomeType, MathError> {
        let elev = self.elevation_at(x, z)?;
        let elev_m = elev.to_int_trunc();

        if elev_m > 130 {
            Ok(BiomeType::MountainPeak)
        } else if elev_m > 80 {
            Ok(BiomeType::DeepWilds)
        } else if elev_m > 40 {
            Ok(BiomeType::FrontierWilds)
        } else {
            Ok(BiomeType::Heartlands)
        }
    }

    /// Deterministically derive mineral ore veins inside a spatial chunk.
    pub fn mineral_vein_for_chunk(&self, chunk: ChunkCoord) -> Result<Option<ResourceNode>, MathError> {
        let h = self.hash_coords(chunk.x as i64, chunk.z as i64);
        
        // 40% probability of an active mineral deposit in chunk
        if (h % 100) < 40 {
            let cx_m = (chunk.x as i64) * 128 + 64;
            let cz_m = (chunk.z as i64) * 128 + 64;
            let pos_x = Q64::from_int(cx_m)?;
            let pos_z = Q64::from_int(cz_m)?;
            let pos_y = self.elevation_at(pos_x, pos_z)?;

            let resource_id = match (h >> 8) % 3 {
                0 => 1, // Iron Ore
                1 => 2, // Oak Timber
                _ => 3, // Silver
            };

            let max_amount = 50 + (h % 150);

            Ok(Some(ResourceNode {
                node_id: h,
                chunk,
                world_pos: Vec3Fixed::new(pos_x, pos_y, pos_z),
                resource_id: resource_id as u32,
                remaining_amount: max_amount,
                max_amount,
            }))
        } else {
            Ok(None)
        }
    }
}
