//! The sending half.
//!
//! The one rule that shapes this file: no file bytes leave until the peer has
//! sent ACCEPT (S-3). The state machine is consulted before every chunk rather
//! than the order of statements being trusted.
//!
//! Once accepted, up to [`SendOptions::window`] chunks may be on their way
//! before their answers arrive (ADR-0040). Every chunk read from disk is
//! checked against the hash taken of it before the request, so what is sent
//! is what was promised.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite};

use crate::transport::{PathKind, Route, RouteTracker, fixed_route};

use super::bitmap::ChunkBitmap;
use super::chunk::{ChunkPlan, hash_stream_and_chunks, sha256_hex};
use super::engine::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_CHUNK_ATTEMPTS, DEFAULT_STALL_TIMEOUT, PIPELINE_WINDOW,
    Progress, Reporter, SOURCE_READ_ATTEMPTS, TransferError,
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
    /// The chunk size to propose. Configurable so tests can make a small file
    /// span several chunks without writing gigabytes.
    pub chunk_size: u32,
    /// How many times to re-send a chunk that failed its hash.
    pub max_chunk_attempts: u32,
    /// How long to wait for the receiver's next frame once accepted. The
    /// receiver sends keep-alives while it verifies, so a large file does not
    /// trip this (ADR-0033).
    pub stall_timeout: Duration,
    /// How the peers are connected, for the progress line. It can change
    /// during the transfer; see [`Route`].
    pub route: Route,
    /// How many chunks may be sent before their answers arrive. 1 is the
    /// original one-at-a-time protocol, which a receiver that only speaks
    /// `beam/xfer/1` needs; see [`crate::transport::dial::send_on`].
    pub window: u32,
}

impl SendOptions {
    /// Options for sending `path` as the holder of `public_key`.
    pub fn new(path: impl Into<PathBuf>, public_key: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            sender_public_key: public_key.into(),
            accept_timeout: DEFAULT_ACCEPT_TIMEOUT,
            chunk_size: CHUNK_SIZE,
            max_chunk_attempts: DEFAULT_CHUNK_ATTEMPTS,
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            route: fixed_route(PathKind::Direct),
            window: PIPELINE_WINDOW,
        }
    }
}

/// How a completed send turned out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendSummary {
    pub transfer_id: TransferId,
    /// Bytes actually put on the wire. Less than the file size when the
    /// receiver already had some of it.
    pub bytes_sent: u64,
    /// Bytes the receiver already had, and so were never sent.
    pub bytes_skipped: u64,
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
    // contents before the peer is asked to agree to anything. Each chunk's
    // hash is kept too, to check the chunk against when it is sent.
    let chunk_size = options.chunk_size.max(1);
    let (file_sha256, size, chunk_hashes) =
        hash_whole_file(&options.path, chunk_size, reporter).await?;
    let plan = ChunkPlan::new(size, chunk_size);

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

    let have = await_decision(stream, &mut machine, options.accept_timeout, plan).await?;
    let mut route = RouteTracker::new(&options.route);
    reporter.report(Progress::Accepted {
        path: route.poll().0,
    });

    let skipped: u64 = (0..plan.chunk_count())
        .filter(|i| have.get(*i))
        .map(|i| u64::from(plan.len_of(i)))
        .sum();

    let mut file = tokio::fs::File::open(&options.path)
        .await
        .map_err(|e| TransferError::io("open", options.path.display(), e))?;

    // Only what the receiver said it has not got. On a first attempt that is
    // every chunk; on a resume it is the gap. A chunk the receiver rejects
    // goes back to the front of the queue (ADR-0040, option B).
    let mut queue: VecDeque<u32> = have.missing().collect();
    let mut in_flight: Vec<u32> = Vec::new();
    let mut attempts: HashMap<u32, u32> = HashMap::new();
    let window = options.window.max(1) as usize;
    let mut sent = 0u64;

    loop {
        while in_flight.len() < window {
            let Some(index) = queue.pop_front() else {
                break;
            };
            let expected = &chunk_hashes[index as usize];
            let bytes =
                match read_verified_chunk(&mut file, &options.path, plan, index, expected).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        if matches!(e, TransferError::SourceChanged { .. }) {
                            send_cancel(
                                stream,
                                transfer_id,
                                "the file changed while it was being sent",
                            )
                            .await;
                        }
                        machine.apply(Event::Fail)?;
                        return Err(e);
                    }
                };
            send_chunk(stream, &mut machine, index, &bytes, expected).await?;
            *attempts.entry(index).or_insert(0) += 1;
            in_flight.push(index);
        }
        if in_flight.is_empty() {
            break;
        }

        let answer = tokio::time::timeout(options.stall_timeout, read_message(stream))
            .await
            .map_err(|_| TransferError::Stalled {
                after: options.stall_timeout,
                waiting_for: "waiting for a chunk to be acknowledged",
            })??;
        match answer {
            Message::ChunkAck(ack) if in_flight.contains(&ack.index) => {
                in_flight.retain(|i| *i != ack.index);
                sent += u64::from(plan.len_of(ack.index));
                let (path, before) = route.poll();
                if let Some(from) = before {
                    reporter.report(Progress::PathChanged { from, to: path });
                }
                reporter.report(Progress::Transferring {
                    done: skipped + sent,
                    total: size,
                    path,
                });
            }
            Message::ChunkNak(nak) if in_flight.contains(&nak.index) => {
                in_flight.retain(|i| *i != nak.index);
                if attempts[&nak.index] >= options.max_chunk_attempts {
                    machine.apply(Event::Fail)?;
                    return Err(TransferError::ChunkFailed {
                        index: nak.index,
                        attempts: options.max_chunk_attempts,
                    });
                }
                queue.push_front(nak.index);
            }
            Message::Cancel(cancel) => {
                machine.apply(Event::Cancel)?;
                return Err(TransferError::Cancelled(cancel.reason));
            }
            other => {
                machine.apply(Event::Fail)?;
                return Err(TransferError::OutOfOrder {
                    expected: "CHUNK_ACK or CHUNK_NAK for a chunk that was sent",
                    got: other.kind_name(),
                });
            }
        }
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
    // saved the file under. On a large file that takes a while, so it sends
    // VERIFYING frames meanwhile; each one resets the stall timeout.
    let final_name = loop {
        let message = tokio::time::timeout(options.stall_timeout, read_message(stream))
            .await
            .map_err(|_| TransferError::Stalled {
                after: options.stall_timeout,
                waiting_for: "waiting for the file to be verified",
            })??;
        match message {
            Message::Verifying(progress) => {
                reporter.report(Progress::PeerVerifying {
                    done: progress.done,
                    total: progress.total,
                });
                continue;
            }
            other => break other,
        }
    };
    let final_name = match final_name {
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
        bytes_skipped: skipped,
        final_name,
    })
}

/// Waits for ACCEPT or REJECT, and treats silence as a Reject (S-5).
///
/// Returns what the receiver says it already has, which decides which chunks
/// get sent.
async fn await_decision<S>(
    stream: &mut S,
    machine: &mut Machine,
    accept_timeout: Duration,
    plan: ChunkPlan,
) -> Result<ChunkBitmap, TransferError>
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
        Message::Accept(accept) => {
            // A have-bitmap decides which chunks are never sent, so it is
            // checked rather than trusted: exactly the right length, and no
            // bits past the end of the transfer. Anything else aborts instead
            // of being repaired, because the repair would be a guess about
            // which parts of a file to skip. See ADR-0024.
            let have = match accept.have_bitmap {
                Some(text) => match ChunkBitmap::decode(&text, plan.chunk_count()) {
                    Ok(have) => have,
                    Err(e) => {
                        machine.apply(Event::Fail)?;
                        return Err(e.into());
                    }
                },
                None => ChunkBitmap::new(plan.chunk_count()),
            };

            machine.apply(Event::Accepted)?;
            // On a plain byte stream the connection already exists, so this
            // state is entered and left at once. From M5 it is where ICE
            // happens. See ADR-0016.
            machine.apply(Event::Connected)?;
            Ok(have)
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

/// Writes one chunk: CHUNK_START, then its data frames. The answer is read by
/// the caller, which may have other chunks in flight.
async fn send_chunk<S>(
    stream: &mut S,
    machine: &mut Machine,
    index: u32,
    bytes: &[u8],
    digest: &str,
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

    write_message(
        stream,
        &Message::ChunkStart(ChunkStart {
            index,
            len: bytes.len() as u32,
            sha256: digest.to_string(),
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
    Ok(())
}

/// Reads chunk `index` and checks it against `expected`, the hash taken of it
/// before the request was sent.
///
/// A mismatch is read again, up to [`SOURCE_READ_ATTEMPTS`] times in all, in
/// case the disk returned bad data once. If it still does not match, or the
/// file has become too short, the file is no longer the one that was
/// promised: [`TransferError::SourceChanged`].
async fn read_verified_chunk(
    file: &mut tokio::fs::File,
    path: &Path,
    plan: ChunkPlan,
    index: u32,
    expected: &str,
) -> Result<Vec<u8>, TransferError> {
    for _ in 0..SOURCE_READ_ATTEMPTS {
        let bytes = match read_chunk(file, path, plan, index).await {
            Ok(bytes) => bytes,
            Err(TransferError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Err(TransferError::SourceChanged { index });
            }
            Err(e) => return Err(e),
        };
        if sha256_hex(&bytes) == expected {
            return Ok(bytes);
        }
    }
    Err(TransferError::SourceChanged { index })
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

/// The file's SHA-256, its size, and the SHA-256 of each chunk.
async fn hash_whole_file<R>(
    path: &Path,
    chunk_size: u32,
    reporter: &mut R,
) -> Result<(String, u64, Vec<String>), TransferError>
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

    hash_stream_and_chunks(&mut file, total, chunk_size, |done, total| {
        reporter.report(Progress::Hashing { done, total })
    })
    .await
    .map_err(|e| TransferError::io("read", path.display(), e))
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
