use super::framing::{WireFrame, PriorityQueue, FramingError, MAX_PACKET_SIZE, MAGIC_HEADER, CURRENT_WIRE_VERSION};
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};

/// Multiplexed frame type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FrameKind {
    Handshake = 0,
    Gossip = 1,
    DirectRpc = 2,
    Ping = 3,
    Pong = 4,
}

impl FrameKind {
    pub fn from_u8(val: u8) -> Result<Self, FramingError> {
        match val {
            0 => Ok(FrameKind::Handshake),
            1 => Ok(FrameKind::Gossip),
            2 => Ok(FrameKind::DirectRpc),
            3 => Ok(FrameKind::Ping),
            4 => Ok(FrameKind::Pong),
            _ => Err(FramingError::MalformedPayload),
        }
    }
}

/// Compute 4-byte transport integrity checksum from SHA-256 digest.
pub fn compute_transport_checksum(header_and_payload: &[u8]) -> [u8; 4] {
    let mut hasher = Sha256::new();
    hasher.update(b"AEVI_FRAME_INTEGRITY_V1");
    hasher.update(header_and_payload);
    let digest: [u8; 32] = hasher.finalize().into();
    [digest[0], digest[1], digest[2], digest[3]]
}

/// Streaming Frame Codec for TCP/Stream Sockets.
/// Handles byte chunk fragmentation, coalesced packets, pre-allocation length bounds,
/// and slowloris buffer holding mitigation (V4.1 §17, F16, T08).
#[derive(Debug, Clone)]
pub struct StreamingFrameCodec {
    buffer: Vec<u8>,
    max_packet_size: usize,
    max_buffer_capacity: usize,
}

impl Default for StreamingFrameCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingFrameCodec {
    pub const CHECKSUM_SIZE: usize = 4;
    pub const HEADER_LEN: usize = 7; // 4B magic + 2B version + 1B queue

    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(MAX_PACKET_SIZE + 12),
            max_packet_size: MAX_PACKET_SIZE,
            max_buffer_capacity: MAX_PACKET_SIZE * 2,
        }
    }

    /// Feed incoming raw byte stream into the accumulator buffer.
    /// Strictly limits accumulated unparsed buffer to prevent slowloris memory holding.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), FramingError> {
        if self.buffer.len().saturating_add(chunk.len()) > self.max_buffer_capacity {
            return Err(FramingError::FrameTooLarge(self.buffer.len() + chunk.len()));
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Attempt to parse and pop the next available frame from the accumulator.
    /// Returns Ok(Some(frame)) if a complete frame was parsed, Ok(None) if more bytes are needed.
    pub fn pop_frame(&mut self) -> Result<Option<WireFrame>, FramingError> {
        // Must have at least 4 bytes for length prefix
        if self.buffer.len() < 4 {
            return Ok(None);
        }

        let length_prefix = u32::from_be_bytes(self.buffer[0..4].try_into().unwrap()) as usize;
        
        // 1. Guard length prefix against allocation bomb (drain bad prefix to prevent head-of-line blocking)
        if length_prefix > self.max_packet_size {
            self.buffer.drain(0..4);
            return Err(FramingError::FrameTooLarge(length_prefix));
        }
        if length_prefix < Self::HEADER_LEN + Self::CHECKSUM_SIZE {
            let drain_len = (4 + length_prefix).min(self.buffer.len());
            self.buffer.drain(0..drain_len);
            return Err(FramingError::MalformedPayload);
        }

        let total_frame_len = 4 + length_prefix;
        if self.buffer.len() < total_frame_len {
            // Incomplete frame, wait for more data from socket
            return Ok(None);
        }

        // 2. Drain complete frame slice upfront so any header/checksum failure never poisons pipelined frames
        let frame_vec: Vec<u8> = self.buffer.drain(0..total_frame_len).collect();
        let frame_slice = &frame_vec[4..total_frame_len];

        // 3. Validate magic
        if &frame_slice[0..4] != MAGIC_HEADER {
            return Err(FramingError::InvalidMagic);
        }

        // 4. Validate version
        let version = u16::from_be_bytes(frame_slice[4..6].try_into().unwrap());
        if version != CURRENT_WIRE_VERSION {
            return Err(FramingError::UnsupportedVersion(version));
        }

        // 5. Validate queue
        let queue_byte = frame_slice[6];
        let queue = match queue_byte {
            0 => PriorityQueue::Consensus,
            1 => PriorityQueue::Control,
            2 => PriorityQueue::Gameplay,
            3 => PriorityQueue::Audit,
            4 => PriorityQueue::Recovery,
            _ => return Err(FramingError::InvalidQueue),
        };

        // 6. Verify 4-byte transport checksum at the tail
        let payload_len = length_prefix - Self::HEADER_LEN - Self::CHECKSUM_SIZE;
        let payload_end = Self::HEADER_LEN + payload_len;
        let body_to_checksum = &frame_slice[0..payload_end];
        let expected_checksum = compute_transport_checksum(body_to_checksum);
        let actual_checksum: [u8; 4] = frame_slice[payload_end..payload_end + Self::CHECKSUM_SIZE]
            .try_into()
            .unwrap();

        if actual_checksum != expected_checksum {
            return Err(FramingError::CommitmentMismatch);
        }

        let payload = frame_slice[Self::HEADER_LEN..payload_end].to_vec();

        Ok(Some(WireFrame {
            magic: *MAGIC_HEADER,
            version,
            queue,
            payload,
        }))
    }

    /// Encode a WireFrame into wire format with length prefix and transport checksum.
    pub fn encode_frame(frame: &WireFrame) -> Result<Vec<u8>, FramingError> {
        let length_prefix = Self::HEADER_LEN + frame.payload.len() + Self::CHECKSUM_SIZE;
        if length_prefix > MAX_PACKET_SIZE {
            return Err(FramingError::FrameTooLarge(length_prefix));
        }

        let mut out = Vec::with_capacity(4 + length_prefix);
        out.extend_from_slice(&(length_prefix as u32).to_be_bytes());
        out.extend_from_slice(&frame.magic);
        out.extend_from_slice(&frame.version.to_be_bytes());
        out.push(frame.queue as u8);
        out.extend_from_slice(&frame.payload);

        let checksum = compute_transport_checksum(&out[4..4 + Self::HEADER_LEN + frame.payload.len()]);
        out.extend_from_slice(&checksum);

        Ok(out)
    }

    /// Number of buffered unparsed bytes.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// Reset accumulator buffer (e.g. on framing desynchronization or error).
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}
