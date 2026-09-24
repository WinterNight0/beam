//! The sending half.
//!
//! The one rule that shapes this file: no file bytes leave until the peer has
//! sent ACCEPT (S-3). The state machine is consulted before every chunk rather
//! than the order of statements being trusted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite};

use crate::transport::PathKind;

use super::chunk::{ChunkPlan, hash_stream, sha256_hex};
use super::engine::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_CHUNK_ATTEMPTS, Progress, Reporter, TransferError,
};
use super::frame::{MAX_CHUNK_DATA, read_message, write_message};
use super::message::{
    CHUNK_SIZE, Cancel, ChunkStart, Complete, Message, TransferId, TransferRequest,
};
use super::state::{Event, Machine};

/// What to send, and how patiently.
#[derive(Clone, Debug)]
pub struct SendOptions {
    /// The file to send.
    pub path: PathBuf,
    /// This device's Ed25519 public key, base64.
    pub sender_public_key: String,
    /// How long to wait for the peer to answer the prompt.
    pub accept_timeout: Duration,
    /// How many times to re-send a chunk that failed its hash.
    pub max_chunk_attempts: u32,
    /// How the peers are connected, for the progress line.
    pub path_kind: PathKind,
}

impl SendOptions {
    /// Options for sending `path` as the holder of `public_key`.
    pub fn new(path: impl Into<PathBuf>, public_key: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            sender_public_key: public_key.into(),
            accept_timeout: DEFAULT_ACCEPT_TIMEOUT,
            max_chunk_attempts: DEFAULT_CHUNK_ATTEMPTS,
            path_kind: PathKind::Direct,
        }
    }
}

/// How a completed send turned out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendSummary {
    pub transfer_id: TransferId,
    pub bytes_sent: u64,
    /// The name the receiver actually saved the file under, which may differ
    /// from the name that was sent if it collided with an existing file.
    pub final_name: Option<String>,
}

/// Sends one file over an already-connected byte stream.
pub async fn send_file<S, R>(
    stream: &mut S,
    options: &SendOptions,
    reporter: &mut R,
) -> Result<SendSummary, TransferError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: Reporter,
{
    let file_name = local_file_name(&options.path)?;

    // Hash the whole file first, so file_sha256 binds this transfer to exact
    // contents before the peer is asked to agree to anything.
    let (file_sha256, size) = hash_whole_file(&options.path, reporter).await?;
    let plan = ChunkPlan::new(size, CHUNK_SIZE);

    let transfer_id = TransferId::generate().map_err(|e| {
        TransferError::io(
            "draw a transfer id for",
            options.path.display(),
            std::io::Error::other(e),
        )
    })?;

    let mut machine = Machine::new();

    let request = TransferRequest {
        transfer_id,
        sender_public_key: options.sender_public_key.clone(),
        file_name,
        size,
        chunk_size: plan.chunk_size(),
        chunk_count: plan.chunk_count(),
        file_sha256,
    };
    write_message(stream, &Message::TransferRequest(request)).await?;
    machine.apply(Event::RequestSent)?;
    reporter.report(Progress::AwaitingAccept);

    await_decision(stream, &mut machine, options.accept_timeout).await?;
    reporter.report(Progress::Accepted {
        path: options.path_kind,
    });

    let mut file = tokio::fs::File::open(&options.path)
        .await
        .map_err(|e| TransferError::io("open", options.path.display(), e))?;

    let mut sent = 0u64;
    for index in 0..plan.chunk_count() {
        let bytes = read_chunk(&mut file, &options.path, plan, index).await?;
        send_chunk(
            stream,
            &mut machine,
            index,
            &bytes,
            options.max_chunk_attempts,
        )
        .await?;
        sent += bytes.len() as u64;
        reporter.report(Progress::Transferring {
            done: sent,
            total: size,
            path: options.path_kind,
        });
    }

    write_message(
        stream,
        &Message::Complete(Complete {
            transfer_id,
            final_name: None,
        }),
    )
    .await?;
    machine.apply(Event::ChunksDone)?;
    reporter.report(Progress::Verifying);

    // The receiver verifies the whole-file hash and answers with the name it
    // saved the file under.
    let final_name = match read_message(stream).await? {
        Message::Complete(complete) => complete.final_name,
        Message::Cancel(cancel) => {
            machine.apply(Event::Cancel)?;
            return Err(TransferError::Cancelled(cancel.reason));
        }
        other => {
            machine.apply(Event::Fail)?;
            return Err(TransferError::OutOfOrder {
                expected: "COMPLETE",
                got: other.kind_name(),
            });
        }
    };
    machine.apply(Event::Verified)?;

    Ok(SendSummary {
        transfer_id,
        bytes_sent: sent,
        final_name,
    })
}

/// Waits for ACCEPT or REJECT, and treats silence as a Reject (S-5).
async fn await_decision<S>(
    stream: &mut S,
    machine: &mut Machine,
    accept_timeout: Duration,
) -> Result<(), TransferError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let answer = tokio::time::timeout(accept_timeout, read_message(stream)).await;

    let message = match answer {
        Err(_elapsed) => {
            machine.apply(Event::TimedOut)?;
            return Err(TransferError::Rejected(
                super::message::RejectReason::Expired,
            ));
        }
        Ok(result) => result?,
    };

    match message {
        Message::Accept(_) => {
            machine.apply(Event::Accepted)?;
            // On a plain byte stream the connection already exists, so this
            // state is entered and left at once. From M5 it is where ICE
            // happens. See ADR-0016.
            machine.apply(Event::Connected)?;
            Ok(())
        }
        Message::Reject(reject) => {
            machine.apply(Event::Declined)?;
            Err(TransferError::Rejected(reject.reason))
        }
        Message::Cancel(cancel) => {
            machine.apply(Event::Cancel)?;
            Err(TransferError::Cancelled(cancel.reason))
        }
        other => {
            machine.apply(Event::Fail)?;
            Err(TransferError::OutOfOrder {
                expected: "ACCEPT or REJECT",
                got: other.kind_name(),
            })
        }
    }
}

/// Sends one chunk, re-sending it while the receiver reports a bad hash.
async fn send_chunk<S>(
    stream: &mut S,
    machine: &mut Machine,
    index: u32,
    bytes: &[u8],
    max_attempts: u32,
) -> Result<(), TransferError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // S-3, checked rather than assumed: the only state in which file bytes may
    // leave this process is Transferring.
    if !machine.file_data_allowed() {
        machine.apply(Event::Fail)?;
        return Err(TransferError::OutOfOrder {
            expected: "an accepted transfer",
            got: "a chunk",
        });
    }

    let digest = sha256_hex(bytes);

    for attempt in 1..=max_attempts {
        write_message(
            stream,
            &Message::ChunkStart(ChunkStart {
                index,
                len: bytes.len() as u32,
                sha256: digest.clone(),
            }),
        )
        .await?;

        for slice in bytes.chunks(MAX_CHUNK_DATA) {
            write_message(
                stream,
                &Message::ChunkData {
                    index,
                    bytes: slice.to_vec(),
                },
            )
            .await?;
        }
        // A zero-length chunk still needs one empty data frame, so that the
        // receiver sees the chunk end the same way every time.
        if bytes.is_empty() {
            write_message(
                stream,
                &Message::ChunkData {
                    index,
                    bytes: Vec::new(),
                },
            )
            .await?;
        }

        match read_message(stream).await? {
            Message::ChunkAck(ack) if ack.index == index => return Ok(()),
            Message::ChunkNak(nak) if nak.index == index => {
                if attempt == max_attempts {
                    machine.apply(Event::Fail)?;
                    return Err(TransferError::ChunkFailed {
                        index,
                        attempts: max_attempts,
                    });
                }
            }
            Message::Cancel(cancel) => {
                machine.apply(Event::Cancel)?;
                return Err(TransferError::Cancelled(cancel.reason));
            }
            other => {
                machine.apply(Event::Fail)?;
                return Err(TransferError::OutOfOrder {
                    expected: "CHUNK_ACK or CHUNK_NAK",
                    got: other.kind_name(),
                });
            }
        }
    }

    machine.apply(Event::Fail)?;
    Err(TransferError::ChunkFailed {
        index,
        attempts: max_attempts,
    })
}

/// Sends CANCEL, ignoring a stream that has already gone away.
pub async fn send_cancel<S>(stream: &mut S, transfer_id: TransferId, reason: &str)
where
    S: AsyncWrite + Unpin,
{
    let _ = write_message(
        stream,
        &Message::Cancel(Cancel {
            transfer_id,
            reason: reason.to_string(),
        }),
    )
    .await;
}

async fn hash_whole_file<R>(path: &Path, reporter: &mut R) -> Result<(String, u64), TransferError>
where
    R: Reporter,
{
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|e| TransferError::io("read", path.display(), e))?;
    let total = metadata.len();

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| TransferError::io("open", path.display(), e))?;

    let (digest, size) = hash_stream(&mut file, total, |done, total| {
        reporter.report(Progress::Hashing { done, total })
    })
    .await
    .map_err(|e| TransferError::io("read", path.display(), e))?;

    Ok((digest, size))
}

async fn read_chunk(
    file: &mut tokio::fs::File,
    path: &Path,
    plan: ChunkPlan,
    index: u32,
) -> Result<Vec<u8>, TransferError> {
    // Seeking rather than reading straight through, because M3 will ask for
    // chunks out of order.
    file.seek(std::io::SeekFrom::Start(plan.offset_of(index)))
        .await
        .map_err(|e| TransferError::io("seek in", path.display(), e))?;

    let mut bytes = vec![0u8; plan.len_of(index) as usize];
    file.read_exact(&mut bytes)
        .await
        .map_err(|e| TransferError::io("read", path.display(), e))?;
    Ok(bytes)
}

/// The base name of the file being sent.
fn local_file_name(path: &Path) -> Result<String, TransferError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .ok_or_else(|| {
            TransferError::BadRequest(format!("{} has no usable file name", path.display()))
        })
}
