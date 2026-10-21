use super::peer::NodeId;
use super::framing::WireFrame;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledPacket {
    pub delivery_tick: u64,
    pub sender: NodeId,
    pub receiver: NodeId,
    pub frame: WireFrame,
}

impl Ord for ScheduledPacket {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap by delivery_tick
        other.delivery_tick.cmp(&self.delivery_tick)
    }
}

impl PartialOrd for ScheduledPacket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Deterministic Discrete-Event P2P Network Simulator.
/// Simulates packet latency, jitter, stochastic drops, and asymmetric partitions
/// driven strictly by virtual logical ticks (V4.1 §17).
#[derive(Debug, Clone)]
pub struct SimulatedNetwork {
    pub current_tick: u64,
    pub in_flight: BinaryHeap<ScheduledPacket>,
    /// Blocked directional communication edges (sender, receiver)
    pub partitions: BTreeSet<(NodeId, NodeId)>,
    pub base_latency_ticks: u64,
    pub packet_drop_numerator: u32,   // e.g. 5 for 5%
    pub packet_drop_denominator: u32, // e.g. 100
    rng_state: u64,
}

impl SimulatedNetwork {
    pub fn new(base_latency_ticks: u64, rng_seed: u64) -> Self {
        Self {
            current_tick: 0,
            in_flight: BinaryHeap::new(),
            partitions: BTreeSet::new(),
            base_latency_ticks,
            packet_drop_numerator: 0,
            packet_drop_denominator: 100,
            rng_state: rng_seed ^ 0xAE01_C0DE_5EED_0001,
        }
    }

    pub fn set_packet_drop_rate(&mut self, numerator: u32, denominator: u32) {
        assert!(denominator > 0);
        self.packet_drop_numerator = numerator;
        self.packet_drop_denominator = denominator;
    }

    /// Linear congruential PRNG step for deterministic simulation.
    fn next_rand(&mut self) -> u64 {
        self.rng_state = self.rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.rng_state
    }

    /// Schedule a wire frame transmission between peers.
    pub fn send_packet(
        &mut self,
        sender: NodeId,
        receiver: NodeId,
        frame: WireFrame,
    ) {
        // 1. Check partition drop
        if self.partitions.contains(&(sender, receiver)) {
            return;
        }

        // 2. Check stochastic packet drop
        if self.packet_drop_numerator > 0 {
            let roll = (self.next_rand() % (self.packet_drop_denominator as u64)) as u32;
            if roll < self.packet_drop_numerator {
                return; // Dropped by simulated lossy channel
            }
        }

        // 3. Jitter calculation: 0, 1, or 2 extra ticks
        let jitter = self.next_rand() % 3;
        let delivery_tick = self.current_tick
            .saturating_add(self.base_latency_ticks)
            .saturating_add(jitter);

        self.in_flight.push(ScheduledPacket {
            delivery_tick,
            sender,
            receiver,
            frame,
        });
    }

    /// Advance simulation clock and pop all packets due at or before the current tick.
    pub fn step(&mut self) -> Vec<(NodeId, NodeId, WireFrame)> {
        self.current_tick = self.current_tick.saturating_add(1);
        let mut delivered = Vec::new();

        while let Some(top) = self.in_flight.peek() {
            if top.delivery_tick <= self.current_tick {
                let packet = self.in_flight.pop().unwrap();
                // Check if partition was created after scheduling
                if !self.partitions.contains(&(packet.sender, packet.receiver)) {
                    delivered.push((packet.sender, packet.receiver, packet.frame));
                }
            } else {
                break;
            }
        }

        delivered
    }

    /// Inject an asymmetric network partition between two nodes.
    pub fn add_partition(&mut self, sender: NodeId, receiver: NodeId) {
        self.partitions.insert((sender, receiver));
    }

    /// Remove an asymmetric partition.
    pub fn heal_partition(&mut self, sender: NodeId, receiver: NodeId) {
        self.partitions.remove(&(sender, receiver));
    }

    /// Bidirectional partition between two network subsets.
    pub fn partition_subsets(&mut self, set_a: &[NodeId], set_b: &[NodeId]) {
        for &a in set_a {
            for &b in set_b {
                self.partitions.insert((a, b));
                self.partitions.insert((b, a));
            }
        }
    }

    /// Heal all network partitions across the entire network.
    pub fn heal_all_partitions(&mut self) {
        self.partitions.clear();
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}
