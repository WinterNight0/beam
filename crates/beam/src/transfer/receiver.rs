//! The receiving half.
//!
//! Four rules shape this file:
//!
//! * only peers in `known_peers` get as far as the prompt (S-7);
//! * a person answers every prompt, and there is no way to skip it (S-1);
//! * silence is a Reject (S-5);
//! * anything carrying file bytes that arrives before ACCEPT ends the transfer
//!   and everything received is discarded (S-4).
//!
//! From M3 a request may find a partial transfer already on disk. That does not
//! soften any of the above: a resume is an ordinary transfer that happens to
//! start with some chunks already present, and it is prompted for like any
//! other (S-2). What the partial changes is which chunks are asked for, and
//! what survives a failure — see ADR-0021 and ADR-0022.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use std::time::Duration as StdDuration;

use time::OffsetDateTime;

use crate::identity::{Fingerprint, KnownPeers, decode_public_key};
use crate::transport::{PathKind, Route, RouteTracker, fixed_route};

use super::chunk::{ChunkPlan, hash_stream, sha256_hex};
use super::engine::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_CHUNK_ATTEMPTS, Progress, Reporter, TransferError,
};
use super::frame::{FrameError, read_message, write_message};
use super::message::{
    Accept, Cancel, ChunkAck, ChunkNak, Complete, Message, NakReason, Reject, RejectReason,
    TransferId, TransferRequest,
};
use super::partial::{Partial, PartialError, PartialKey, PartialStore};
use super::paths::{reserve_destination, sanitize_file_name};
use super::state::{Event, Machine};
use super::storage::{check_space, commit};

/// What the person answering the prompt is shown (S-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRequest {
    /// The nickname *this* machine stored for the peer. The sender has no say
    /// in it, so it cannot dress itself up as somebody else.
    pub peer_name: String,
    pub fingerprint: String,
    pub file_name: String,
    pub size: u64,
    /// Present when data for this file is already on disk, so the person can
    /// see they are continuing something rather than starting it (S-2).
    pub resume: Option<ResumeInfo>,
}

/// What is already on disk for a transfer being resumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeInfo {
    pub have_bytes: u64,
    pub have_chunks: u32,
    pub chunk_count: u32,
    /// How long ago the partial was last written to, if that is known.
    pub age: Option<StdDuration>,
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
    /// How the peers are connected, for the progress line. It can change
    /// during the transfer; see [`Route`].
    pub route: Route,
    /// The sender's key **as proved by the transport**, when the transport
    /// proves one. Over iroh this is the connection's `remote_id()`; the M2
    /// TCP stand-in proves nothing and leaves it `None`. When it is set, the
    /// sender is looked up by it, and a request claiming any other key is
    /// refused. See ADR-0031.
    pub proven_sender: Option<ed25519_dalek::VerifyingKey>,
    /// How long an untouched partial survives.
    pub max_partial_age: StdDuration,
}

impl ReceiveOptions {
    /// Options writing into `out_dir`, working under `tmp_dir`.
    pub fn new(out_dir: impl Into<PathBuf>, tmp_dir: impl Into<PathBuf>) -> Self {
        Self {
            out_dir: out_dir.into(),
            tmp_dir: tmp_dir.into(),
            accept_timeout: DEFAULT_ACCEPT_TIMEOUT,
            max_chunk_attempts: DEFAULT_CHUNK_ATTEMPTS,
            route: fixed_route(PathKind::Direct),
            proven_sender: None,
            max_partial_age: super::partial::DEFAULT_MAX_AGE,
        }
    }

    /// The store of partial transfers under the tmp directory.
    pub fn partials(&self) -> PartialStore {
        PartialStore::new(&self.tmp_dir)
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
    /// How many bytes this session actually had to receive. Less than `bytes`
    /// when a partial was resumed.
    pub received_now: u64,
    /// Whether this session continued an earlier one.
    pub resumed: bool,
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

    // A transport that proved who is on the other end outranks whatever the
    // request says. A request that disagrees with the proof is not a mistake
    // an honest sender can make.
    if let Some(proven) = &options.proven_sender
        && decode_public_key(&request.sender_public_key).ok().as_ref() != Some(proven)
    {
        refuse(&mut writer, request.transfer_id, RejectReason::BadRequest).await;
        machine.apply(Event::Fail)?;
        return Err(TransferError::BadRequest(
            "the request claims a different key from the one the connection proved".into(),
        ));
    }

    // S-7: a sender this machine has not paired with never reaches the prompt.
    let Some((peer_name, fingerprint)) = known_sender(known_peers, &request.sender_public_key)
    else {
        refuse(&mut writer, request.transfer_id, RejectReason::UnknownPeer).await;
        machine.apply(Event::Declined)?;
        return Err(TransferError::Rejected(RejectReason::UnknownPeer));
    };

    // Find or start the partial. Matched on what the sender cannot change
    // without changing the file, never on the transfer id it chose (ADR-0021).
    let key = PartialKey {
        peer_fingerprint: fingerprint,
        file_sha256: request.file_sha256.clone(),
        size: request.size,
        chunk_size: request.chunk_size,
    };
    let mut partial = match options.partials().open(&key, &file_name, plan).await {
        Ok(partial) => partial,
        Err(PartialError::Busy) => {
            refuse(&mut writer, request.transfer_id, RejectReason::Busy).await;
            machine.apply(Event::Declined)?;
            return Err(TransferError::PartialInUse);
        }
        Err(e) => {
            machine.apply(Event::Fail)?;
            return Err(TransferError::Partial(Box::new(e)));
        }
    };

    // Everything from here can fail, and what happens to the partial when it
    // does is one decision made in one place, below.
    let outcome = receive_into_partial(
        &mut incoming,
        &mut writer,
        &mut machine,
        &request,
        plan,
        &file_name,
        &peer_name,
        &fingerprint,
        &mut partial,
        options,
        prompt,
        reporter,
    )
    .await;

    // The retention rules (ADR-0022), in one place so they cannot drift apart:
    //
    // * a finished transfer has become a file, so the partial has served its
    //   purpose;
    // * a partial that produced a file failing its whole-file hash will fail
    //   the same way next time, so keeping it would only waste a later session;
    // * a partial holding nothing is clutter — this covers a fresh request that
    //   was declined, expired, or aborted before a single chunk landed;
    // * anything else keeps what it has, which is the entire point of M3. In
    //   particular a peer that sends data before ACCEPT cannot destroy a
    //   partial an earlier, well-behaved session built.
    let holds_nothing = partial.bitmap().count() == 0;
    match &outcome {
        Ok(_) => discard(&partial),
        Err(TransferError::VerificationFailed) => discard(&partial),
        Err(_) if holds_nothing => discard(&partial),
        Err(_) => {}
    }

    let (final_name, received_now, resumed) = outcome?;
    Ok(ReceiveSummary {
        transfer_id: request.transfer_id,
        peer_name,
        fingerprint: fingerprint.to_string(),
        final_name,
        bytes: request.size,
        received_now,
        resumed,
    })
}

/// Re-checks the partial, asks the person, and runs the transfer.
///
/// Split out from [`receive_file`] so that every way this can fail passes
/// through one retention decision rather than each early return having to
/// remember the rules.
#[allow(clippy::too_many_arguments)]
async fn receive_into_partial<W, P, R>(
    incoming: &mut Incoming,
    writer: &mut W,
    machine: &mut Machine,
    request: &TransferRequest,
    plan: ChunkPlan,
    file_name: &str,
    peer_name: &str,
    fingerprint: &Fingerprint,
    partial: &mut Partial,
    options: &ReceiveOptions,
    prompt: P,
    reporter: &mut R,
) -> Result<(String, u64, bool), TransferError>
where
    W: AsyncWrite + Unpin,
    P: Prompt + Send + 'static,
    R: Reporter,
{
    // Disk is no more trustworthy than the wire: everything the bitmap claims
    // is re-hashed before it is offered to the sender as "already have".
    if !partial.is_new() {
        reporter.report(Progress::Rechecking);
        partial
            .reverify()
            .await
            .map_err(|e| TransferError::Partial(Box::new(e)))?;
    }

    let have_bytes = partial.have_bytes();
    let resume = (have_bytes > 0).then(|| ResumeInfo {
        have_bytes,
        have_chunks: partial.bitmap().count(),
        chunk_count: plan.chunk_count(),
        age: partial
            .updated_at()
            .and_then(|t| (OffsetDateTime::now_utc() - t).try_into().ok()),
    });

    // Asked before the prompt, so nobody is interrupted to agree to something
    // that cannot finish. A refusal here keeps the partial: freeing some space
    // and trying again is exactly the right next move.
    if let Err(e) = check_space(
        partial.dir(),
        &options.out_dir,
        request.size.saturating_sub(have_bytes),
        request.size,
    ) {
        refuse(writer, request.transfer_id, RejectReason::NoSpace).await;
        machine.apply(Event::Declined)?;
        return Err(TransferError::Space(Box::new(e)));
    }

    let prompt_request = PromptRequest {
        peer_name: peer_name.to_string(),
        fingerprint: fingerprint.to_string(),
        file_name: file_name.to_string(),
        size: request.size,
        resume: resume.clone(),
    };

    match decide(incoming, prompt, prompt_request, options.accept_timeout).await? {
        Decision::Accept => {}
        Decision::Decline => {
            // Saying "not now" must not throw away what an earlier session
            // already fetched (ADR-0022).
            refuse(writer, request.transfer_id, RejectReason::Declined).await;
            machine.apply(Event::Declined)?;
            return Err(TransferError::Rejected(RejectReason::Declined));
        }
        Decision::Expired => {
            refuse(writer, request.transfer_id, RejectReason::Expired).await;
            machine.apply(Event::TimedOut)?;
            return Err(TransferError::Rejected(RejectReason::Expired));
        }
    }

    write_message(
        writer,
        &Message::Accept(Accept {
            transfer_id: request.transfer_id,
            have_bitmap: Some(partial.bitmap().encode()),
        }),
    )
    .await?;
    machine.apply(Event::Accepted)?;
    machine.apply(Event::Connected)?;

    let (final_name, received_now) = accept_and_store(
        incoming, writer, machine, request, plan, file_name, partial, options, reporter,
    )
    .await?;

    Ok((final_name, received_now, resume.is_some()))
}

/// Removes a partial's directory, releasing its lock first.
fn discard(partial: &Partial) {
    let dir = partial.dir().to_path_buf();
    let _ = std::fs::remove_dir_all(dir);
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

/// Turns a sender away with `reason` without looking at what it wants to
/// send: reads its request, so the refusal can name the transfer, and answers
/// REJECT. Used by `beam listen` for a second transfer while one is already
/// in progress (ADR-0030). Nothing is created on disk and nobody is asked.
pub async fn turn_away<S>(
    stream: S,
    reason: RejectReason,
    timeout: Duration,
) -> Result<(), TransferError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let request = match tokio::time::timeout(timeout, read_message(&mut reader)).await {
        Ok(Ok(Message::TransferRequest(request))) => request,
        Ok(Ok(other)) => {
            return Err(TransferError::OutOfOrder {
                expected: "TRANSFER_REQUEST",
                got: other.kind_name(),
            });
        }
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(TransferError::Rejected(RejectReason::Expired)),
    };
    refuse(&mut writer, request.transfer_id, reason).await;
    Ok(())
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

/// Looks the sender up in `known_peers`.
///
/// STRENGTHEN IN M6: over the TCP stand-in this only checks that the key the
/// sender *claims* is one we have paired with. Over iroh the caller has already
/// required the claim to equal the key the connection proved
/// (`ReceiveOptions::proven_sender`), which closes the gap; M6 adds the tests
/// that demonstrate it and removes this marker. See ADR-0019 and ADR-0031.
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
    partial: &mut Partial,
    options: &ReceiveOptions,
    reporter: &mut R,
) -> Result<(String, u64), TransferError>
where
    W: AsyncWrite + Unpin,
    R: Reporter,
{
    tokio::fs::create_dir_all(&options.out_dir)
        .await
        .map_err(|e| TransferError::io("create", options.out_dir.display(), e))?;

    let already_had = partial.have_bytes();
    let mut done = already_had;
    let mut route = RouteTracker::new(&options.route);
    let mut received_now = 0u64;

    // Only the chunks that are actually missing. The sender is told the same
    // thing through the have-bitmap in ACCEPT, so the two agree; a sender that
    // sends something else anyway is caught by the out-of-order check.
    let wanted: Vec<u32> = partial.bitmap().missing().collect();
    for index in wanted {
        let (bytes, digest) =
            receive_chunk(incoming, writer, plan, index, options.max_chunk_attempts).await?;

        partial
            .store_chunk(index, &bytes, &digest)
            .await
            .map_err(|e| TransferError::Partial(Box::new(e)))?;

        done += bytes.len() as u64;
        received_now += bytes.len() as u64;
        let (path, before) = route.poll();
        if let Some(from) = before {
            reporter.report(Progress::PathChanged { from, to: path });
        }
        reporter.report(Progress::Transferring {
            done,
            total: plan.size(),
            path,
        });

        write_message(writer, &Message::ChunkAck(ChunkAck { index })).await?;
    }

    let part_path = partial.part_path();

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

    // Every chunk passed its own hash on the way in, but that only says the
    // pieces arrived intact — this says the file is the one that was promised.
    // The caller discards the partial when this fails, because a partial that
    // cannot produce the right file will not produce it next time either.
    verify(&part_path, &request.file_sha256).await?;

    // Reserve the name by creating the file, then move the verified part over
    // it. Nothing is written under the destination name until the contents are
    // known to be right (D-6, D-7).
    let (placeholder, destination, final_name) =
        reserve_destination(&options.out_dir, file_name)
            .map_err(|e| TransferError::io("create a file in", options.out_dir.display(), e))?;
    drop(placeholder);

    // A rename when the partial and the destination share a volume, a copy
    // when they do not — which `--out D:\...` on Windows makes ordinary.
    // See ADR-0023.
    commit(&part_path, &destination)
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

    let _ = already_had;
    Ok((final_name, received_now))
}

/// Collects one chunk, verifying it before it is handed back to be written.
///
/// Returns the bytes and the hash they were checked against, which the partial
/// stores so a later session can check them again.
async fn receive_chunk<W>(
    incoming: &mut Incoming,
    writer: &mut W,
    plan: ChunkPlan,
    index: u32,
    max_attempts: u32,
) -> Result<(Vec<u8>, String), TransferError>
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
            return Err(TransferError::BadRequest(format!(
                "expected chunk {index} next, but the peer sent chunk {}",
                start.index
            )));
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
            None => return Ok((buffer, start.sha256)),
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
