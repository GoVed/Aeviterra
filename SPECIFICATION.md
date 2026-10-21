# Aeviterra Protocol: Architecture & Game Systems Specification
**A Century-Scale Decentralized MMO — Unified Production Specification (Revision 4)**

---

## 1. Executive Summary & Design Principles

The **Aeviterra Protocol** is a deterministic, peer-operated virtual world architecture engineered for continuous century-scale persistence without central servers or corporate operators. Aeviterra treats player agency, social depth, and engaging real-time action gameplay as first-class constraints, backed by an exploit-resistant cryptographic state replication substrate.

### Core Architectural Invariants

| Invariant | Name | Formal Definition |
| :--- | :--- | :--- |
| **I1** | **Authorized State Transition** | Only the normative deterministic state kernel $\mathcal{T}(S_{t}, \mathcal{B}_{t}, \mathcal{C}_{t}) \to S_{t+1}$ can mutate state. All actor signatures, session capabilities, object versions, and gas limits are validated prior to execution. |
| **I2** | **Unique Canonical Lineage** | For any shard $S$ and height $H$, no two honest nodes ever accept distinct state roots $R \neq R'$ under the BFT committee fault bound $f < n/3$. |
| **I3** | **Single Authority & Unique Ownership** | Every entity $E$ belongs to exactly one authoritative shard $S_{\text{auth}}$ at logical tick $t$. Spatial halos and remote shards maintain strictly read-only mirrors. |
| **I4** | **Durable Non-Blocking Transactions** | Cross-shard transactions execute via 2-Phase Commit certified by durable Control Journal decisions. Unilateral local timeout unlocks are prohibited; stranded locks cannot occur. |
| **I5** | **Exact Integer Mass Conservation** | The universe mass obeys $\sum M_{\text{active}} + \sum M_{\text{dormant}} + \sum M_{\text{sink}} = M_{\text{invariant}}$ in integer micro-units ($10^{-6}\text{g}$). Zero rounding leakage. |
| **I6** | **Single-Use Settlement Nullifiers** | An offline leased state enclave can settle at most once. Conflicting timeline proofs are rejected via consumed nullifiers $\mathcal{N} = H(\text{LeaseID} \parallel \text{"AEVITERRA_LEASE_NULLIFIER_V4_SETTLE"})$. |
| **I7** | **Commit-Before-Entropy Irrevocability** | Consequential probabilistic rolls require irrevocable action cost commitment before threshold beacon entropy is emitted. Selective abort is mathematically impossible. |
| **I8** | **Zero Arbitrary Media Carriage** | Consensus state contains zero polygonal 3D meshes or texture images. Assets exist purely as strongly typed mathematical generative grammar tokens. |

---

## PART I — PLAYABLE GAME DESIGN

### 2. World & Content Geography

- **Procedural-from-Seed, Authored-by-Canon**: Continental terrain, mountain ranges, river networks, and mineral distributions are generated locally by each node from an immutable 32-byte Genesis Seed via deterministic fixed-point noise functions, requiring zero persistent disk storage for geography. Historic landmarks, player keeps, bridges, and monuments are established by *Canon Records*—cryptographically signed entries in the append-only ledger.
- **Finite, Bounded Active Surface**: The explorable surface is capped at a $4,096 \times 4,096$ chunk grid (~524,000 square kilometers, a continent-scale world). Unbounded procedural expansion is prohibited to preserve player density, territorial politics, and closed-economy mass conservation. Expansion occurs exclusively via scheduled world event epochs.
- **Ecological Biomes**:
  - *Temperate Heartlands*: Safe zones, beginner settlements, agrarian plots, and starter craft workshops. Protected from open PvP.
  - *Frontier Wilds*: Rich mineral and lumber deposits, player-founded settlements, and flag-based opt-in PvP.
  - *Deep Wilds*: High-risk, high-yield territory. Open PvP with full-loot on carried currency and raw materials (equipped soulbound gear is never dropped).
  - *The Underdeep*: Procedural subterranean labyrinths containing eldritch materials, high-tier crafting reagents, and raid bosses.
- **Deterministic Seasons**: Seasons advance on the global tick clock. Winter freezes river crossings and slows siege engines; spring accelerates agricultural yields and mob migration.

### 3. Character Systems & Classless Buildcraft

- **Nine Weapon & Ability Families**: Rather than rigid character classes, players equip a Primary and Secondary weapon family from:
  1. *Blade* (slashing, parries, counter-attacks)
  2. *Spear* (reach, piercing thrusts, spacing)
  3. *Bow* (ranged physical attacks, kiting)
  4. *Wand/Prism* (arcane burst, kinetic deflections)
  5. *Staff* (elemental area control, hazard fields)
  6. *Shield* (poise, directional blocking, shield bashes)
  7. *Shadow* (mobility, stealth, critical strikes)
  8. *Song* (auras, party tempo buffs, inspiration)
  9. *Tinker* (deployable turrets, traps, siegecraft, alchemy)
- **Core Attributes**: Might (physical power, carry capacity), Finesse (attack cadence, critical chance), Resolve (stamina pool, blocking stability), Wit (ability efficiency, trap crafting), and Spirit (restoration, aura potency). Attributes derive from worn equipment and training allocations; respec is low-cost to encourage build experimentation.
- **Action Combat Mechanics**:
  - *Local Responsiveness*: Client prediction renders instant swings, blocks, and rolls. Server/shard consensus evaluates authoritative hitboxes. Rollback is display-only: finalized combat outcomes are never rewritten.
  - *Stamina-Gated Movesets*: Abilities have no arbitrary cooldown timers; all actions consume stamina. A skilled player balances stamina, i-frame dodge rolls (10 ticks / 200ms invulnerability), and directional blocking (mitigating 70% damage).
  - *Crowd-Control Diminishing Returns*: Stun, freeze, and stagger effects suffer strict diminishing returns (100% $\to$ 50% $\to$ 25% $\to$ 0% duration / immunity), preventing stun-lock degeneracy.
- **Cosmetic Prestige Currency**: Titles, cloaks, heraldic crests, and masterwork weapon glows represent player prestige. Prestige confers zero combat stats and cannot be weaponized.

### 4. Economy, Crafting & Settlements

- **Dual-Currency System**:
  - *Marks (Soft Currency)*: Minted by mob bounties and beginner quests; drained by gear repairs, municipal transport taxes, and food upkeep.
  - *Sovereigns (Hard Currency)*: Minted strictly through endgame territory conquest, high-tier guild caravans, and seasonal events. Used for keep upgrades and grand auction houses.
  - *Zero NPC Infinite Gold*: Every coin in circulation originates from an explicit, monitored faucet and leaves through certified sinks. Total currency supply is tracked per epoch.
  - *Liquidity Demurrage*: Stagnant Sovereign vaults uncirculated for >180 days incur an algorithmic 5% demurrage decay redirected into regional public infrastructure funds, preventing cartel hoarding.
- **Crafting Centrality**: Best-in-slot gear is craftable by player artisans, not dropped exclusively by raids. Raid bosses drop unique crafting catalysts and cosmetic modifiers, ensuring crafters remain central to the world economy.
- **Homesteads & Settlement Governance**: Housing is a guaranteed right. Every player can claim a homestead plot in a chartered settlement and construct buildings from typed semantic structural primitives. Settlements elect mayors, levy local trade taxes, and construct public defenses. Mayoral voting requires proven economic tenure (30 days residency + tax bond) to prevent Sybil bot takeovers.

---

## PART II — SUBSTRATE & SECURITY ARCHITECTURE

### 5. Consensus, Validator Federation & Shuffled Committees

1. **Bonded Operator Federation**: Consensus is operated by an admitted federation of 30–100 independent operators across diverse bare-metal and cloud providers posting slashed storage bonds.
2. **Verifiable Shard Committee Shuffling**: For spatial chunk $C = (x, y, z)$ and epoch $E$, the authoritative consensus committee of size $n = 3f + 1$ is sampled deterministically using the BLS threshold beacon seed:
   $$\text{Committee}(C, E) = \text{Sample}(\text{ValidatorPool}, H(\sigma_E \parallel C))$$
   Players inside chunk $C$ never vote on their own actions.
3. **HotStuff-Variant BFT with Write-Ahead Logging (WAL)**: Consensus executes Prepare, Pre-commit, and Commit phases. Every commit certificate requires $\ge 2f + 1$ distinct validator signatures. Validators persist votes to disk before broadcasting; duplicate votes at the same height/view trigger instant equivocation slashing proofs.

### 6. Leased State Enclaves & Single-Use Settlement Nullifiers

1. **State Lease Checkout**: An avatar entering unobserved wilderness checks out an exclusive lease from the shard committee:
   $$\text{LeaseID} = H(\text{ActorID} \parallel R_{\text{start}} \parallel \text{Tick}_{\text{grant}} \parallel \text{Salt})$$
   The committee locks the avatar's state in the Sparse Merkle Tree, assigning a tick horizon $T_{\text{max}}$, reclamation grace barrier $\Delta_{\text{grace}}$, and network threshold entropy seed $S_{\text{entropy}}$.
2. **Deterministic Offline Execution**: The client executes the Wasm/zkVM kernel locally. Random rolls derive from $H(S_{\text{entropy}} \parallel \text{step})$. Inputs form an unbroken hash chain:
   $$C_0 = \text{LeaseID}, \quad C_i = H(C_{i-1} \parallel \text{Action}_i)$$
3. **Single-Use Nullifier**: The settlement nullifier is derived strictly from `LeaseID` alone:
   $$\mathcal{N} = H(\text{LeaseID} \parallel \text{"AEVITERRA_LEASE_NULLIFIER_V4_SETTLE"})$$
   Conflicting timeline branches produce the exact same nullifier; the first timeline burns it and any subsequent attempt is permanently rejected.
4. **Reclamation Grace Barrier**: A lease cannot be reclaimed as expired until tick $T_{\text{max}} + \Delta_{\text{grace}}$. If a valid settlement proof arrives during the grace window, settlement strictly supersedes expiration.

### 7. Dynamic Swept-Volume Halos & Durable 2PC

1. **Dynamic Swept Margin**: Halo bounds adapt dynamically to velocity, acceleration, and rollback horizons:
   $$R_{\text{halo}} = R_{\text{interact}} + v_{\text{max}} \cdot \Delta t_{\text{rollback}} + \frac{1}{2} a_{\text{max}} \cdot (\Delta t_{\text{rollback}})^2 + \epsilon_{\text{numerical}}$$
   Continuous collision detection evaluates swept capsules $AABB(t_0 \to t_1)$ covering all intersecting chunks.
2. **Octree Split/Merge Hysteresis**: Shard capacity is governed by an 80% split threshold, 20% merge threshold, and a 300-tick minimum lifetime barrier, preventing boundary oscillation churn.
3. **Durable Journal Decisions**: Unilateral local timeout unlocks are completely prohibited. Transactions declare write-sets and persist prepare locks. Transactions resolve strictly through a consensus-certified `CertifiedJournalDecision` (`Commit` or `Abort`) recorded in the Control Journal.

### 8. Commit-Before-Entropy Randomness Pipeline

1. **Irrevocable Commitment (Tick $T$)**: Action intent is committed and costs (stamina, ammo, durability) are debited immediately. An assigned beacon round $R_{\text{target}} = R_{\text{current}} + \Delta R$ is locked.
2. **Deterministic Threshold Resolution**: At round $R_{\text{target}}$, the committee emits the threshold BLS signature $\sigma_R$. The outcome is resolved via unbiased counter-mode rejection sampling:
   $$E_{\text{event}} = H(\text{"AEVITERRA_EVENT_RNG_V1"} \parallel \text{action_id} \parallel R_{\text{target}} \parallel \sigma_R)$$
   Because costs were debited at Step 1, player disconnects do not pause resolution. Selective abort is mathematically impossible.

### 9. Pure Semantic Generative Asset Grammar

Consensus state contains zero binary meshes or texture images. Assets exist purely as strongly typed mathematical tokens:
- `StructuralPrimitive { style, primary_material, secondary_material, length, height, thickness, features }` (max 48 bytes)
- `HeraldryToken { primary_field, secondary_field, charge, charge_color, division_style }` (8 bytes)
- `EquipmentGrammar { item_class_id, primary_material, secondary_material, balance_point, mass_grams, heraldry }` (32 bytes)

Clients render photorealistic geometry procedurally from verified grammar rules. Steganographic carrier capacity is reduced to zero, state footprint is compressed by 99.99%, and node operators bear zero media liability.

### 10. Strict Q64.64 Deterministic Numerics

- Software-emulated Q64.64 fixed-point arithmetic (`i128` raw integer).
- Multiplication computes an exact signed `i256` intermediate product, truncated toward zero.
- Division computes exact 256-bit binary long division with checked division-by-zero handling.
- Square root uses exact 256-bit integer square root: $\lfloor \sqrt{X \cdot 2^{64}} \rfloor$.
- Mathematical floor conversion (`to_int_floor`) ensures negative world coordinates map to exact chunk lattice indices.

### 11. Authenticated MDS Cauchy Reed-Solomon Coding over $\text{GF}(2^8)$

- Data snapshots are encoded into $K$ systematic source symbols and $R$ Cauchy repair symbols over $\text{GF}(2^8)$ (irreducible polynomial $x^8 + x^4 + x^3 + x + 1$).
- Reconstruction succeeds from *any* $K$ distinct valid symbols (source or repair) via Gauss-Jordan elimination.
- Manifests fix object hash, length, symbol size, and per-symbol SHA-256 commitments, rejecting duplicate or tampered symbols prior to decode.

### 12. P2P Networking, Streaming Framing & Multi-Queue Scheduling

- **Streaming Wire Codec**: Enforces a 64 KB pre-allocation frame ceiling, verified 7-byte header (`AEVI` magic, version, priority queue), 4-byte transport integrity checksum, and bounded accumulator buffers to prevent slowloris holding.
- **Mutual Handshake & Eclipse Defense**: Challenge-response mutual authentication binding `world_id`, session nonces, and `NodeId` with $\le 30\text{s}$ timestamp drift enforcement; address books partition peers into `/16` subnets with an outbound diversity constraint of $\ge 6$ distinct subnets and reserved validator seats.
- **Deficit Round-Robin Multi-Queue Scheduler**: Guarantees bandwidth allocations across 5 priority queues (`Consensus` 40%, `Control` 25%, `Gameplay` 20%, `Audit` 10%, `Recovery` 5% guaranteed floor), eliminating state catchup deadlocks while preempting immediately for BFT consensus latency deadlines.
- **Dynamic Swept-Halo GossipSub**: Topic-partitioned gossip (`aeviterra/consensus`, `aeviterra/shard`, `aeviterra/halo/{x}_{z}`) with `IHAVE`/`IWANT` metadata exchange for payloads $>1\text{ KB}$ and two-tier deduplication caches defeating broadcast loops.
- **Peer Reputation Ledger**: Multi-factor scoring ($S \in [-10000, +1000]$) with exponential half-life decay, progressive throttling/quarantine, and instant permaban with escalating subnet bans for equivocation.
- **Time-Locked Mempool**: Combat action intents are sealed under a counter-mode threshold keystream bound to `(threshold_key, target_block_height, actor_id, session_epoch)` with cryptographic commitment verification, eliminating front-running and MEV.

### 13. Formalized Protocol Epoch Gates (FPEG)

Protocol upgrades occur through scheduled epoch migration proposals with supermajority committee certification. Physics invariants remain mathematically frozen; execution runtimes and cryptographic suites upgrade via epoch transition certificates supporting post-quantum ML-DSA / Blake3 and recursive SNARK proofs.

---

## PART III — VERIFICATION & IMPLEMENTATION

### 14. Reference Implementation & Test Suite

The normative core engine and reference simulation are implemented across two workspace crates:
- `aeviterra-core`: Cryptographic primitives, BFT consensus, SMT state, dynamic halos, 2PC, offline leases, erasure coding, and thermodynamic mass ledgers.
- `aeviterra-cli`: Genesis seed terrain generator, 20 TPS deterministic simulation loop, terminal client, and performance benchmarks.

```
$ cargo test --workspace
running 19 tests (aeviterra-core/tests/security_tests.rs)
test test_adversarial_hardening_all_review_gaps ... ok
test test_bft_quorum_and_equivocation_detection ... ok
test test_combat_movesets_and_iframes ... ok
test test_commit_before_entropy_eliminates_selective_abort ... ok
test test_cross_shard_durable_journal_decision ... ok
test test_dynamic_swept_halo_calculation ... ok
test test_municipal_governance_anti_sybil_tenure_check ... ok
test test_q64_deterministic_arithmetic ... ok
test test_quest_claim_idempotency_and_replay_protection ... ok
test test_semantic_generative_asset_grammar ... ok
test test_smt_domain_separation_and_verification ... ok
test test_t01_partition_tolerance_and_sybil_isolation ... ok
test test_t08_bounded_wire_framing_and_allocation_bombs ... ok
test test_t09_time_locked_mempool_anti_front_running ... ok
test test_t11_authenticated_erasure_coding_recovery ... ok
test test_t12_fpeg_epoch_gates_and_crypto_agility ... ok
test test_thermodynamic_mass_conservation_and_agent_budget ... ok
test test_tier_compartmentalized_pity_system ... ok
test test_v4_nullifier_immune_to_differing_end_roots ... ok

running 7 tests (aeviterra-core/tests/p2p_network_tests.rs)
test test_address_book_subnet_diversity_and_eclipse_defense ... ok
test test_gossip_pubsub_deduplication_and_ihave_iwant ... ok
test test_multi_queue_scheduler_anti_starvation_and_preemption ... ok
test test_peer_handshake_mutual_auth_and_replay ... ok
test test_peer_reputation_quarantine_and_ban ... ok
test test_simulated_network_latency_partition_and_heal ... ok
test test_streaming_codec_fragmentation_and_checksum ... ok

running 6 tests (aeviterra-cli/tests/playability_tests.rs)
test test_p01_combat_balance_ttk_stamina_and_iframes ... ok
test test_p02_gathering_depletion_and_damascus_forging ... ok
test test_p03_ai_agent_pursuit_and_capabilities ... ok
test test_p04_settlement_plot_claiming_and_modular_construction ... ok
test test_p05_idempotent_quests_and_consumed_once_reward_claims ... ok
test test_p06_high_throughput_deterministic_simulation_pacing ... ok

test result: ok. 32 passed; 0 failed; 0 ignored; finished in 0.72s
```

---

## 15. Staged Implementation Roadmap

| Stage | Milestone | Deliverables & Acceptance Gates |
| :--- | :--- | :--- |
| **Stage 1** | **Normative Core Engine** | Pinned Q64.64 math, SMT with domain separation, typed ECS schemas, deterministic Wasm runner with float stripper, cross-architecture golden state roots. *Gate: Bit-exact across x86-64, ARM64, RISC-V.* |
| **Stage 2** | **Certified Network & Federation** | HotStuff-variant BFT with durable WAL, federation admission flow, checkpoint lineage, discovery/relays as untrusted routing, fault injection. *Gate: T01, T02.* |
| **Stage 3** | **Shards & Transactions** | Dynamic swept halos, hierarchical partition-proof 2PC, journal-certified decisions, hysteresis split/merge. *Gate: T03, T06.* |
| **Stage 4** | **Game Systems at Scale** | Dual-currency ledgers, shard-replicated auction house, caravans, guilds, territory sieges, housing, AI agent capability envelopes. *Gate: T05, T10 + economic baselines.* |
| **Stage 5** | **Storage & Operations** | RaptorQ / Cauchy authenticated coding, 13-replica placement, retrieval audits, disaster recovery drills, reproducible builds with SBOM. *Gate: T11.* |
| **Stage 6** | **Gated Extensions** | Leased offline wilderness exploration, threshold-random competitive PvP seasons, progressive validator seat opening. |

---

## 16. Protocol Architecture Verdict

The Aeviterra Protocol (Revision 4) establishes a complete, sovereign, classless MMORPG running atop an unexploitable, peer-operated deterministic execution substrate. Physical conservation laws are exact, cheating is rendered mathematically irrational and detectable, and century-scale persistence is realized through disciplined cryptographic engineering.
