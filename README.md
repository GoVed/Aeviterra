# AEVITERRA (AETHERIA V4.1) — Century-Scale Decentralized MMO Engine

[![Rust 2024](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/Tests-25%2F25_Passing-brightgreen.svg)](#verification--test-suite)
[![Clippy](https://img.shields.io/badge/Clippy-Zero_Warnings-blue.svg)](#verification--test-suite)

**Aeviterra Protocol V4.1** is a deterministic, peer-operated MMORPG substrate and playable vertical-slice simulation engine engineered for century-scale persistence. It synthesizes and resolves five generations of adversarial architectural review:

1. **Gemini v1.0 (`Century Scale Decentralized MMO Architecture.pdf`)** — Introduced deterministic Q64.64 digital physics, procedural Genesis Seed terrain, and closed thermodynamic mass conservation, but contained 30 critical protocol flaws (`F01`–`F30`).
2. **ChatGPT Security Review R2 (`Century-Scale-MMO-Architecture-Reviewed.pdf`)** — Identified all 30 vulnerabilities (`F01`–`F30`) and established the 12-scenario adversarial verification matrix (`T01`–`T12`) and invariants (`I1`–`I8`).
3. **Antigravity V3.0 ([`AETHERIA_V3_UNEXPLOITABLE_SPECIFICATION.md`](file:///home/ved/Projects/AETHERIA/AETHERIA_V3_UNEXPLOITABLE_SPECIFICATION.md))** — Re-engineered sovereign consensus, leased state enclaves, dynamic swept halos, and semantic generative asset grammars.
4. **MiMo V4.0** — Added full classless MMO gameplay systems (9 weapon families, stamina-gated action combat, tier-isolated loot, settlements, quests) and uncovered four subtle V3 design flaws.
5. **Aeviterra V4.1 ([`AEVITERRA_V4_1_FINAL_SPECIFICATION.md`](file:///home/ved/Projects/AETHERIA/AEVITERRA_V4_1_FINAL_SPECIFICATION.md))** — Resolved all six remaining architectural and macroeconomic gaps (`G1`–`G6`) and hardened the Rust implementation against all 24 code-level edge cases identified during public-release code audit.

---

## Workspace Architecture

```text
AETHERIA/
├── Cargo.toml                                  # Rust Workspace (aeviterra-core, aeviterra-cli)
├── AEVITERRA_V4_1_FINAL_SPECIFICATION.md       # Normative V4.1 Production Specification
├── AETHERIA_V3_UNEXPLOITABLE_SPECIFICATION.md  # V3.0 Protocol Reference
├── aeviterra-core/                             # Normative Cryptographic & Physics Core
│   ├── src/
│   │   ├── math/fixed.rs                       # Software Q64.64 math, i256 intermediates, swept capsules
│   │   ├── state/smt.rs                        # 64-bit Sparse Merkle Tree with domain separation
│   │   ├── consensus/envelope.rs               # Domain-separated action envelopes, replay guards, certs
│   │   ├── consensus/bft.rs                    # HotStuff-variant Shard BFT with admitted committee & WAL
│   │   ├── shard/halo.rs                       # Dynamic swept-volume halos & split/merge hysteresis
│   │   ├── shard/cross_shard.rs                # Durable journal-certified 2PC & ownership versioning
│   │   ├── offline/lease.rs                    # Leased state enclaves, V4 nullifiers & G2 grace barrier
│   │   ├── randomness/beacon.rs                # Commit-before-entropy RNG with unbiased rejection sampling
│   │   ├── assets/grammar.rs                   # Pure semantic generative asset grammar tokens
│   │   ├── economy/thermodynamics.rs           # Conserved micro-unit mass ledger, agent BCE & G6 demurrage
│   │   ├── storage/erasure.rs                  # Authenticated MDS Cauchy Reed-Solomon coding over GF(2^8)
│   │   ├── network/framing.rs                  # 64 KB pre-allocation wire framing & time-locked mempool
│   │   ├── governance/fpeg.rs                  # Formalized Protocol Epoch Gates (FPEG) & PQ migration
│   │   └── game/{combat,loot,quest,settlement} # Stamina combat, CC DR, pity isolation, quests, tenure
│   └── tests/security_tests.rs                 # 19 adversarial security & invariant tests (T01–T12, G1–G6)
└── aeviterra-cli/                              # Stage 0 Playable Vertical Slice CLI & Simulation Loop
    ├── src/
    │   ├── terrain.rs                          # 32-byte Genesis Seed fBm noise & deterministic veins
    │   ├── simulation.rs                       # 20 TPS pure state transition loop & SMT root commitment
    │   └── main.rs                             # Interactive terminal MMO client & TPS benchmark runner
    └── tests/playability_tests.rs              # 6 end-to-end gameplay & determinism tests (P01–P06)
```

---

## Public Release Security & Code Hardening Summary

Every subsystem in `aeviterra-core` and `aeviterra-cli` has been audited and hardened against the R2 (`F01`–`F30`, `T01`–`T12`) and V4.1 (`G1`–`G6`) findings:

| Subsystem | Hardened Guarantee | Code Reference |
| :--- | :--- | :--- |
| **Q64.64 Fixed-Point Math (`F13, F14, T05`)** | Exact 256-bit intermediate mul/div/isqrt, checked `i128::MIN` negation (`checked_neg`), and exact mathematical floor (`to_int_floor`) for negative world coordinates. | [`math/fixed.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/math/fixed.rs) |
| **Sparse Merkle Tree (`F07, F13, T02`)** | Domain-separated leaf (`0x00`) and internal (`0x01`) nodes, zero-allocation slice-partitioned root computation, and canonical commitment of inventory & resource depletion. | [`state/smt.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/state/smt.rs) |
| **Shard BFT & Action Replay (`F01, F02, F17, F27, T01, T02`)** | Strict admitted-committee voter authorization (`UnauthorizedValidator`), duplicate-signer rejection in quorum certificates, WAL equivocation detection, and monotonic session sequence high-water marks (`ActionAdmissionVerifier`). | [`consensus/bft.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/consensus/bft.rs), [`consensus/envelope.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/consensus/envelope.rs) |
| **Dynamic Halos & Hysteresis (`F11, F23, T06`)** | Velocity/acceleration swept margins, exact floor chunk mapping across negative coordinates, and `ShardHysteresisController` (80% split / 20% merge / 300-tick minimum lifetime). | [`shard/halo.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/shard/halo.rs) |
| **Durable Cross-Shard 2PC (`F04, F12, G1, T03`)** | Certified journal decisions (`Commit`/`Abort`), strict entity ownership version checks (`VersionMismatch`), and idempotent retry application. | [`shard/cross_shard.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/shard/cross_shard.rs) |
| **Offline Leases & Grace Barrier (`F03, F18, G2, T04`)** | Single-use nullifiers derived strictly from `LeaseID` alone, single active lease per actor (`ActorAlreadyLeased`), and 200-tick Reclamation Grace Barrier ($\Delta_{\text{grace}}$) preventing expiry return races. | [`offline/lease.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/offline/lease.rs) |
| **Threshold Beacon RNG (`F05, F06, T07`)** | Irrevocable action cost commitment prior to beacon emission, write-once round ingestion, and unbiased SHA-256 counter-mode rejection sampling over `[0, 1_000_000)`. | [`randomness/beacon.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/randomness/beacon.rs) |
| **Authenticated $\text{GF}(2^8)$ Erasure Coding (`F07, F08, T11`)** | Systematic MDS Cauchy Reed-Solomon coding over $\text{GF}(2^8)$ with Gauss-Jordan elimination (reconstructs from *any* $K$ distinct source or repair symbols), symbol deduplication, and per-fragment SHA-256 commitments. | [`storage/erasure.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/storage/erasure.rs) |
| **Wire Framing & Time-Locked Mempool (`F16, F30, G3, T08, T09`)** | Pre-allocation 64 KB frame ceiling, truncated-header (`< 7B`) & trailing-byte rejection, and counter-mode authenticated time-locked combat intents. | [`network/framing.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/network/framing.rs) |
| **Thermodynamics & Demurrage (`F25, F26, G6, T05`)** | Exact micro-unit mass conservation (`input == output + residue`), AI agent `AgentCapabilityEnvelope`, and 180-day stagnant Sovereign vault demurrage (`SovereignDemurrageLedger`). | [`economy/thermodynamics.rs`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/economy/thermodynamics.rs) |
| **Playable MMO Mechanics (`G4, G5, P01–P06`)** | Stamina-gated combat with i-frames and crowd-control diminishing returns, tier-compartmentalized pity counters (`u64`-safe), single-use quest reward nullifiers, and 30-day staked municipal voting tenure. | [`game/`](file:///home/ved/Projects/AETHERIA/aeviterra-core/src/game) |

---

## Verification & Test Suite

Run the complete workspace test suite and strict linter checks:

```bash
# Run all 25 unit, security, and playability integration tests
cargo test --workspace

# Run Clippy with all warnings treated as errors
cargo clippy --workspace --all-targets -- -D warnings

# Launch the interactive Stage 0 Playable Vertical Slice CLI
cargo run -p aeviterra-cli
```
