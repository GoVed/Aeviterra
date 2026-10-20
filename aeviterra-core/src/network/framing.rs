use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};

pub const MAX_PACKET_SIZE: usize = 65_536; // 64 KB strict framing limit (F16, T08)
pub const MIN_FRAME_HEADER_SIZE: usize = 7; // 4B magic + 2B version + 1B queue
pub const MAGIC_HEADER: &[u8; 4] = b"AEVI";
pub const CURRENT_WIRE_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriorityQueue {
    Consensus = 0,
    Control = 1,
    Gameplay = 2,
    Audit = 3,
    Recovery = 4,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFrame {
    pub magic: [u8; 4],
    pub version: u16,
    pub queue: PriorityQueue,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramingError {
    BufferTooShort,
    FrameTooLarge(usize),
    TrailingBytes(usize),
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidQueue,
    MalformedPayload,
    CommitmentMismatch,
}

/// Bounded Wire Framing Parser (V4.1 §17, F16, T08).
/// Enforces strict length limits prior to heap allocation to prevent decompression bombs,
/// rejects truncated frames (< 7B header), and rejects trailing bytes (R2 §11).
pub struct WireParser;

impl WireParser {
    pub fn parse_frame(buffer: &[u8]) -> Result<WireFrame, FramingError> {
        if buffer.len() < 4 {
            return Err(FramingError::BufferTooShort);
        }

        // 1. Check length prefix before allocation
        let length_prefix = u32::from_be_bytes(buffer[0..4].try_into().unwrap()) as usize;
        if length_prefix > MAX_PACKET_SIZE {
            return Err(FramingError::FrameTooLarge(length_prefix));
        }
        if length_prefix < MIN_FRAME_HEADER_SIZE {
            return Err(FramingError::MalformedPayload);
        }

        if buffer.len() < 4 + length_prefix {
            return Err(FramingError::BufferTooShort);
        }
        if buffer.len() > 4 + length_prefix {
            return Err(FramingError::TrailingBytes(buffer.len() - (4 + length_prefix)));
        }

        let frame_slice = &buffer[4..4 + length_prefix];

        // 2. Validate magic bytes
        if &frame_slice[0..4] != MAGIC_HEADER {
            return Err(FramingError::InvalidMagic);
        }

        // 3. Validate wire version
        let version = u16::from_be_bytes(frame_slice[4..6].try_into().unwrap());
        if version != CURRENT_WIRE_VERSION {
            return Err(FramingError::UnsupportedVersion(version));
        }

        // 4. Validate priority channel
        let queue_byte = frame_slice[6];
        let queue = match queue_byte {
            0 => PriorityQueue::Consensus,
            1 => PriorityQueue::Control,
            2 => PriorityQueue::Gameplay,
            3 => PriorityQueue::Audit,
            4 => PriorityQueue::Recovery,
            _ => return Err(FramingError::InvalidQueue),
        };

        let payload = frame_slice[MIN_FRAME_HEADER_SIZE..].to_vec();

        Ok(WireFrame {
            magic: *MAGIC_HEADER,
            version,
            queue,
            payload,
        })
    }

    /// Serialize a frame with explicit 32-bit length prefix.
    pub fn serialize_frame(frame: &WireFrame) -> Result<Vec<u8>, FramingError> {
        if frame.magic != *MAGIC_HEADER {
            return Err(FramingError::InvalidMagic);
        }
        if frame.version != CURRENT_WIRE_VERSION {
            return Err(FramingError::UnsupportedVersion(frame.version));
        }
        let frame_len = MIN_FRAME_HEADER_SIZE.saturating_add(frame.payload.len());
        if frame_len > MAX_PACKET_SIZE {
            return Err(FramingError::FrameTooLarge(frame_len));
        }

        let mut out = Vec::with_capacity(4 + frame_len);
        out.extend_from_slice(&(frame_len as u32).to_be_bytes());
        out.extend_from_slice(&frame.magic);
        out.extend_from_slice(&frame.version.to_be_bytes());
        out.push(frame.queue as u8);
        out.extend_from_slice(&frame.payload);

        Ok(out)
    }
}

/// Threshold Time-Locked Mempool Intent Envelope (V4.1 §4 G3, F24, F30, T07, T09).
/// Prevents combat front-running / MEV by sealing intent contents under counter-mode
/// threshold keystream bound to (actor_id, session_epoch, target_block_height).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeLockedIntent {
    pub actor_id: u64,
    pub session_epoch: u64,
    pub target_block_height: u64,
    pub encrypted_payload: Vec<u8>,
    pub commitment_hash: [u8; 32],
}

impl TimeLockedIntent {
    fn xor_keystream(
        actor_id: u64,
        epoch: u64,
        target_height: u64,
        threshold_key: &[u8; 32],
        input: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for (block_idx, chunk) in input.chunks(32).enumerate() {
            let mut hasher = Sha256::new();
            hasher.update(b"AEVITERRA_TIME_LOCK_KEY_V2");
            hasher.update(threshold_key);
            hasher.update(target_height.to_be_bytes());
            hasher.update(actor_id.to_be_bytes());
            hasher.update(epoch.to_be_bytes());
            hasher.update((block_idx as u64).to_be_bytes());
            let stream_block: [u8; 32] = hasher.finalize().into();

            for (i, &byte) in chunk.iter().enumerate() {
                out.push(byte ^ stream_block[i]);
            }
        }
        out
    }

    pub fn compute_commitment(
        actor_id: u64,
        session_epoch: u64,
        target_block_height: u64,
        encrypted_payload: &[u8],
    ) -> [u8; 32] {
        let mut commit_hasher = Sha256::new();
        commit_hasher.update(b"AEVITERRA_INTENT_COMMITMENT_V2");
        commit_hasher.update(actor_id.to_be_bytes());
        commit_hasher.update(session_epoch.to_be_bytes());
        commit_hasher.update(target_block_height.to_be_bytes());
        commit_hasher.update((encrypted_payload.len() as u32).to_be_bytes());
        commit_hasher.update(encrypted_payload);
        commit_hasher.finalize().into()
    }

    pub fn new(
        actor_id: u64,
        epoch: u64,
        target_height: u64,
        plaintext_payload: &[u8],
        threshold_key: &[u8; 32],
    ) -> Self {
        let encrypted = Self::xor_keystream(actor_id, epoch, target_height, threshold_key, plaintext_payload);
        let commitment_hash = Self::compute_commitment(actor_id, epoch, target_height, &encrypted);

        TimeLockedIntent {
            actor_id,
            session_epoch: epoch,
            target_block_height: target_height,
            encrypted_payload: encrypted,
            commitment_hash,
        }
    }

    pub fn verify_commitment(&self) -> bool {
        let expected = Self::compute_commitment(
            self.actor_id,
            self.session_epoch,
            self.target_block_height,
            &self.encrypted_payload,
        );
        self.commitment_hash == expected
    }

    pub fn verify_and_decrypt(&self, threshold_key: &[u8; 32]) -> Result<Vec<u8>, FramingError> {
        if !self.verify_commitment() {
            return Err(FramingError::CommitmentMismatch);
        }
        Ok(Self::xor_keystream(
            self.actor_id,
            self.session_epoch,
            self.target_block_height,
            threshold_key,
            &self.encrypted_payload,
        ))
    }

    pub fn decrypt(&self, threshold_key: &[u8; 32]) -> Vec<u8> {
        Self::xor_keystream(
            self.actor_id,
            self.session_epoch,
            self.target_block_height,
            threshold_key,
            &self.encrypted_payload,
        )
    }
}
