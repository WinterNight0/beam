//! The receiving half.
//!
//! Four rules shape this file:
//!
//! * only peers in `known_peers` get as far as the prompt (S-7);
//! * a person answers every prompt, and there is no way to skip it (S-1);
//! * silence is a Reject (S-5);
//! * anything carrying file bytes that arrives before ACCEPT ends the transfer
//!   and everything received is discarded (S-4).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::identity::{Fingerprint, KnownPeers, decode_public_key};
use crate::transport::PathKind;

use super::chunk::{ChunkPlan, hash_stream, sha256_hex};
use super::engine::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_CHUNK_ATTEMPTS, Progress, Reporter, TransferError,
};
use super::frame::{FrameError, read_message, write_message};
use super::message::{
    Accept, Cancel, ChunkAck, ChunkNak, Complete, Message, NakReason, Reject, RejectReason,
    TransferId, TransferRequest,
};
use super::paths::{reserve_destination, sanitize_file_name};
use super::state::{Event, Machine};

/// What the person answering the prompt is shown (S-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRequest {
    /// The nickname *this* machine stored for the peer. The sender has no say
    /// in it, so it cannot dress itself up as somebody else.
    pub peer_name: String,
    pub fingerprint: String,
    pub file_name: String,
    pub size: u64,
}

/// Asks a person whether to accept a transfer.
///
/// There is deliberately no implementation of this trait that answers by
/// itself: accepting is what a person does (S-1). Blocking is fine — it is
/// called on a blocking thread — but the call may be abandoned if the request
/// expires first, so it must not hold anything the next prompt needs.
pub trait Prompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool>;
}

/// Where received files go, and how patiently.
#[derive(Clone, Debug)]
pub struct ReceiveOptions {
    /// Where verified files are moved to.
    pub out_dir: PathBuf,
    /// Where partial files live while they are being assembled.
    pub tmp_dir: PathBuf,
    /// How long the prompt may go unanswered before it counts as a Reject.
    pub accept_timeout: Duration,
    /// How many times a chunk may be asked for again.
    pub max_chunk_attempts: u32,
    /// How the peers are connected, for the progress line.
    pub path_kind: PathKind,
}

impl ReceiveOptions {
    /// Options writing into `out_dir`, working under `tmp_dir`.
    pub fn new(out_dir: impl Into<PathBuf>, tmp_dir: impl Into<PathBuf>) -> Self {
        Self {
            out_dir: out_dir.into(),
            tmp_dir: tmp_dir.into(),
            accept_timeout: DEFAULT_ACCEPT_TIMEOUT,
            max_chunk_attempts: DEFAULT_CHUNK_ATTEMPTS,
            path_kind: PathKind::Direct,
        }
    }
}

/// How a completed receive turned out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiveSummary {
    pub transfer_id: TransferId,
    pub peer_name: String,
    pub fingerprint: String,
    /// The name the file was saved under, after any collision renaming.
    pub final_name: String,
    pub bytes: u64,
}

/// A stream of messages from the peer.
///
/// Reading is pumped into a channel by its own task so that waiting for a
/// message is cancel-safe: the prompt, the expiry timer and the arrival of an
/// early frame are raced against each other, and a losing `recv` must not eat
/// half a frame.
struct Incoming {
    rx: mpsc::Receiver<Result<Message, FrameError>>,
    pump: tokio::task::JoinHandle<()>,
}

impl Incoming {
    async fn next(&mut self) -> Result<Message, TransferError> {
        match self.rx.recv().await {
            Some(Ok(message)) => Ok(message),
            Some(Err(e)) => Err(e.into()),
            None => Err(FrameError::Closed.into()),
        }
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// Receives one file over an already-connected byte stream.
///
/// `seen` carries the transfer ids this process has already handled, so a
/// replayed id is refused (S-11). M3 will persist it.
pub async fn receive_file<S, P, R>(
    stream: S,
    known_peers: &KnownPeers,
    options: &ReceiveOptions,
    prompt: P,
    reporter: &mut R,
    seen: &mut HashSet<TransferId>,
) -> Result<ReceiveSummary, TransferError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    P: Prompt + Send + 'static,
    R: Reporter,
{
    let (reader, mut writer) = tokio::io::split(stream);

    let (tx, rx) = mpsc::channel(4);
    let pump = tokio::spawn(async move {
        let mut reader = reader;
        loop {
            let result = read_message(&mut reader).await;
            let fatal = result.is_err();
            if tx.send(result).await.is_err() || fatal {
                break;
            }
        }
    });
    let mut incoming = Incoming { rx, pump };

    let mut machine = Machine::new();
    let request = read_request(&mut incoming, options.accept_timeout).await?;
    machine.apply(Event::RequestSent)?;

    // Validate before anything is created on disk, and before a person is
    // interrupted with a prompt.
    let file_name = sanitize_file_name(&request.file_name)?;
    let plan = ChunkPlan::try_new(request.size, request.chunk_size, request.chunk_count)?;

    if !seen.insert(request.transfer_id) {
        refuse(&mut writer, request.transfer_id, RejectReason::BadRequest).await;
        machine.apply(Event::Fail)?;
        return Err(TransferError::ReplayedTransferId);
    }

    // S-7: a sender this machine has not paired with never reaches the prompt.
    let Some((peer_name, fingerprint)) = known_sender(known_peers, &request.sender_public_key)
    else {
        refuse(&mut writer, request.transfer_id, RejectReason::UnknownPeer).await;
        machine.apply(Event::Declined)?;
        return Err(TransferError::Rejected(RejectReason::UnknownPeer));
    };

    let prompt_request = PromptRequest {
        peer_name: peer_name.clone(),
        fingerprint: fingerprint.to_string(),
        file_name: file_name.clone(),
        size: request.size,
    };

    match decide(
        &mut incoming,
        prompt,
        prompt_request,
        options.accept_timeout,
    )
    .await?
    {
        Decision::Accept => {}
        Decision::Decline => {
            refuse(&mut writer, request.transfer_id, RejectReason::Declined).await;
            machine.apply(Event::Declined)?;
            return Err(TransferError::Rejected(RejectReason::Declined));
        }
        Decision::Expired => {
            refuse(&mut writer, request.transfer_id, RejectReason::Expired).await;
            machine.apply(Event::TimedOut)?;
            return Err(TransferError::Rejected(RejectReason::Expired));
        }
    }

    write_message(
        &mut writer,
        &Message::Accept(Accept {
            transfer_id: request.transfer_id,
            have_bitmap: None,
        }),
    )
    .await?;
    machine.apply(Event::Accepted)?;
    machine.apply(Event::Connected)?;

    // From here on there is a working directory to clean up on any failure.
    let work_dir = options.tmp_dir.join(request.transfer_id.to_string());
    let outcome = accept_and_store(
        &mut incoming,
        &mut writer,
        &mut machine,
        &request,
        plan,
        &file_name,
        &work_dir,
        options,
        reporter,
    )
    .await;

    let _ = tokio::fs::remove_dir_all(&work_dir).await;

    let final_name = outcome?;
    Ok(ReceiveSummary {
        transfer_id: request.transfer_id,
        peer_name,
        fingerprint: fingerprint.to_string(),
        final_name,
        bytes: request.size,
    })
}

/// What the person decided, or that they never got the chance.
enum Decision {
    Accept,
    Decline,
    Expired,
}

/// Races the prompt against the expiry timer and against the peer jumping the
/// gun.
async fn decide<P>(
    incoming: &mut Incoming,
    prompt: P,
    request: PromptRequest,
    accept_timeout: Duration,
) -> Result<Decision, TransferError>
where
    P: Prompt + Send + 'static,
{
    let mut asking = tokio::task::spawn_blocking(move || {
        let mut prompt = prompt;
        prompt.confirm(&request)
    });

    tokio::select! {
        answer = &mut asking => {
            match answer {
                Ok(Ok(true)) => Ok(Decision::Accept),
                Ok(Ok(false)) => Ok(Decision::Decline),
                Ok(Err(e)) => Err(TransferError::io("read an answer from", "the terminal", e)),
                // The prompt thread died; refusing is the safe reading.
                Err(_joined) => Ok(Decision::Decline),
            }
        }

        early = incoming.next() => {
            // Nothing legitimately arrives here. If it carries file bytes this
            // is S-4 exactly, and the caller discards the work directory.
            match early {
                Ok(message) if message.carries_file_data() => Err(TransferError::DataBeforeAccept),
                Ok(Message::Cancel(cancel)) => Err(TransferError::Cancelled(cancel.reason)),
                Ok(other) => Err(TransferError::OutOfOrder {
                    expected: "nothing until the transfer is accepted",
                    got: other.kind_name(),
                }),
                Err(e) => Err(e),
            }
        }

        _ = tokio::time::sleep(accept_timeout) => Ok(Decision::Expired),
    }
}

/// Reads the opening request, refusing to wait forever for a silent peer.
async fn read_request(
    incoming: &mut Incoming,
    timeout: Duration,
) -> Result<TransferRequest, TransferError> {
    let message = tokio::time::timeout(timeout, incoming.next())
        .await
        .map_err(|_| TransferError::Rejected(RejectReason::Expired))??;

    match message {
        Message::TransferRequest(request) => Ok(request),
        other => Err(TransferError::OutOfOrder {
            expected: "TRANSFER_REQUEST",
            got: other.kind_name(),
        }),
    }
}

/// Looks the claimed sender up in `known_peers`.
///
/// STRENGTHEN IN M6: this only checks that the key the sender *claims* is one
/// we have paired with. Nothing here proves the sender holds the matching
/// private key; the Noise KK handshake is what will. See ADR-0019.
fn known_sender(known_peers: &KnownPeers, claimed_key: &str) -> Option<(String, Fingerprint)> {
    let key = decode_public_key(claimed_key).ok()?;
    let peer = known_peers.lookup_key(&key)?;
    Some((peer.name.clone(), peer.fingerprint()))
}

async fn refuse<W>(writer: &mut W, transfer_id: TransferId, reason: RejectReason)
where
    W: AsyncWrite + Unpin,
{
    let _ = write_message(
        writer,
        &Message::Reject(Reject {
            transfer_id,
            reason,
        }),
    )
    .await;
}

/// Everything after ACCEPT: assemble, verify, commit.
#[allow(clippy::too_many_arguments)]
async fn accept_and_store<W, R>(
    incoming: &mut Incoming,
    writer: &mut W,
    machine: &mut Machine,
    request: &TransferRequest,
    plan: ChunkPlan,
    file_name: &str,
    work_dir: &Path,
    options: &ReceiveOptions,
    reporter: &mut R,
) -> Result<String, TransferError>
where
    W: AsyncWrite + Unpin,
    R: Reporter,
{
    tokio::fs::create_dir_all(work_dir)
        .await
        .map_err(|e| TransferError::io("create", work_dir.display(), e))?;
    tokio::fs::create_dir_all(&options.out_dir)
        .await
        .map_err(|e| TransferError::io("create", options.out_dir.display(), e))?;

    let part_path = work_dir.join("part");
    let mut part = tokio::fs::File::create(&part_path)
        .await
        .map_err(|e| TransferError::io("create", part_path.display(), e))?;

    let mut received = 0u64;
    for index in 0..plan.chunk_count() {
        let bytes =
            receive_chunk(incoming, writer, plan, index, options.max_chunk_attempts).await?;

        part.seek(std::io::SeekFrom::Start(plan.offset_of(index)))
            .await
            .map_err(|e| TransferError::io("seek in", part_path.display(), e))?;
        part.write_all(&bytes)
            .await
            .map_err(|e| TransferError::io("write", part_path.display(), e))?;

        received += bytes.len() as u64;
        reporter.report(Progress::Transferring {
            done: received,
            total: plan.size(),
            path: options.path_kind,
        });

        write_message(writer, &Message::ChunkAck(ChunkAck { index })).await?;
    }

    part.flush()
        .await
        .map_err(|e| TransferError::io("flush", part_path.display(), e))?;
    drop(part);

    // The sender says it has finished before the whole-file hash is checked.
    match incoming.next().await? {
        Message::Complete(_) => {}
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
    }
    machine.apply(Event::ChunksDone)?;
    reporter.report(Progress::Verifying);

    // On failure the caller removes the work directory, so a file that does
    // not match its promised hash is never left anywhere.
    verify(&part_path, &request.file_sha256).await?;

    // Reserve the name by creating the file, then move the verified part over
    // it. Nothing is written under the destination name until the contents are
    // known to be right (D-6, D-7).
    let (placeholder, destination, final_name) =
        reserve_destination(&options.out_dir, file_name)
            .map_err(|e| TransferError::io("create a file in", options.out_dir.display(), e))?;
    drop(placeholder);

    tokio::fs::rename(&part_path, &destination)
        .await
        .map_err(|e| TransferError::io("move the finished file to", destination.display(), e))?;

    machine.apply(Event::Verified)?;
    write_message(
        writer,
        &Message::Complete(Complete {
            transfer_id: request.transfer_id,
            final_name: Some(final_name.clone()),
        }),
    )
    .await?;

    Ok(final_name)
}

/// Collects one chunk, verifying it before it is handed back to be written.
async fn receive_chunk<W>(
    incoming: &mut Incoming,
    writer: &mut W,
    plan: ChunkPlan,
    index: u32,
    max_attempts: u32,
) -> Result<Vec<u8>, TransferError>
where
    W: AsyncWrite + Unpin,
{
    let expected_len = plan.len_of(index);

    for attempt in 1..=max_attempts {
        let start = match incoming.next().await? {
            Message::ChunkStart(start) => start,
            Message::Cancel(cancel) => return Err(TransferError::Cancelled(cancel.reason)),
            other => {
                return Err(TransferError::OutOfOrder {
                    expected: "CHUNK_START",
                    got: other.kind_name(),
                });
            }
        };

        if start.index != index {
            return Err(TransferError::OutOfOrder {
                expected: "the next chunk in order",
                got: "a chunk out of order",
            });
        }
        // Bounds the buffer below: a peer cannot make us reserve more than the
        // size it already committed to in the request.
        if start.len != expected_len {
            return Err(TransferError::BadRequest(format!(
                "chunk {index} was announced as {} bytes, but the request implies {expected_len}",
                start.len
            )));
        }

        let mut buffer = Vec::with_capacity(expected_len as usize);
        loop {
            match incoming.next().await? {
                Message::ChunkData { index: got, bytes } if got == index => {
                    buffer.extend_from_slice(&bytes);
                }
                Message::Cancel(cancel) => return Err(TransferError::Cancelled(cancel.reason)),
                other => {
                    return Err(TransferError::OutOfOrder {
                        expected: "CHUNK_DATA",
                        got: other.kind_name(),
                    });
                }
            }
            // At least one frame is always read, so a zero-length chunk ends
            // the same way every other chunk does.
            if buffer.len() >= expected_len as usize {
                break;
            }
        }

        let reason = if buffer.len() != expected_len as usize {
            Some(NakReason::LengthMismatch)
        } else if sha256_hex(&buffer) != start.sha256 {
            Some(NakReason::HashMismatch)
        } else {
            None
        };

        match reason {
            // Verified before it is written, which is the whole point (S-12).
            None => return Ok(buffer),
            Some(reason) => {
                write_message(writer, &Message::ChunkNak(ChunkNak { index, reason })).await?;
                if attempt == max_attempts {
                    return Err(TransferError::ChunkFailed {
                        index,
                        attempts: max_attempts,
                    });
                }
            }
        }
    }

    Err(TransferError::ChunkFailed {
        index,
        attempts: max_attempts,
    })
}

/// Hashes the assembled file and compares it with what the sender promised.
async fn verify(part_path: &Path, expected: &str) -> Result<(), TransferError> {
    let mut file = tokio::fs::File::open(part_path)
        .await
        .map_err(|e| TransferError::io("open", part_path.display(), e))?;
    let total = file
        .metadata()
        .await
        .map_err(|e| TransferError::io("read", part_path.display(), e))?
        .len();

    let (digest, _) = hash_stream(&mut file, total, |_, _| {})
        .await
        .map_err(|e| TransferError::io("read", part_path.display(), e))?;

    if digest == expected {
        Ok(())
    } else {
        Err(TransferError::VerificationFailed)
    }
}

/// Sends CANCEL, ignoring a stream that has already gone away.
pub async fn send_cancel<W>(writer: &mut W, transfer_id: TransferId, reason: &str)
where
    W: AsyncWrite + Unpin,
{
    let _ = write_message(
        writer,
        &Message::Cancel(Cancel {
            transfer_id,
            reason: reason.to_string(),
        }),
    )
    .await;
}
