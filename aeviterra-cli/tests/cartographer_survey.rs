use aeviterra_cli::terrain::GenesisTerrainGenerator;
use aeviterra_core::math::fixed::Q64;
use aeviterra_core::shard::halo::ChunkCoord;
use std::collections::BTreeMap;

#[test]
fn test_cartographer_continental_survey() {
    let seed = *b"AEVITERRA_GENESIS_SEED_2026_100Y";
    let generator = GenesisTerrainGenerator::new(seed);

    let mut biome_counts = BTreeMap::new();
    let mut min_elevation = i64::MAX;
    let mut max_elevation = i64::MIN;

    // Survey a 2,000m x 2,000m regional grid around spawn ([-1000, 1000] in steps of 20m)
    let step = 20;
    let mut total_samples = 0;

    for z in (-1000..=1000).step_by(step) {
        for x in (-1000..=1000).step_by(step) {
            let qx = Q64::from_int(x).unwrap();
            let qz = Q64::from_int(z).unwrap();
            let elev = generator.elevation_at(qx, qz).unwrap().to_int_trunc();
            let biome = generator.biome_at(qx, qz).unwrap();

            *biome_counts.entry(format!("{:?}", biome)).or_insert(0) += 1;
            total_samples += 1;

            if elev < min_elevation { min_elevation = elev; }
            if elev > max_elevation { max_elevation = elev; }
        }
    }

    println!("\n=== CARTOGRAPHER REGIONAL SURVEY (2km x 2km around Origin) ===");
    println!("Total Samples: {}", total_samples);
    println!("Elevation Range: [{}m, {}m]", min_elevation, max_elevation);
    for (biome, count) in &biome_counts {
        let pct = (*count as f64 / total_samples as f64) * 100.0;
        println!("  Biome {:<16}: {:>5} samples ({:>5.1}%)", biome, count, pct);
    }

    // Now survey macro-scale continental grid: 20,000m x 20,000m (20km x 20km)
    let macro_step = 200;
    let mut macro_biome_counts = BTreeMap::new();
    let mut macro_min_elev = i64::MAX;
    let mut macro_max_elev = i64::MIN;
    let mut macro_samples = 0;

    for z in (-10000..=10000).step_by(macro_step) {
        for x in (-10000..=10000).step_by(macro_step) {
            let qx = Q64::from_int(x).unwrap();
            let qz = Q64::from_int(z).unwrap();
            let elev = generator.elevation_at(qx, qz).unwrap().to_int_trunc();
            let biome = generator.biome_at(qx, qz).unwrap();

            *macro_biome_counts.entry(format!("{:?}", biome)).or_insert(0) += 1;
            macro_samples += 1;

            if elev < macro_min_elev { macro_min_elev = elev; }
            if elev > macro_max_elev { macro_max_elev = elev; }
        }
    }

    // Mega-scale 100,000 samples across 100km x 100km
    let mut peak_elev = i64::MIN;
    let mut peak_coord = (0, 0);
    let mut deepwilds_found = 0;
    let mut mountain_found = 0;

    for z in (-50000..=50000).step_by(1000) {
        for x in (-50000..=50000).step_by(1000) {
            let qx = Q64::from_int(x).unwrap();
            let qz = Q64::from_int(z).unwrap();
            let elev = generator.elevation_at(qx, qz).unwrap().to_int_trunc();
            if elev > peak_elev {
                peak_elev = elev;
                peak_coord = (x, z);
            }
            if elev > 130 { mountain_found += 1; }
            else if elev > 80 { deepwilds_found += 1; }
        }
    }

    println!("\n=== CONTINENTAL MEGA SEARCH (100km x 100km, 10,201 points) ===");
    println!("Global Peak Elevation Found: {}m at ({}, {})", peak_elev, peak_coord.0, peak_coord.1);
    println!("DeepWilds Points (>80m): {}", deepwilds_found);
    println!("MountainPeak Points (>130m): {}", mountain_found);

    println!("\n=== CARTOGRAPHER CONTINENTAL MACRO SURVEY (20km x 20km) ===");
    println!("Total Samples: {}", macro_samples);
    println!("Elevation Range: [{}m, {}m]", macro_min_elev, macro_max_elev);
    for (biome, count) in &macro_biome_counts {
        let pct = (*count as f64 / macro_samples as f64) * 100.0;
        println!("  Biome {:<16}: {:>5} samples ({:>5.1}%)", biome, count, pct);
    }

    // Survey Resource Node distribution across 100 chunks
    let mut resource_counts = BTreeMap::new();
    let mut empty_chunks = 0;
    let mut active_chunks = 0;

    for cz in -5..=5 {
        for cx in -5..=5 {
            let coord = ChunkCoord::new(cx, 0, cz);
            if let Some(node) = generator.mineral_vein_for_chunk(coord).unwrap() {
                active_chunks += 1;
                let res_name = match node.resource_id {
                    1 => "Iron Ore",
                    2 => "Oak Timber",
                    3 => "Silver",
                    _ => "Unknown",
                };
                *resource_counts.entry(res_name).or_insert(0) += 1;
            } else {
                empty_chunks += 1;
            }
        }
    }

    println!("\n=== CHUNK RESOURCE DEPOSIT SURVEY (11x11 = 121 Chunks) ===");
    println!("Active Resource Chunks: {} ({:.1}%)", active_chunks, (active_chunks as f64 / 121.0) * 100.0);
    println!("Empty Chunks:           {} ({:.1}%)", empty_chunks, (empty_chunks as f64 / 121.0) * 100.0);
    for (res, count) in &resource_counts {
        println!("  Resource {:<12}: {} deposits", res, count);
    }

    // Find closest MountainPeak, DeepWilds, and Heartlands to spawn (64, 64)
    println!("\n=== PROXIMITY SEARCH FROM SPAWN (64, 64) ===");
    let mut closest_mountain: Option<(i64, i64, i64, f64)> = None;
    let mut closest_deepwilds: Option<(i64, i64, i64, f64)> = None;
    let mut closest_heartlands: Option<(i64, i64, i64, f64)> = None;

    for dist in (10..5000).step_by(10) {
        // check ring
        for angle_deg in (0..360).step_by(15) {
            let rad = (angle_deg as f64).to_radians();
            let x = 64 + (dist as f64 * rad.cos()) as i64;
            let z = 64 + (dist as f64 * rad.sin()) as i64;
            let qx = Q64::from_int(x).unwrap();
            let qz = Q64::from_int(z).unwrap();
            let elev = generator.elevation_at(qx, qz).unwrap().to_int_trunc();
            let d = ((x - 64).pow(2) as f64 + (z - 64).pow(2) as f64).sqrt();

            if elev > 130 && closest_mountain.is_none() {
                closest_mountain = Some((x, elev, z, d));
            }
            if elev > 80 && elev <= 130 && closest_deepwilds.is_none() {
                closest_deepwilds = Some((x, elev, z, d));
            }
            if elev <= 40 && closest_heartlands.is_none() {
                closest_heartlands = Some((x, elev, z, d));
            }
        }
        if closest_mountain.is_some() && closest_deepwilds.is_some() && closest_heartlands.is_some() {
            break;
        }
    }

    if let Some((x, y, z, d)) = closest_mountain {
        println!("  Closest MountainPeak:  ({}, {}m, {}) — {:.1}m away", x, y, z, d);
    } else {
        println!("  Closest MountainPeak:  NONE within 5km!");
    }
    if let Some((x, y, z, d)) = closest_deepwilds {
        println!("  Closest DeepWilds:     ({}, {}m, {}) — {:.1}m away", x, y, z, d);
    } else {
        println!("  Closest DeepWilds:     NONE within 5km!");
    }
    println!("\n=== NEARBY RESOURCE NODES DETAIL ===");
    for cz in -3..=3 {
        for cx in -3..=3 {
            let coord = ChunkCoord::new(cx, 0, cz);
            if let Some(node) = generator.mineral_vein_for_chunk(coord).unwrap() {
                let name = match node.resource_id {
                    1 => "Iron Ore",
                    2 => "Oak Timber",
                    3 => "Silver",
                    _ => "Unknown",
                };
                let x = node.world_pos.x.to_int_trunc();
                let y = node.world_pos.y.to_int_trunc();
                let z = node.world_pos.z.to_int_trunc();
                let biome = generator.biome_at(node.world_pos.x, node.world_pos.z).unwrap();
                println!("  Chunk [{}, {}] -> Pos: ({}, {}m, {}) | Res: {:<10} ({} units) | Biome: {:?}",
                    cx, cz, x, y, z, name, node.max_amount, biome);
            }
        }
    }
}
