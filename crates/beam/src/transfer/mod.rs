//! The transfer engine: wire messages, framing, chunking, the state machine,
//! and the sending and receiving halves.
//!
//! The engine is generic over its byte stream and knows nothing about TCP or
//! QUIC; see [`crate::transport`] and ADR-0016.

pub mod bitmap;
pub mod chunk;
pub mod engine;
pub mod frame;
pub mod message;
pub mod partial;
pub mod paths;
pub mod receiver;
pub mod sender;
pub mod state;
pub mod storage;

pub use bitmap::{BitmapError, ChunkBitmap};
pub use chunk::{ChunkPlan, PlanError, sha256_hex};
pub use engine::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_CHUNK_ATTEMPTS, Progress, Reporter, SilentReporter,
    TransferError,
};
pub use frame::{FrameError, MAX_CHUNK_DATA, MAX_FRAME_PAYLOAD};
pub use message::{
    Accept, CHUNK_SIZE, Cancel, ChunkAck, ChunkNak, ChunkStart, Complete, Message, NakReason,
    Reject, RejectReason, TransferId, TransferRequest,
};
pub use partial::{
    DEFAULT_MAX_AGE, Partial, PartialError, PartialKey, PartialState, PartialStore, PartialSummary,
};
pub use paths::{NameError, reserve_destination, sanitize_file_name};
pub use receiver::{
    Prompt, PromptRequest, ReceiveOptions, ReceiveSummary, receive_file, turn_away,
};
pub use sender::{SendOptions, SendSummary, send_file};
pub use state::{Event, IllegalTransition, Machine, State};
pub use storage::{SPACE_MARGIN, SpaceError, check_space, commit, same_volume};
