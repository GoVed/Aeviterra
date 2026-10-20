use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

pub type Hash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureSymbol {
    pub symbol_id: u32,
    pub is_repair: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureManifest {
    pub object_hash: Hash,
    pub total_bytes: usize,
    pub k_source_symbols: usize,
    pub symbol_size: usize,
    pub symbol_commitments: Vec<Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErasureError {
    InsufficientSymbols,
    CorruptedData,
    InvalidSymbolSize,
    ManifestMismatch,
}

/// Compute per-fragment cryptographic commitment (R2 §20, F07, F08).
pub fn compute_symbol_commitment(symbol_id: u32, is_repair: bool, data: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVITERRA_ERASURE_SYMBOL_V1");
    hasher.update(symbol_id.to_be_bytes());
    hasher.update([u8::from(is_repair)]);
    hasher.update((data.len() as u32).to_be_bytes());
    hasher.update(data);
    hasher.finalize().into()
}

/// GF(2^8) multiplication modulo irreducible polynomial x^8 + x^4 + x^3 + x + 1 (0x11B).
#[inline]
fn gf256_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p: u8 = 0;
    for _ in 0..8 {
        if (b & 1) != 0 {
            p ^= a;
        }
        let hi_bit = (a & 0x80) != 0;
        a <<= 1;
        if hi_bit {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    p
}

/// GF(2^8) multiplicative inverse via Fermat's Little Theorem: a^(2^8 - 2) = a^254.
#[inline]
fn gf256_inv(a: u8) -> u8 {
    debug_assert_ne!(a, 0, "Cannot invert 0 in GF(2^8)");
    let mut res: u8 = 1;
    let mut base = a;
    let mut exp: u8 = 254;
    while exp > 0 {
        if (exp & 1) != 0 {
            res = gf256_mul(res, base);
        }
        base = gf256_mul(base, base);
        exp >>= 1;
    }
    res
}

/// Cauchy MDS generator coefficient in GF(2^8) for row `symbol_id` and column `col` (0..k).
#[inline]
fn cauchy_coeff(symbol_id: usize, col: usize, k_source: usize) -> u8 {
    if symbol_id < k_source {
        u8::from(symbol_id == col)
    } else {
        // x_r = symbol_id as u8 (in [k, k+r)), y_j = col as u8 (in [0, k))
        // Disjoint sets guarantee (x_r ^ y_j) != 0 in GF(2^8)
        let x = (symbol_id & 0xFF) as u8;
        let y = (col & 0xFF) as u8;
        gf256_inv(x ^ y)
    }
}

/// Authenticated MDS Cauchy Reed-Solomon Erasure Coding Engine over GF(2^8) (V4.1 §16, F07, F08, T11).
/// Guarantees exact reconstruction from ANY K distinct valid symbols (source or repair),
/// deduplicates repeated symbol IDs, and authenticates fragments before and after decoding.
pub struct ErasureCodec {
    pub symbol_size: usize,
    pub k_source: usize,
}

impl ErasureCodec {
    pub fn new(symbol_size: usize, k_source: usize) -> Self {
        ErasureCodec {
            symbol_size: symbol_size.max(1),
            k_source: k_source.clamp(1, 128),
        }
    }

    /// Encode data payload into K source symbols and R repair symbols over GF(2^8).
    pub fn encode(&self, payload: &[u8], r_repair: usize) -> (ErasureManifest, Vec<ErasureSymbol>) {
        let mut hasher = Sha256::new();
        hasher.update(payload);
        let object_hash: Hash = hasher.finalize().into();

        let total_bytes = payload.len();
        let max_repair = r_repair.min(256usize.saturating_sub(self.k_source));
        let mut symbols = Vec::with_capacity(self.k_source + max_repair);
        let mut symbol_commitments = Vec::with_capacity(self.k_source + max_repair);

        // Generate K systematic source symbols
        for i in 0..self.k_source {
            let start = (i * self.symbol_size).min(total_bytes);
            let end = ((i + 1) * self.symbol_size).min(total_bytes);
            let mut block = payload[start..end].to_vec();
            block.resize(self.symbol_size, 0); // Zero-pad to symbol_size

            let sym_id = i as u32;
            symbol_commitments.push(compute_symbol_commitment(sym_id, false, &block));
            symbols.push(ErasureSymbol {
                symbol_id: sym_id,
                is_repair: false,
                data: block,
            });
        }

        // Generate R Cauchy MDS repair symbols over GF(2^8)
        for r in 0..max_repair {
            let sym_idx = self.k_source + r;
            let mut repair_data = vec![0u8; self.symbol_size];
            for (col, src) in symbols[..self.k_source].iter().enumerate() {
                let coeff = cauchy_coeff(sym_idx, col, self.k_source);
                for (b_out, &b_in) in repair_data.iter_mut().zip(src.data.iter()) {
                    *b_out ^= gf256_mul(b_in, coeff);
                }
            }

            let sym_id = sym_idx as u32;
            symbol_commitments.push(compute_symbol_commitment(sym_id, true, &repair_data));
            symbols.push(ErasureSymbol {
                symbol_id: sym_id,
                is_repair: true,
                data: repair_data,
            });
        }

        let manifest = ErasureManifest {
            object_hash,
            total_bytes,
            k_source_symbols: self.k_source,
            symbol_size: self.symbol_size,
            symbol_commitments,
        };

        (manifest, symbols)
    }

    /// Reconstruct payload from any K distinct valid symbols (source or repair).
    pub fn decode(
        &self,
        manifest: &ErasureManifest,
        received_symbols: &[ErasureSymbol],
    ) -> Result<Vec<u8>, ErasureError> {
        let k = manifest.k_source_symbols;
        if k == 0 || k > 128 || manifest.symbol_size == 0 {
            return Err(ErasureError::ManifestMismatch);
        }

        // 1. Deduplicate by symbol_id and authenticate each symbol before expensive decode (R2 §20)
        let mut distinct_symbols: BTreeMap<usize, &ErasureSymbol> = BTreeMap::new();
        for sym in received_symbols {
            if sym.data.len() != manifest.symbol_size {
                return Err(ErasureError::InvalidSymbolSize);
            }
            let idx = sym.symbol_id as usize;
            if idx >= 256 {
                return Err(ErasureError::ManifestMismatch);
            }
            let expected_is_repair = idx >= k;
            if sym.is_repair != expected_is_repair {
                return Err(ErasureError::ManifestMismatch);
            }

            // Authenticate against manifest fragment commitment if available
            if idx < manifest.symbol_commitments.len() {
                let actual_commit = compute_symbol_commitment(sym.symbol_id, sym.is_repair, &sym.data);
                if actual_commit != manifest.symbol_commitments[idx] {
                    return Err(ErasureError::CorruptedData);
                }
            }

            distinct_symbols.entry(idx).or_insert(sym);
            if distinct_symbols.len() == k {
                break;
            }
        }

        if distinct_symbols.len() < k {
            return Err(ErasureError::InsufficientSymbols);
        }

        // 2. Build K x K linear system M * X = Y over GF(2^8) and solve via Gauss-Jordan elimination
        let chosen: Vec<(usize, &ErasureSymbol)> = distinct_symbols.into_iter().take(k).collect();
        let mut mat = vec![vec![0u8; k]; k];
        let mut rhs: Vec<Vec<u8>> = chosen.iter().map(|(_, s)| s.data.clone()).collect();

        for (row, &(sym_idx, _)) in chosen.iter().enumerate() {
            for (col, cell) in mat[row].iter_mut().enumerate().take(k) {
                *cell = cauchy_coeff(sym_idx, col, k);
            }
        }

        for pivot in 0..k {
            // Find non-zero pivot row in column `pivot`
            let pivot_row = (pivot..k)
                .find(|&r| mat[r][pivot] != 0)
                .ok_or(ErasureError::InsufficientSymbols)?;

            if pivot_row != pivot {
                mat.swap(pivot, pivot_row);
                rhs.swap(pivot, pivot_row);
            }

            let inv_diag = gf256_inv(mat[pivot][pivot]);
            for cell in mat[pivot].iter_mut().take(k).skip(pivot) {
                *cell = gf256_mul(*cell, inv_diag);
            }
            for b in rhs[pivot].iter_mut() {
                *b = gf256_mul(*b, inv_diag);
            }

            // Eliminate column `pivot` in all other rows
            let pivot_row_vals = mat[pivot].clone();
            let pivot_rhs = rhs[pivot].clone();
            for row in 0..k {
                if row == pivot {
                    continue;
                }
                let factor = mat[row][pivot];
                if factor != 0 {
                    for (cell, &piv_val) in mat[row].iter_mut().zip(pivot_row_vals.iter()).take(k).skip(pivot) {
                        *cell ^= gf256_mul(factor, piv_val);
                    }
                    for (b_row, &b_piv) in rhs[row].iter_mut().zip(pivot_rhs.iter()) {
                        *b_row ^= gf256_mul(factor, b_piv);
                    }
                }
            }
        }

        // 3. Assemble K decoded source blocks and truncate to total_bytes
        let mut reconstructed = Vec::with_capacity(k * manifest.symbol_size);
        for block in rhs {
            reconstructed.extend_from_slice(&block);
        }
        if manifest.total_bytes > reconstructed.len() {
            return Err(ErasureError::ManifestMismatch);
        }
        reconstructed.truncate(manifest.total_bytes);

        // 4. Final end-to-end object hash verification against certified manifest
        let mut hasher = Sha256::new();
        hasher.update(&reconstructed);
        let computed_hash: Hash = hasher.finalize().into();

        if computed_hash != manifest.object_hash {
            return Err(ErasureError::CorruptedData);
        }

        Ok(reconstructed)
    }
}
