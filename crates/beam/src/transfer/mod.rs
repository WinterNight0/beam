//! The transfer engine: wire messages, framing, chunking, the state machine,
//! and the sending and receiving halves.
//!
//! The engine is generic over its byte stream and knows nothing about TCP,
//! QUIC or WebRTC data channels; see [`crate::transport`] and ADR-0016.

pub mod frame;
pub mod message;

pub use frame::{FrameError, MAX_CHUNK_DATA, MAX_FRAME_PAYLOAD};
pub use message::{
    Accept, CHUNK_SIZE, Cancel, ChunkAck, ChunkNak, ChunkStart, Complete, Message, NakReason,
    Reject, RejectReason, TransferId, TransferRequest,
};
