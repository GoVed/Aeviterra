use super::framing::{WireFrame, PriorityQueue};
use std::collections::VecDeque;

pub const DEFAULT_TOTAL_QUANTUM: usize = 100_000; // 100 KB total per DRR cycle
pub const QUANTUM_CONSENSUS: usize = 40_000;      // 40%
pub const QUANTUM_CONTROL: usize   = 25_000;      // 25%
pub const QUANTUM_GAMEPLAY: usize  = 20_000;      // 20%
pub const QUANTUM_AUDIT: usize     = 10_000;      // 10%
pub const QUANTUM_RECOVERY: usize  =  5_000;      //  5% guaranteed floor

pub const MAX_CONSENSUS_QUEUE_LEN: usize = 1_000;
pub const MAX_CONTROL_QUEUE_LEN: usize = 500;
pub const MAX_GAMEPLAY_QUEUE_LEN: usize = 500;
pub const MAX_AUDIT_QUEUE_LEN: usize = 200;
pub const MAX_RECOVERY_QUEUE_LEN: usize = 100;
pub const MAX_CONSENSUS_QUEUE_LATENCY_TICKS: u64 = 2; // Preempt if consensus frame waits > 2 ticks
pub const MAX_CONSECUTIVE_PREEMPTIONS: u8 = 4; // Cap consecutive preemptions to prevent DRR starvation

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedFrame {
    pub frame: WireFrame,
    pub enqueued_tick: u64,
}

/// Deficit Round-Robin (DRR) Multi-Queue Priority Scheduler.
/// Solves bandwidth starvation across Consensus, Control, Gameplay, Audit, and Recovery queues
/// with guaranteed bandwidth floor for state catchup and bounded preemption for BFT consensus latency deadlines.
#[derive(Debug, Clone)]
pub struct MultiQueueScheduler {
    pub consensus_q: VecDeque<QueuedFrame>,
    pub control_q: VecDeque<QueuedFrame>,
    pub gameplay_q: VecDeque<QueuedFrame>,
    pub audit_q: VecDeque<QueuedFrame>,
    pub recovery_q: VecDeque<QueuedFrame>,

    // Deficit counters for each queue in bytes
    deficit_consensus: usize,
    deficit_control: usize,
    deficit_gameplay: usize,
    deficit_audit: usize,
    deficit_recovery: usize,

    current_queue_index: usize,
    consecutive_preemptions: u8,
}

impl Default for MultiQueueScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiQueueScheduler {
    pub fn new() -> Self {
        Self {
            consensus_q: VecDeque::new(),
            control_q: VecDeque::new(),
            gameplay_q: VecDeque::new(),
            audit_q: VecDeque::new(),
            recovery_q: VecDeque::new(),

            deficit_consensus: 0,
            deficit_control: 0,
            deficit_gameplay: 0,
            deficit_audit: 0,
            deficit_recovery: 0,

            current_queue_index: 0,
            consecutive_preemptions: 0,
        }
    }

    /// Enqueue a frame with bounded capacity and priority-specific drop policies.
    pub fn enqueue(&mut self, frame: WireFrame, current_tick: u64) {
        let q_frame = QueuedFrame {
            frame,
            enqueued_tick: current_tick,
        };

        match q_frame.frame.queue {
            PriorityQueue::Consensus => {
                // Bounded capacity to prevent spoofed consensus flood OOM
                if self.consensus_q.len() >= MAX_CONSENSUS_QUEUE_LEN {
                    self.consensus_q.pop_front();
                }
                self.consensus_q.push_back(q_frame);
            }
            PriorityQueue::Control => {
                if self.control_q.len() >= MAX_CONTROL_QUEUE_LEN {
                    self.control_q.pop_front();
                }
                self.control_q.push_back(q_frame);
            }
            PriorityQueue::Gameplay => {
                // Tail-drop oldest: loss-tolerant under client-side prediction
                if self.gameplay_q.len() >= MAX_GAMEPLAY_QUEUE_LEN {
                    self.gameplay_q.pop_front();
                }
                self.gameplay_q.push_back(q_frame);
            }
            PriorityQueue::Audit => {
                if self.audit_q.len() >= MAX_AUDIT_QUEUE_LEN {
                    self.audit_q.pop_front();
                }
                self.audit_q.push_back(q_frame);
            }
            PriorityQueue::Recovery => {
                if self.recovery_q.len() >= MAX_RECOVERY_QUEUE_LEN {
                    self.recovery_q.pop_front();
                }
                self.recovery_q.push_back(q_frame);
            }
        }
    }

    /// Dispatch next frame to transmit over the network socket.
    /// Checks for urgent consensus preemption first (bounded by `MAX_CONSECUTIVE_PREEMPTIONS`),
    /// then executes Deficit Round Robin across all priority queues.
    pub fn dispatch_next(&mut self, current_tick: u64) -> Option<QueuedFrame> {
        if self.is_empty() {
            self.consecutive_preemptions = 0;
            return None;
        }

        let has_non_consensus = !self.control_q.is_empty()
            || !self.gameplay_q.is_empty()
            || !self.audit_q.is_empty()
            || !self.recovery_q.is_empty();

        // 1. Bounded Preemption Override: if the head consensus frame has waited >= 2 ticks,
        // dispatch immediately up to MAX_CONSECUTIVE_PREEMPTIONS before yielding to DRR.
        if (!has_non_consensus || self.consecutive_preemptions < MAX_CONSECUTIVE_PREEMPTIONS)
            && let Some(head) = self.consensus_q.front()
            && current_tick.saturating_sub(head.enqueued_tick) >= MAX_CONSENSUS_QUEUE_LATENCY_TICKS
        {
            self.consecutive_preemptions = self.consecutive_preemptions.saturating_add(1);
            return self.consensus_q.pop_front();
        }

        if self.consecutive_preemptions >= MAX_CONSECUTIVE_PREEMPTIONS && has_non_consensus && self.current_queue_index == 0 {
            self.current_queue_index = 1;
        }
        self.consecutive_preemptions = 0;

        // 2. Deficit Round Robin across queues: 0=Consensus, 1=Control, 2=Gameplay, 3=Audit, 4=Recovery
        for _ in 0..5 {
            let served = match self.current_queue_index {
                0 => self.try_serve_queue(
                    PriorityQueue::Consensus,
                    QUANTUM_CONSENSUS,
                    |s| &mut s.consensus_q,
                    |s| &mut s.deficit_consensus,
                ),
                1 => self.try_serve_queue(
                    PriorityQueue::Control,
                    QUANTUM_CONTROL,
                    |s| &mut s.control_q,
                    |s| &mut s.deficit_control,
                ),
                2 => self.try_serve_queue(
                    PriorityQueue::Gameplay,
                    QUANTUM_GAMEPLAY,
                    |s| &mut s.gameplay_q,
                    |s| &mut s.deficit_gameplay,
                ),
                3 => self.try_serve_queue(
                    PriorityQueue::Audit,
                    QUANTUM_AUDIT,
                    |s| &mut s.audit_q,
                    |s| &mut s.deficit_audit,
                ),
                4 => self.try_serve_queue(
                    PriorityQueue::Recovery,
                    QUANTUM_RECOVERY,
                    |s| &mut s.recovery_q,
                    |s| &mut s.deficit_recovery,
                ),
                _ => None,
            };

            self.current_queue_index = (self.current_queue_index + 1) % 5;

            if let Some(frame) = served {
                return Some(frame);
            }
        }

        // If all deficit buckets exhausted in this cycle, replenish deficits and retry
        self.replenish_all_deficits();

        // Fallback: pop first available frame from highest priority non-empty queue
        if let Some(f) = self.consensus_q.pop_front() {
            return Some(f);
        }
        if let Some(f) = self.control_q.pop_front() {
            return Some(f);
        }
        if let Some(f) = self.gameplay_q.pop_front() {
            return Some(f);
        }
        if let Some(f) = self.audit_q.pop_front() {
            return Some(f);
        }
        if let Some(f) = self.recovery_q.pop_front() {
            return Some(f);
        }

        None
    }

    fn try_serve_queue<F, G>(
        &mut self,
        _q_type: PriorityQueue,
        quantum: usize,
        get_queue: F,
        get_deficit: G,
    ) -> Option<QueuedFrame>
    where
        F: Fn(&mut Self) -> &mut VecDeque<QueuedFrame>,
        G: Fn(&mut Self) -> &mut usize,
    {
        // Add quantum to deficit if queue has pending frames
        let q_len = get_queue(self).len();
        if q_len == 0 {
            *get_deficit(self) = 0;
            return None;
        }

        *get_deficit(self) = get_deficit(self).saturating_add(quantum);

        let head_size = get_queue(self).front().map(|qf| qf.frame.payload.len())?;
        if *get_deficit(self) >= head_size {
            *get_deficit(self) = get_deficit(self).saturating_sub(head_size);
            let frame = get_queue(self).pop_front();
            return frame;
        }

        None
    }

    fn replenish_all_deficits(&mut self) {
        if !self.consensus_q.is_empty() {
            self.deficit_consensus = self.deficit_consensus.saturating_add(QUANTUM_CONSENSUS);
        }
        if !self.control_q.is_empty() {
            self.deficit_control = self.deficit_control.saturating_add(QUANTUM_CONTROL);
        }
        if !self.gameplay_q.is_empty() {
            self.deficit_gameplay = self.deficit_gameplay.saturating_add(QUANTUM_GAMEPLAY);
        }
        if !self.audit_q.is_empty() {
            self.deficit_audit = self.deficit_audit.saturating_add(QUANTUM_AUDIT);
        }
        if !self.recovery_q.is_empty() {
            self.deficit_recovery = self.deficit_recovery.saturating_add(QUANTUM_RECOVERY);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.consensus_q.is_empty()
            && self.control_q.is_empty()
            && self.gameplay_q.is_empty()
            && self.audit_q.is_empty()
            && self.recovery_q.is_empty()
    }

    pub fn total_pending_frames(&self) -> usize {
        self.consensus_q.len()
            + self.control_q.len()
            + self.gameplay_q.len()
            + self.audit_q.len()
            + self.recovery_q.len()
    }
}
