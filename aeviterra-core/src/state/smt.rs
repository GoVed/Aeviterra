use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

pub const HASH_SIZE: usize = 32;
pub type Hash = [u8; HASH_SIZE];

pub const EMPTY_HASH: Hash = [0u8; HASH_SIZE];

// Cryptographic Domain Separators (Pre-image attack defense)
pub const DOMAIN_LEAF: u8 = 0x00;
pub const DOMAIN_INNER: u8 = 0x01;
pub const DOMAIN_SMT_DEFAULT: u8 = 0x02;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtProof {
    pub key: u64,
    pub value_hash: Hash,
    pub siblings: Vec<Hash>,
}

/// Deterministic Cryptographic Hash function with Domain Separation.
pub fn hash_leaf(key: u64, value: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update([DOMAIN_LEAF]);
    hasher.update(key.to_be_bytes());
    hasher.update(value);
    hasher.finalize().into()
}

pub fn hash_inner(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update([DOMAIN_INNER]);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Fixed-depth 64-bit Sparse Merkle Tree (SMT) for Canonical ECS State.
/// Guaranteed identical root hash across all CPU architectures and runtimes.
pub struct SparseMerkleTree {
    pub default_hashes: [Hash; 65],
    pub leaves: BTreeMap<u64, Hash>,
}

impl Default for SparseMerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

impl SparseMerkleTree {
    pub fn new() -> Self {
        let mut default_hashes = [EMPTY_HASH; 65];
        // Precompute default hashes for empty branches at each level
        for i in 1..=64 {
            let prev = default_hashes[i - 1];
            default_hashes[i] = hash_inner(&prev, &prev);
        }

        SparseMerkleTree {
            default_hashes,
            leaves: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: u64, value_hash: Hash) {
        self.leaves.insert(key, value_hash);
    }

    pub fn get(&self, key: u64) -> Option<&Hash> {
        self.leaves.get(&key)
    }

    /// Compute the canonical Merkle Root for the current leaf state.
    pub fn compute_root(&self) -> Hash {
        if self.leaves.is_empty() {
            return self.default_hashes[64];
        }
        let sorted_leaves: Vec<(u64, Hash)> = self.leaves.iter().map(|(&k, &v)| (k, v)).collect();
        self.compute_slice_root(64, &sorted_leaves)
    }

    fn compute_slice_root(&self, depth: usize, slice: &[(u64, Hash)]) -> Hash {
        if slice.is_empty() {
            return self.default_hashes[depth];
        }
        if depth == 0 {
            return slice[0].1;
        }

        let bit_pos = depth - 1;
        let split = slice.partition_point(|&(k, _)| ((k >> bit_pos) & 1) == 0);
        let left_hash = self.compute_slice_root(bit_pos, &slice[..split]);
        let right_hash = self.compute_slice_root(bit_pos, &slice[split..]);

        hash_inner(&left_hash, &right_hash)
    }

    /// Generate an inclusion proof for a given key.
    pub fn prove(&self, key: u64) -> SmtProof {
        let value_hash = self.leaves.get(&key).copied().unwrap_or(self.default_hashes[0]);
        let sorted_leaves: Vec<(u64, Hash)> = self.leaves.iter().map(|(&k, &v)| (k, v)).collect();
        let mut siblings = vec![EMPTY_HASH; 64];

        let mut current_slice: &[(u64, Hash)] = &sorted_leaves;
        for depth in (1..=64).rev() {
            let bit_pos = depth - 1;
            let split = current_slice.partition_point(|&(k, _)| ((k >> bit_pos) & 1) == 0);
            let (left_slice, right_slice) = current_slice.split_at(split);
            let bit = (key >> bit_pos) & 1;
            if bit == 0 {
                siblings[bit_pos] = self.compute_slice_root(bit_pos, right_slice);
                current_slice = left_slice;
            } else {
                siblings[bit_pos] = self.compute_slice_root(bit_pos, left_slice);
                current_slice = right_slice;
            }
        }

        SmtProof {
            key,
            value_hash,
            siblings,
        }
    }

    /// Verify an SMT inclusion/membership proof against a canonical root.
    pub fn verify_proof(root: &Hash, proof: &SmtProof) -> bool {
        if proof.siblings.len() != 64 {
            return false;
        }

        let mut current_hash = proof.value_hash;
        for (depth, sibling) in proof.siblings.iter().enumerate() {
            let bit = (proof.key >> depth) & 1;
            current_hash = if bit == 0 {
                hash_inner(&current_hash, sibling)
            } else {
                hash_inner(sibling, &current_hash)
            };
        }

        &current_hash == root
    }
}

/// Canonical Typed Entity-Component-System (ECS) State Record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRecord {
    pub entity_id: u64,
    pub authority_shard: u32,
    pub ownership_version: u64,
    pub mass_microunits: u64,
    pub position_x: i128,
    pub position_y: i128,
    pub position_z: i128,
    pub velocity_x: i128,
    pub velocity_y: i128,
    pub velocity_z: i128,
    pub inventory_root: Hash,
    pub state_flags: u32,
}

impl EntityRecord {
    /// Compute deterministic leaf hash for this entity.
    pub fn compute_leaf_hash(&self) -> Hash {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(&self.entity_id.to_be_bytes());
        bytes.extend_from_slice(&self.authority_shard.to_be_bytes());
        bytes.extend_from_slice(&self.ownership_version.to_be_bytes());
        bytes.extend_from_slice(&self.mass_microunits.to_be_bytes());
        bytes.extend_from_slice(&self.position_x.to_be_bytes());
        bytes.extend_from_slice(&self.position_y.to_be_bytes());
        bytes.extend_from_slice(&self.position_z.to_be_bytes());
        bytes.extend_from_slice(&self.velocity_x.to_be_bytes());
        bytes.extend_from_slice(&self.velocity_y.to_be_bytes());
        bytes.extend_from_slice(&self.velocity_z.to_be_bytes());
        bytes.extend_from_slice(&self.inventory_root);
        bytes.extend_from_slice(&self.state_flags.to_be_bytes());

        hash_leaf(self.entity_id, &bytes)
    }
}
