//! Resume: the failure cases M3 exists for, and the rules about what survives.
//!
//! Every test here answers one of two questions. Does the second attempt pick
//! up what the first one left? And does the partial survive exactly when it
//! should, and vanish exactly when it should?

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beam::identity::{Identity, KnownPeers, Peer, encode_public_key};
use beam::transfer::frame::{read_message, write_message};
use beam::transfer::message::{Message, RejectReason, TransferId, TransferRequest};
use beam::transfer::{
    ChunkBitmap, PartialStore, Prompt, PromptRequest, ReceiveOptions, ReceiveSummary, SendOptions,
    SilentReporter, TransferError, receive_file, send_file,
};
use tempfile::TempDir;

/// A chunk size small enough that a modest test file spans several chunks.
const SMALL_CHUNK: u32 = 64 * 1024;

/// A prompt with a fixed answer that remembers what it was shown.
#[derive(Clone)]
struct ScriptedPrompt {
    answer: bool,
    /// How long the "person" takes to answer.
    ///
    /// Zero for most tests. Where a test turns on something arriving *while*
    /// the prompt is open, this has to be non-zero: a prompt that answers in
    /// nanoseconds can win a race that no real person ever would, which makes
    /// the test flaky rather than strict.
    thinking: Duration,
    asked: Arc<Mutex<Vec<PromptRequest>>>,
}

impl ScriptedPrompt {
    fn new(answer: bool) -> Self {
        Self {
            answer,
            thinking: Duration::ZERO,
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A prompt that takes a moment, as a person does.
    fn deliberate(answer: bool) -> Self {
        Self {
            thinking: Duration::from_millis(300),
            ..Self::new(answer)
        }
    }

    fn asked(&self) -> Vec<PromptRequest> {
        self.asked.lock().expect("prompt log").clone()
    }
}

impl Prompt for ScriptedPrompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        self.asked.lock().expect("prompt log").push(request.clone());
        if !self.thinking.is_zero() {
            std::thread::sleep(self.thinking);
        }
        Ok(self.answer)
    }
}

struct World {
    _tmp: TempDir,
    sender: Identity,
    known_peers: KnownPeers,
    out: PathBuf,
    work: PathBuf,
    source: PathBuf,
    payload: Vec<u8>,
}

fn world(payload_len: usize) -> World {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("out");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&out).expect("create out");
    std::fs::create_dir_all(&work).expect("create work");

    let sender = Identity::generate("sender").expect("generate");
    let mut known_peers = KnownPeers::with_header();
    known_peers
        .add(Peer::new("alice", sender.verifying_key()))
        .expect("pair");

    let payload: Vec<u8> = (0..payload_len)
        .map(|i| ((i * 29 + 5) % 251) as u8)
        .collect();
    let source = tmp.path().join("payload.bin");
    std::fs::write(&source, &payload).expect("write source");

    World {
        _tmp: tmp,
        sender,
        known_peers,
        out,
        work,
        source,
        payload,
    }
}

impl World {
    fn receive_options(&self) -> ReceiveOptions {
        let mut options = ReceiveOptions::new(&self.out, &self.work);
        options.accept_timeout = Duration::from_secs(5);
        options
    }

    fn send_options(&self) -> SendOptions {
        let mut options = SendOptions::new(
            &self.source,
            encode_public_key(&self.sender.verifying_key()),
        );
        options.chunk_size = SMALL_CHUNK;
        options.accept_timeout = Duration::from_secs(5);
        options
    }

    fn partials(&self) -> PartialStore {
        PartialStore::new(&self.work)
    }

    /// Runs one whole session and reports both sides' outcomes.
    async fn session(
        &self,
        prompt: ScriptedPrompt,
    ) -> (
        Result<beam::transfer::SendSummary, TransferError>,
        Result<ReceiveSummary, TransferError>,
    ) {
        let (client, server) = tokio::io::duplex(128 * 1024);
        let options = self.send_options();
        let send = tokio::spawn(async move {
            let mut client = client;
            send_file(&mut client, &options, &mut SilentReporter).await
        });

        let received = receive_file(
            server,
            &self.known_peers,
            &self.receive_options(),
            prompt,
            &mut SilentReporter,
            &mut HashSet::new(),
        )
        .await;

        (send.await.expect("sender task"), received)
    }

    /// Runs a session that stops after `chunks` chunks, leaving a partial.
    ///
    /// The sender here is written by hand rather than being `send_file` cut
    /// short by a timer. An in-memory pipe moves a few hundred kilobytes in
    /// microseconds, so a timed interruption is a race the test loses often
    /// enough to be useless: sometimes the transfer simply finishes first.
    /// Sending exactly the chunks we mean to send, and then hanging up, always
    /// leaves the same partial behind.
    async fn partial_session(&self, chunks: u32) {
        let (mut client, server) = tokio::io::duplex(128 * 1024);
        let known_peers = self.known_peers.clone();
        let options = self.receive_options();

        let receiver = tokio::spawn(async move {
            receive_file(
                server,
                &known_peers,
                &options,
                ScriptedPrompt::new(true),
                &mut SilentReporter,
                &mut HashSet::new(),
            )
            .await
        });

        let plan = beam::transfer::ChunkPlan::new(self.payload.len() as u64, SMALL_CHUNK);
        let request = TransferRequest {
            transfer_id: TransferId::generate().expect("id"),
            sender_public_key: encode_public_key(&self.sender.verifying_key()),
            file_name: "payload.bin".to_string(),
            size: self.payload.len() as u64,
            chunk_size: SMALL_CHUNK,
            chunk_count: plan.chunk_count(),
            file_sha256: beam::transfer::sha256_hex(&self.payload),
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write request");

        match read_message(&mut client).await.expect("read accept") {
            Message::Accept(_) => {}
            other => panic!("unexpected {}", other.kind_name()),
        }

        for index in 0..chunks.min(plan.chunk_count()) {
            let offset = plan.offset_of(index) as usize;
            let body = &self.payload[offset..offset + plan.len_of(index) as usize];
            write_message(
                &mut client,
                &Message::ChunkStart(beam::transfer::ChunkStart {
                    index,
                    len: body.len() as u32,
                    sha256: beam::transfer::sha256_hex(body),
                }),
            )
            .await
            .expect("write chunk start");
            for slice in body.chunks(beam::transfer::MAX_CHUNK_DATA) {
                write_message(
                    &mut client,
                    &Message::ChunkData {
                        index,
                        bytes: slice.to_vec(),
                    },
                )
                .await
                .expect("write chunk data");
            }
            match read_message(&mut client).await.expect("read ack") {
                Message::ChunkAck(ack) => assert_eq!(ack.index, index),
                other => panic!("unexpected {}", other.kind_name()),
            }
        }

        // Hanging up mid-transfer, the way a killed process would.
        drop(client);
        let outcome = receiver.await.expect("receiver task");
        assert!(
            outcome.is_err(),
            "the cut-short session reported success: {outcome:?}"
        );
    }
}

fn only_partial(world: &World) -> beam::transfer::PartialSummary {
    let mut list = world
        .partials()
        .list(Duration::from_secs(3600))
        .expect("list partials");
    assert_eq!(list.len(), 1, "expected exactly one partial, got {list:?}");
    list.remove(0)
}

fn partial_count(world: &World) -> usize {
    world
        .partials()
        .list(Duration::from_secs(3600))
        .expect("list partials")
        .len()
}

// ---------------------------------------------------------------------------
// The required failure cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_interrupted_transfer_resumes_and_sends_only_what_is_missing() {
    let world = world(SMALL_CHUNK as usize * 8);

    world.partial_session(3).await;

    let partial = only_partial(&world);
    assert!(
        partial.have_chunks >= 3 && partial.have_chunks < 8,
        "expected a genuinely partial transfer, got {}/{} chunks",
        partial.have_chunks,
        partial.chunk_count
    );
    let carried_over = partial.have_bytes;

    let prompt = ScriptedPrompt::new(true);
    let (sent, received) = world.session(prompt.clone()).await;
    let received = received.expect("second receive");
    let sent = sent.expect("second send");

    assert_eq!(prompt.asked().len(), 1, "a resume must still be accepted");
    assert!(
        prompt.asked()[0].resume.is_some(),
        "the prompt did not say this was a resume"
    );
    assert!(received.resumed);
    assert_eq!(
        sent.bytes_skipped, carried_over,
        "the sender re-sent bytes the receiver already had"
    );
    assert!(
        sent.bytes_sent < world.payload.len() as u64,
        "the whole file was sent again"
    );
    assert_eq!(
        std::fs::read(world.out.join("payload.bin")).expect("read result"),
        world.payload
    );
    assert_eq!(
        partial_count(&world),
        0,
        "the partial outlived the transfer"
    );
}

#[tokio::test]
async fn a_corrupted_stored_chunk_is_re_fetched_rather_than_trusted() {
    let world = world(SMALL_CHUNK as usize * 6);
    world.partial_session(3).await;

    // Reach into the partial and damage a chunk that it believes is good,
    // standing in for a bad sector or a stray edit between sessions.
    let partial = only_partial(&world);
    let part = world.work.join(&partial.id).join("part");
    let mut bytes = std::fs::read(&part).expect("read part");
    bytes[10] ^= 0xff;
    std::fs::write(&part, &bytes).expect("corrupt part");
    let before = partial.have_chunks;

    let (sent, received) = world.session(ScriptedPrompt::new(true)).await;
    sent.expect("send");
    received.expect("receive");

    assert_eq!(
        std::fs::read(world.out.join("payload.bin")).expect("read result"),
        world.payload,
        "a corrupted stored chunk reached the finished file"
    );
    assert!(before > 0);
}

#[tokio::test]
async fn a_changed_source_file_starts_over_instead_of_resuming() {
    let world = world(SMALL_CHUNK as usize * 6);
    world.partial_session(3).await;
    let first = only_partial(&world);

    // Editing the file changes its hash, so this is a different transfer.
    // Continuing the old one would splice two files together.
    let mut changed = world.payload.clone();
    changed[0] ^= 0xff;
    std::fs::write(&world.source, &changed).expect("change the source");

    let prompt = ScriptedPrompt::new(true);
    let (sent, received) = world.session(prompt.clone()).await;
    let sent = sent.expect("send");
    received.expect("receive");

    assert_eq!(
        sent.bytes_skipped, 0,
        "a changed file reused chunks from the old one"
    );
    assert!(
        prompt.asked()[0].resume.is_none(),
        "a changed file was presented as a resume"
    );
    assert_eq!(
        std::fs::read(world.out.join("payload.bin")).expect("read result"),
        changed
    );

    // The old partial is still there: it belongs to the file as it was, and
    // nothing has said that file is unwanted.
    let left = world
        .partials()
        .list(Duration::from_secs(3600))
        .expect("list");
    assert!(
        left.iter().any(|p| p.id == first.id),
        "the original partial was destroyed by an unrelated transfer"
    );
}

#[tokio::test]
async fn a_malformed_have_bitmap_aborts_the_sender() {
    // Three bitmaps a receiver must never get away with: too short, too long,
    // and one claiming chunks past the end of the transfer.
    let world = world(SMALL_CHUNK as usize * 5);
    let chunk_count = 5u32;

    let mut too_many_bits = vec![0u8; ChunkBitmap::byte_len(chunk_count)];
    *too_many_bits.last_mut().expect("a byte") = 0xff;
    let padded = base64_encode(&too_many_bits);

    for (name, bitmap) in [
        ("too short", base64_encode(&[])),
        ("too long", base64_encode(&[0u8; 4])),
        ("bits past the end", padded),
        ("not base64", "!!!not base64!!!".to_string()),
    ] {
        let (client, mut server) = tokio::io::duplex(128 * 1024);
        let options = world.send_options();
        let send = tokio::spawn(async move {
            let mut client = client;
            send_file(&mut client, &options, &mut SilentReporter).await
        });

        let transfer_id = match read_message(&mut server).await.expect("read request") {
            Message::TransferRequest(request) => {
                assert_eq!(request.chunk_count, chunk_count);
                request.transfer_id
            }
            other => panic!("unexpected {}", other.kind_name()),
        };
        write_message(
            &mut server,
            &Message::Accept(beam::transfer::Accept {
                transfer_id,
                have_bitmap: Some(bitmap),
            }),
        )
        .await
        .expect("write accept");

        let outcome = send.await.expect("sender task");
        assert!(
            matches!(outcome, Err(TransferError::Bitmap(_))),
            "{name}: expected a bitmap error, got {outcome:?}"
        );
    }
}

#[tokio::test]
async fn a_partial_is_never_offered_to_a_different_peer() {
    // The realistic attack: carol knows the file and asks for exactly the same
    // thing, hoping to be handed alice's partial — or to learn how much of it
    // exists. She is refused at the known_peers check, before any of that.
    let world = world(SMALL_CHUNK as usize * 6);
    world.partial_session(3).await;
    let before = only_partial(&world);

    let carol = Identity::generate("carol").expect("generate");
    let prompt = ScriptedPrompt::new(true);

    let (client, server) = tokio::io::duplex(128 * 1024);
    let mut options = SendOptions::new(&world.source, encode_public_key(&carol.verifying_key()));
    options.chunk_size = SMALL_CHUNK;
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });

    let received = receive_file(
        server,
        &world.known_peers,
        &world.receive_options(),
        prompt.clone(),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;

    assert!(
        matches!(
            received,
            Err(TransferError::Rejected(RejectReason::UnknownPeer))
        ),
        "expected UnknownPeer, got {received:?}"
    );
    assert!(
        prompt.asked().is_empty(),
        "an unknown peer reached the prompt"
    );
    assert!(
        matches!(
            send.await.expect("task"),
            Err(TransferError::Rejected(RejectReason::UnknownPeer))
        ),
        "the stranger was not told why"
    );

    let after = only_partial(&world);
    assert_eq!(
        after.have_chunks, before.have_chunks,
        "a stranger's request disturbed the partial"
    );
}

#[tokio::test]
async fn a_second_session_for_the_same_partial_is_refused() {
    let world = world(SMALL_CHUNK as usize * 6);
    world.partial_session(3).await;

    // Hold the partial the way a running session would.
    let partial = only_partial(&world);
    let key = beam::transfer::PartialKey {
        peer_fingerprint: world.sender.fingerprint(),
        file_sha256: beam::transfer::sha256_hex(&world.payload),
        size: world.payload.len() as u64,
        chunk_size: SMALL_CHUNK,
    };
    let plan = beam::transfer::ChunkPlan::new(world.payload.len() as u64, SMALL_CHUNK);
    let _held = world
        .partials()
        .open(&key, "payload.bin", plan)
        .await
        .expect("hold the partial");

    let prompt = ScriptedPrompt::new(true);
    let (sent, received) = world.session(prompt.clone()).await;

    assert!(
        matches!(received, Err(TransferError::PartialInUse)),
        "expected PartialInUse, got {received:?}"
    );
    assert!(
        prompt.asked().is_empty(),
        "a blocked transfer still interrupted somebody"
    );
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Busy))),
        "the sender was not told the peer was busy, got {sent:?}"
    );

    let after = only_partial(&world);
    assert_eq!(
        after.have_chunks, partial.have_chunks,
        "the blocked session damaged the partial it could not have"
    );
}

// ---------------------------------------------------------------------------
// Batched flushing (ADR-0039)
// ---------------------------------------------------------------------------

/// Opens (or reopens) the partial for `payload` from the world's sender.
async fn open_partial(world: &World, chunk_size: u32) -> beam::transfer::Partial {
    let key = beam::transfer::PartialKey {
        peer_fingerprint: world.sender.fingerprint(),
        file_sha256: beam::transfer::sha256_hex(&world.payload),
        size: world.payload.len() as u64,
        chunk_size,
    };
    let plan = beam::transfer::ChunkPlan::new(world.payload.len() as u64, chunk_size);
    world
        .partials()
        .open(&key, "payload.bin", plan)
        .await
        .expect("open the partial")
}

async fn store(world: &World, partial: &mut beam::transfer::Partial, chunk_size: u32, index: u32) {
    let plan = beam::transfer::ChunkPlan::new(world.payload.len() as u64, chunk_size);
    let offset = plan.offset_of(index) as usize;
    let body = &world.payload[offset..offset + plan.len_of(index) as usize];
    partial
        .store_chunk(index, body, &beam::transfer::sha256_hex(body))
        .await
        .expect("store a chunk");
}

/// beam killed between two batched flushes loses nothing: each chunk was
/// recorded as soon as it was written, and the operating system keeps what
/// the process wrote. (Here the "kill" is dropping the partial unflushed.)
#[tokio::test]
async fn a_killed_receiver_keeps_chunks_it_had_not_flushed() {
    let world = world(SMALL_CHUNK as usize * 6);

    let mut partial = open_partial(&world, SMALL_CHUNK).await;
    for index in 0..3 {
        store(&world, &mut partial, SMALL_CHUNK, index).await;
    }
    assert_eq!(partial.unflushed_chunks(), 3);
    drop(partial); // killed: no flush

    let mut partial = open_partial(&world, SMALL_CHUNK).await;
    assert_eq!(partial.bitmap().count(), 3);
    assert_eq!(partial.reverify().await.expect("reverify"), 0);
    assert_eq!(partial.bitmap().count(), 3, "a recorded chunk was lost");
}

/// After power loss, chunks that were recorded but never reached the disk are
/// not trusted: `reverify` re-hashes them, drops them, and they are asked for
/// again. Simulated by recording chunks, then emptying the data file the way
/// a lost cache would leave it.
#[tokio::test]
async fn chunks_lost_to_power_failure_are_dropped_not_trusted() {
    let world = world(SMALL_CHUNK as usize * 6);

    let mut partial = open_partial(&world, SMALL_CHUNK).await;
    for index in 0..3 {
        store(&world, &mut partial, SMALL_CHUNK, index).await;
    }
    let part = partial.part_path();
    drop(partial);
    // The data never made it to disk; the record of it did.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&part)
        .expect("open part")
        .set_len(SMALL_CHUNK as u64) // chunk 0 survived, 1 and 2 did not
        .expect("truncate");

    let mut partial = open_partial(&world, SMALL_CHUNK).await;
    assert_eq!(partial.bitmap().count(), 3, "the record survived");
    assert_eq!(partial.reverify().await.expect("reverify"), 2);
    assert!(partial.bitmap().get(0));
    assert!(!partial.bitmap().get(1) && !partial.bitmap().get(2));

    // And a whole session over it still produces the right file.
    drop(partial);
    let (sent, received) = world.session(ScriptedPrompt::new(true)).await;
    sent.expect("send");
    received.expect("receive");
    assert_eq!(
        std::fs::read(world.out.join("payload.bin")).expect("read result"),
        world.payload
    );
}

/// A batch is flushed by itself once `FLUSH_EVERY_CHUNKS` chunks are waiting,
/// which bounds what power loss can cost.
#[tokio::test]
async fn a_full_batch_of_chunks_is_flushed_without_being_asked() {
    let batch = beam::transfer::FLUSH_EVERY_CHUNKS;
    let world = world(SMALL_CHUNK as usize * (batch + 2));

    let mut partial = open_partial(&world, SMALL_CHUNK).await;
    for index in 0..batch as u32 {
        store(&world, &mut partial, SMALL_CHUNK, index).await;
    }
    assert_eq!(
        partial.unflushed_chunks(),
        0,
        "a full batch was not flushed"
    );
    store(&world, &mut partial, SMALL_CHUNK, batch as u32).await;
    assert_eq!(partial.unflushed_chunks(), 1);
    partial.flush().await.expect("flush");
    assert_eq!(partial.unflushed_chunks(), 0);
}

/// Large chunks are batched by size instead: `FLUSH_EVERY` bytes is a batch
/// even when that is fewer than `FLUSH_EVERY_CHUNKS` chunks.
#[tokio::test]
async fn a_full_batch_of_bytes_is_flushed_without_being_asked() {
    let chunk = 16 * 1024 * 1024;
    let batch = (beam::transfer::FLUSH_EVERY / u64::from(chunk)) as u32;
    assert!((batch as usize) < beam::transfer::FLUSH_EVERY_CHUNKS);
    let world = world(chunk as usize * (batch as usize + 1));

    let mut partial = open_partial(&world, chunk).await;
    for index in 0..batch - 1 {
        store(&world, &mut partial, chunk, index).await;
    }
    assert_eq!(partial.unflushed_chunks(), batch as usize - 1);
    store(&world, &mut partial, chunk, batch - 1).await;
    assert_eq!(
        partial.unflushed_chunks(),
        0,
        "a full batch was not flushed"
    );
}

// ---------------------------------------------------------------------------
// Retention: what survives a failure, and what does not
// ---------------------------------------------------------------------------

#[tokio::test]
async fn declining_a_resume_keeps_the_partial_for_next_time() {
    let world = world(SMALL_CHUNK as usize * 6);
    world.partial_session(3).await;
    let before = only_partial(&world);

    let prompt = ScriptedPrompt::new(false);
    let (_sent, received) = world.session(prompt.clone()).await;
    assert!(matches!(
        received,
        Err(TransferError::Rejected(RejectReason::Declined))
    ));
    assert!(
        prompt.asked()[0].resume.is_some(),
        "the person was not told this was a resume"
    );

    let after = only_partial(&world);
    assert_eq!(
        after.have_chunks, before.have_chunks,
        "saying 'not now' threw away what was already received"
    );

    // And it really is still usable.
    let (sent, received) = world.session(ScriptedPrompt::new(true)).await;
    assert_eq!(sent.expect("send").bytes_skipped, before.have_bytes);
    received.expect("receive");
    assert_eq!(
        std::fs::read(world.out.join("payload.bin")).expect("read result"),
        world.payload
    );
}

#[tokio::test]
async fn declining_a_fresh_transfer_leaves_nothing_behind() {
    // Nothing was received, so there is nothing worth keeping, and an empty
    // directory would only clutter `beam transfers`.
    let world = world(SMALL_CHUNK as usize * 3);
    let (_sent, received) = world.session(ScriptedPrompt::new(false)).await;
    assert!(matches!(
        received,
        Err(TransferError::Rejected(RejectReason::Declined))
    ));
    assert_eq!(partial_count(&world), 0);
}

#[tokio::test]
async fn a_finished_transfer_leaves_no_partial() {
    let world = world(SMALL_CHUNK as usize * 3);
    let (sent, received) = world.session(ScriptedPrompt::new(true)).await;
    sent.expect("send");
    received.expect("receive");
    assert_eq!(partial_count(&world), 0);
}

#[tokio::test]
async fn data_before_accept_cannot_destroy_an_existing_partial() {
    // A peer that jumps the gun gets its transfer thrown out (S-4) — but the
    // bytes an earlier, well-behaved session collected are not its to delete.
    let world = world(SMALL_CHUNK as usize * 6);
    world.partial_session(3).await;
    let before = only_partial(&world);

    let (mut client, server) = tokio::io::duplex(128 * 1024);
    let sender_key = encode_public_key(&world.sender.verifying_key());
    let payload = world.payload.clone();
    let rude = tokio::spawn(async move {
        let plan = beam::transfer::ChunkPlan::new(payload.len() as u64, SMALL_CHUNK);
        let request = TransferRequest {
            transfer_id: TransferId::generate().expect("id"),
            sender_public_key: sender_key,
            file_name: "payload.bin".to_string(),
            size: payload.len() as u64,
            chunk_size: SMALL_CHUNK,
            chunk_count: plan.chunk_count(),
            file_sha256: beam::transfer::sha256_hex(&payload),
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write request");
        write_message(
            &mut client,
            &Message::ChunkStart(beam::transfer::ChunkStart {
                index: 0,
                len: SMALL_CHUNK,
                sha256: beam::transfer::sha256_hex(&payload[..SMALL_CHUNK as usize]),
            }),
        )
        .await
        .expect("write chunk start");
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let received = receive_file(
        server,
        &world.known_peers,
        &world.receive_options(),
        ScriptedPrompt::deliberate(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;
    rude.abort();

    assert!(
        matches!(received, Err(TransferError::DataBeforeAccept)),
        "expected DataBeforeAccept, got {received:?}"
    );
    let after = only_partial(&world);
    assert_eq!(
        after.have_chunks, before.have_chunks,
        "a rude peer destroyed somebody else's progress"
    );
}

#[tokio::test]
async fn an_empty_transfer_still_completes() {
    // Zero chunks means an empty bitmap, which is already complete. Worth its
    // own test because every "is it finished" question has a degenerate case.
    let world = world(0);
    let (sent, received) = world.session(ScriptedPrompt::new(true)).await;
    assert_eq!(sent.expect("send").bytes_sent, 0);
    received.expect("receive");
    assert_eq!(
        std::fs::read(world.out.join("payload.bin")).expect("read result"),
        Vec::<u8>::new()
    );
    assert_eq!(partial_count(&world), 0);
}

/// Retention, the case that has to delete: the pieces all arrive intact, and
/// the file they make is still not the one that was promised.
///
/// Each chunk passes its own hash, so nothing is caught on the way in. Only the
/// whole-file hash catches it, and by then a complete partial exists. Keeping
/// it would guarantee that the next session re-checks the same bytes and fails
/// in exactly the same way, so it goes.
#[tokio::test]
async fn a_file_that_fails_its_final_hash_is_discarded_and_never_written() {
    let world = world(SMALL_CHUNK as usize * 4);

    let (mut client, server) = tokio::io::duplex(128 * 1024);
    let known_peers = world.known_peers.clone();
    let options = world.receive_options();
    let receiver = tokio::spawn(async move {
        receive_file(
            server,
            &known_peers,
            &options,
            ScriptedPrompt::new(true),
            &mut SilentReporter,
            &mut HashSet::new(),
        )
        .await
    });

    let plan = beam::transfer::ChunkPlan::new(world.payload.len() as u64, SMALL_CHUNK);
    let transfer_id = TransferId::generate().expect("id");

    // The lie: a whole-file hash that belongs to different contents. Every
    // chunk hash below is honest, so the receiver has no reason to object until
    // it checks the assembled file.
    let promised = beam::transfer::sha256_hex(b"some other file entirely");
    assert_ne!(promised, beam::transfer::sha256_hex(&world.payload));

    write_message(
        &mut client,
        &Message::TransferRequest(TransferRequest {
            transfer_id,
            sender_public_key: encode_public_key(&world.sender.verifying_key()),
            file_name: "payload.bin".to_string(),
            size: world.payload.len() as u64,
            chunk_size: SMALL_CHUNK,
            chunk_count: plan.chunk_count(),
            file_sha256: promised,
        }),
    )
    .await
    .expect("write request");

    match read_message(&mut client).await.expect("read accept") {
        Message::Accept(_) => {}
        other => panic!("unexpected {}", other.kind_name()),
    }

    for index in 0..plan.chunk_count() {
        let offset = plan.offset_of(index) as usize;
        let body = &world.payload[offset..offset + plan.len_of(index) as usize];
        write_message(
            &mut client,
            &Message::ChunkStart(beam::transfer::ChunkStart {
                index,
                len: body.len() as u32,
                sha256: beam::transfer::sha256_hex(body),
            }),
        )
        .await
        .expect("write chunk start");
        for slice in body.chunks(beam::transfer::MAX_CHUNK_DATA) {
            write_message(
                &mut client,
                &Message::ChunkData {
                    index,
                    bytes: slice.to_vec(),
                },
            )
            .await
            .expect("write chunk data");
        }
        match read_message(&mut client).await.expect("read ack") {
            // Every chunk is accepted: the damage is invisible piece by piece.
            Message::ChunkAck(ack) => assert_eq!(ack.index, index),
            other => panic!("chunk {index} was not accepted: {}", other.kind_name()),
        }
    }

    write_message(
        &mut client,
        &Message::Complete(beam::transfer::Complete {
            transfer_id,
            final_name: None,
        }),
    )
    .await
    .expect("write complete");

    let outcome = receiver.await.expect("receiver task");
    assert!(
        matches!(outcome, Err(TransferError::VerificationFailed)),
        "expected VerificationFailed, got {outcome:?}"
    );

    assert_eq!(
        partial_count(&world),
        0,
        "a partial that cannot produce the promised file was kept"
    );
    let left_behind: Vec<String> = std::fs::read_dir(&world.out)
        .expect("read out")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        left_behind.is_empty(),
        "the destination gained {left_behind:?} from a file that failed its hash"
    );
    assert!(
        !world.work.join("payload.bin").exists(),
        "the part file was left where the partial used to be"
    );
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
