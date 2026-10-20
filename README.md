# Aeviterra — Century-Scale Decentralized MMO Engine

[![Rust 2024](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/Tests-25%2F25_Passing-brightgreen.svg)](#verification--test-suite)
[![Clippy](https://img.shields.io/badge/Clippy-Zero_Warnings-blue.svg)](#verification--test-suite)
[![Specification](https://img.shields.io/badge/Spec-Revision_4-purple.svg)](SPECIFICATION.md)
[![License](https://img.shields.io/badge/License-MIT%2FApache--2.0-blue.svg)](#license)

**Aeviterra** (Protocol Revision 4) is a deterministic, peer-operated MMORPG substrate and playable simulation engine engineered for century-scale persistence. It provides a sovereign, cheat-proof virtual world governed entirely by cryptographic consensus, deterministic fixed-point physics, and closed macroeconomic thermodynamics without centralized servers, trusted operators, or unbounded state bloat.

The authoritative design specification is available in markdown at [SPECIFICATION.md](SPECIFICATION.md) and as an archival print document at [Aeviterra-Specification.pdf](Aeviterra-Specification.pdf).

---

## Workspace Structure

```text
aeviterra/
├── Cargo.toml                  # Workspace manifest
├── SPECIFICATION.md            # Normative Protocol & Systems Specification (Revision 4)
├── Aeviterra-Specification.pdf # Formatted Specification Document (Revision 4)
├── aeviterra-core/             # Cryptographic, consensus, physics & game mechanics library
│   ├── src/
│   │   ├── math/fixed.rs       # Software Q64.64 math, i256 intermediates, swept capsules
│   │   ├── state/smt.rs        # 64-bit Sparse Merkle Tree with domain separation
│   │   ├── consensus/          # Domain-separated action envelopes, replay guards, HotStuff BFT
│   │   ├── shard/              # Dynamic swept-volume halos, hysteresis & cross-shard 2PC
│   │   ├── offline/lease.rs    # Leased state enclaves, nullifiers & reclamation grace barrier
│   │   ├── randomness/         # Threshold beacon commit-reveal RNG & rejection sampling
│   │   ├── assets/grammar.rs   # Semantic generative asset grammar tokens
│   │   ├── economy/            # Conserved mass ledger, agent capability envelopes & demurrage
│   │   ├── storage/erasure.rs  # Authenticated MDS Cauchy Reed-Solomon coding over GF(2^8)
│   │   ├── network/framing.rs  # Bounded wire framing & time-locked combat mempool
│   │   ├── governance/fpeg.rs  # Formalized Protocol Epoch Gates (FPEG) & cryptosystem migration
│   │   └── game/               # Classless combat, CC diminishing returns, pity & settlements
│   └── tests/security_tests.rs # 19 adversarial security & invariant tests
└── aeviterra-cli/              # Stage 0 Playable Simulation CLI
    ├── src/
    │   ├── terrain.rs          # 32-byte Genesis Seed procedural terrain & deterministic veins
    │   ├── simulation.rs       # 20 TPS deterministic state transition loop & SMT roots
    │   └── main.rs             # Interactive terminal MMO client & TPS benchmark runner
    └── tests/playability_tests.rs # 6 end-to-end gameplay & determinism tests
```

---

## Core Subsystems & Guarantees

Every subsystem in `aeviterra-core` and `aeviterra-cli` implements mathematically bounded guarantees:

| Subsystem | Core Guarantees | Code Reference |
| :--- | :--- | :--- |
| **Q64.64 Fixed-Point Math** | Software-defined 256-bit intermediate multiplication/division/square-root, checked negation protecting `i128::MIN`, swept capsule-vs-AABB continuous collision detection, and exact mathematical floor coordinate mapping. | [`math/fixed.rs`](aeviterra-core/src/math/fixed.rs) |
| **Sparse Merkle Tree** | 64-bit depth binary SMT with cryptographic domain separation between leaf (`0x00`) and internal (`0x01`) nodes, zero-allocation slice-partitioned root generation, and canonical inclusion/exclusion proofs. | [`state/smt.rs`](aeviterra-core/src/state/smt.rs) |
| **Shard BFT & Action Verification** | Pipelined HotStuff BFT with explicit committee admission control, duplicate-signer rejection in quorum certificates, WAL equivocation detection, and monotonic session sequence high-water marks. | [`consensus/bft.rs`](aeviterra-core/src/consensus/bft.rs), [`consensus/envelope.rs`](aeviterra-core/src/consensus/envelope.rs) |
| **Dynamic Halos & Hysteresis** | Velocity/acceleration swept halos ensuring boundary crossing safety across negative coordinate space, coupled with an asymmetric hysteresis controller (80% split / 20% merge / 300-tick stability window). | [`shard/halo.rs`](aeviterra-core/src/shard/halo.rs) |
| **Durable Cross-Shard 2PC** | Certified write-ahead journal transitions (`Commit`/`Abort`), strict entity ownership version checks, and idempotent retry application preventing asset duplication during boundary handoffs. | [`shard/cross_shard.rs`](aeviterra-core/src/shard/cross_shard.rs) |
| **Offline Leases & Grace Barrier** | Deterministic single-use nullifiers derived from lease tokens, single active lease enforcement per actor, zero-knowledge receipt verification, and a 200-tick Reclamation Grace Barrier eliminating expiry race conditions. | [`offline/lease.rs`](aeviterra-core/src/offline/lease.rs) |
| **Threshold Beacon RNG** | Irrevocable action-cost commitment prior to beacon emission, write-once round ingestion, and unbiased SHA-256 counter-mode rejection sampling over `[0, 1_000_000)`. | [`randomness/beacon.rs`](aeviterra-core/src/randomness/beacon.rs) |
| **Authenticated $\text{GF}(2^8)$ Erasure Coding** | Systematic MDS Cauchy Reed-Solomon coding over $\text{GF}(2^8)$ using Gauss-Jordan matrix inversion (reconstructs from *any* $K$ distinct source or repair fragments), symbol deduplication, and SHA-256 root integrity commitments. | [`storage/erasure.rs`](aeviterra-core/src/storage/erasure.rs) |
| **Wire Framing & Time-Locked Mempool** | Pre-allocation 64 KB frame ceiling, rejection of truncated and trailing bytes, and counter-mode authenticated time-locked commitments preventing front-running of combat intents. | [`network/framing.rs`](aeviterra-core/src/network/framing.rs) |
| **Thermodynamics & Demurrage** | Strict conservation of atomic micro-unit mass ledger (`input == output + residue`), bounded capability envelopes for AI/autonomous agents, and continuous demurrage decay on dormant sovereign vaults. | [`economy/thermodynamics.rs`](aeviterra-core/src/economy/thermodynamics.rs) |
| **Playable Combat & Economy** | Stamina-gated active combat with invulnerability frames and crowd-control diminishing returns, tier-isolated drop pity counters, single-use quest completion nullifiers, and 30-day municipal tenure governance. | [`game/`](aeviterra-core/src/game) |

---

## Verification & Test Suite

The workspace is rigorously verified with automated security invariant tests and end-to-end playability scenarios:

```bash
# Run all 25 unit, security, and gameplay integration tests
cargo test --workspace

# Run Clippy with zero warnings permitted
cargo clippy --workspace --all-targets -- -D warnings

# Launch the interactive Stage 0 Playable Simulation CLI
cargo run -p aeviterra-cli

# Run deterministic benchmark simulation (10,000 ticks)
cargo run -p aeviterra-cli -- --benchmark --ticks 10000
```

---

## Regenerating the Specification PDF

The publication-grade specification PDF is generated directly using Google Chrome in headless mode:

```bash
python3 generate_spec_pdf.py
```

This compiles `aeviterra_specification.html` and exports `Aeviterra-Specification.pdf`.

---

## License

Dual-licensed under either:
- MIT License ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.
