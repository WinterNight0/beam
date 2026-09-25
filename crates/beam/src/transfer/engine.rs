//! Types shared by the sending and receiving halves.

use std::time::Duration;

use crate::transport::PathKind;

use super::chunk::PlanError;
use super::frame::FrameError;
use super::message::RejectReason;
use super::paths::NameError;
use super::state::IllegalTransition;

/// How long an unanswered request lives before it counts as a Reject (S-5).
pub const DEFAULT_ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long either side waits for the peer's next frame once a transfer has
/// been accepted. A peer that goes quiet — while its connection stays alive —
/// would otherwise hold the receiver's one transfer slot for ever (ADR-0033).
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How often the receiver tells the sender it is still verifying.
pub const DEFAULT_KEEPALIVE_EVERY: Duration = Duration::from_secs(1);

/// How many times one chunk may be re-sent after a hash mismatch.
pub const DEFAULT_CHUNK_ATTEMPTS: u32 = 3;

/// Why a transfer did not complete.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The peer refused. Expected, not a fault.
    #[error("{}", .0.explain())]
    Rejected(RejectReason),

    /// Somebody stopped the transfer deliberately.
    /// The reason is the peer's text: shown only through `untrusted`.
    #[error("the transfer was cancelled: {}", crate::untrusted::text(.0))]
    Cancelled(String),

    /// File bytes arrived before this side had sent ACCEPT (S-4).
    ///
    /// Everything received is discarded and the transfer is abandoned; a peer
    /// that does this is either broken or trying something.
    #[error("the peer sent file data before the transfer was accepted; discarded it and stopped")]
    DataBeforeAccept,

    /// The peer said something that does not belong where it said it.
    #[error("the peer sent {got} when {expected} was expected")]
    OutOfOrder {
        expected: &'static str,
        got: &'static str,
    },

    /// A transfer id that has already been used (S-11).
    #[error("that transfer id has already been used")]
    ReplayedTransferId,

    /// The request described a file in a way that does not add up.
    #[error("the peer sent a malformed request: {}", crate::untrusted::text(.0))]
    BadRequest(String),

    /// The peer sent nothing for the stall timeout mid-transfer.
    #[error("the peer sent nothing for {} s while {waiting_for}; gave up", after.as_secs())]
    Stalled {
        after: Duration,
        waiting_for: &'static str,
    },

    /// A chunk kept failing its hash.
    #[error("chunk {index} failed its hash {attempts} times; giving up")]
    ChunkFailed { index: u32, attempts: u32 },

    /// The assembled file did not match the hash promised for it.
    #[error("the finished file does not match the hash the sender promised; nothing was saved")]
    VerificationFailed,

    /// A second session tried to work on a partial that is already in use.
    #[error("another beam session is already receiving this file")]
    PartialInUse,

    /// The receiver has not got room for this transfer.
    #[error(transparent)]
    Space(Box<super::storage::SpaceError>),

    /// Something went wrong with the stored partial itself.
    #[error(transparent)]
    Partial(Box<super::partial::PartialError>),

    /// A have-bitmap that does not describe this transfer.
    #[error("the peer sent a have-bitmap that does not fit this transfer: {0}")]
    Bitmap(#[from] super::bitmap::BitmapError),

    #[error(transparent)]
    Name(#[from] NameError),

    #[error(transparent)]
    Plan(#[from] PlanError),

    #[error(transparent)]
    Frame(#[from] FrameError),

    #[error(transparent)]
    Illegal(#[from] IllegalTransition),

    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl TransferError {
    pub(crate) fn io(
        action: &'static str,
        path: impl std::fmt::Display,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            action,
            path: path.to_string(),
            source,
        }
    }
}

/// What the engine is doing, for the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Reading the file to compute its SHA-256, before anything is sent.
    ///
    /// On a large file this takes long enough that saying so is the difference
    /// between "working" and "frozen".
    Hashing { done: u64, total: u64 },
    /// The request is with the peer; waiting for a person to answer.
    AwaitingAccept,
    /// The peer accepted; bytes are about to move.
    Accepted { path: PathKind },
    /// Bytes are moving.
    Transferring {
        done: u64,
        total: u64,
        path: PathKind,
    },
    /// Checking the whole-file hash.
    Verifying,
    /// Re-hashing what is already on disk from an earlier session, before
    /// offering any of it to the sender as "already have".
    Rechecking,
    /// The peer is checking the whole-file hash; sender side only.
    PeerVerifying { done: u64, total: u64 },
    /// The connection moved to a different path mid-transfer, e.g. from the
    /// relay to a direct path once hole punching succeeded (F-11).
    PathChanged { from: PathKind, to: PathKind },
}

/// Somewhere to send [`Progress`].
pub trait Reporter {
    fn report(&mut self, progress: Progress);
}

/// Throws progress away. Used by tests and by non-interactive runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SilentReporter;

impl Reporter for SilentReporter {
    fn report(&mut self, _progress: Progress) {}
}

impl<T: Reporter + ?Sized> Reporter for &mut T {
    fn report(&mut self, progress: Progress) {
        (**self).report(progress);
    }
}
