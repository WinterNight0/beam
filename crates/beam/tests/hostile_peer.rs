//! A peer that is paired — so it gets past S-7 — and then misbehaves.
//!
//! Pairing says who someone is, not that they are well-behaved. These tests
//! give the engine a paired peer that lies about sizes, sends oversized or
//! malformed frames, or simply stops talking, and check that each is refused
//! before anyone is asked, or given up on without holding anything for ever.
//! See `docs/threat-model.md`, actor "paired but malicious peer", and ADR-0033.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beam::identity::{Identity, KnownPeers, Peer, encode_public_key};
use beam::transfer::frame::{read_message, write_message};
use beam::transfer::message::{
    ChunkStart, MAX_CHUNK_COUNT, MAX_CHUNK_SIZE, Message, RejectReason, TransferId, TransferRequest,
};
use beam::transfer::{
    Progress, Prompt, PromptRequest, ReceiveOptions, Reporter, SendOptions, SilentReporter,
    TransferError, receive_file, send_file, sha256_hex,
};
use tokio::io::{AsyncWriteExt, DuplexStream, duplex};

/// Answers yes, and records that it was asked.
#[derive(Clone, Default)]
struct Yes(Arc<Mutex<usize>>);

impl Prompt for Yes {
    fn confirm(&mut self, _: &PromptRequest) -> std::io::Result<bool> {
        *self.0.lock().unwrap() += 1;
        Ok(true)
    }
}

impl Yes {
    fn asked(&self) -> usize {
        *self.0.lock().unwrap()
    }
}

struct Setup {
    _tmp: tempfile::TempDir,
    sender: Identity,
    known: KnownPeers,
    options: ReceiveOptions,
    files: PathBuf,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Identity::generate("mallory").unwrap();
    let mut known = KnownPeers::with_header();
    known
        .add(Peer::new("mallory", sender.verifying_key()))
        .unwrap();
    let out = tmp.path().join("out");
    let files = tmp.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let mut options = ReceiveOptions::new(&out, tmp.path().join("work"));
    options.accept_timeout = Duration::from_secs(5);
    options.stall_timeout = Duration::from_millis(300);
    Setup {
        _tmp: tmp,
        sender,
        known,
        options,
        files,
    }
}

fn request(s: &Setup, size: u64, chunk_size: u32, chunk_count: u32) -> Message {
    Message::TransferRequest(TransferRequest {
        transfer_id: TransferId::generate().unwrap(),
        sender_public_key: encode_public_key(&s.sender.verifying_key()),
        file_name: "x.bin".into(),
        size,
        chunk_size,
        chunk_count,
        file_sha256: sha256_hex(b"whatever"),
    })
}

/// Runs the receiver against a hand-written peer.
async fn receive(s: &Setup, stream: DuplexStream, prompt: Yes) -> Result<(), TransferError> {
    receive_file(
        stream,
        &s.known,
        &s.options,
        prompt,
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await
    .map(|_| ())
}

async fn reply(peer: &mut DuplexStream) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(5), read_message(peer))
        .await
        .ok()?
        .ok()
}

// -------------------------------------------------------------- absurd sizes

/// The largest file a request can describe — 64 TiB, in 2^22 chunks of
/// 16 MiB — so the plan is valid and only the free-space check stands in the
/// way. It refuses, before anything is written and before anybody is asked
/// (N-7).
#[tokio::test]
async fn a_sixty_four_tib_request_is_refused_for_space_without_a_prompt() {
    let s = setup();
    let (mut peer, ours) = duplex(64 * 1024);
    let size = u64::from(MAX_CHUNK_SIZE) * u64::from(MAX_CHUNK_COUNT);
    let count = MAX_CHUNK_COUNT;
    write_message(&mut peer, &request(&s, size, MAX_CHUNK_SIZE, count))
        .await
        .unwrap();
    let prompt = Yes::default();
    let started = std::time::Instant::now();
    let result = receive(&s, ours, prompt.clone()).await;

    assert!(matches!(result, Err(TransferError::Space(_))), "{result:?}");
    assert!(matches!(
        reply(&mut peer).await,
        Some(Message::Reject(r)) if r.reason == RejectReason::NoSpace
    ));
    assert_eq!(prompt.asked(), 0);
    // Refused before any per-chunk state was written for it.
    let work = s.options.tmp_dir.clone();
    let entries = std::fs::read_dir(&work).map(|d| d.count()).unwrap_or(0);
    assert_eq!(
        entries, 0,
        "a partial was created for a request that cannot fit"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

/// A modest size in one-byte chunks: a billion chunks' worth of state.
#[tokio::test]
async fn a_request_with_too_many_chunks_is_refused() {
    let s = setup();
    let (mut peer, ours) = duplex(64 * 1024);
    write_message(&mut peer, &request(&s, 1 << 30, 1, 1 << 30))
        .await
        .unwrap();
    let prompt = Yes::default();
    let result = receive(&s, ours, prompt.clone()).await;
    assert!(matches!(result, Err(TransferError::Plan(_))), "{result:?}");
    assert_eq!(prompt.asked(), 0);
}

/// `u64::MAX` bytes in one-byte chunks: the chunk count would wrap to a small
/// `u32` the peer could simply declare. Refused as malformed.
#[tokio::test]
async fn a_size_whose_chunk_count_would_wrap_is_refused() {
    let s = setup();
    let (mut peer, ours) = duplex(64 * 1024);
    write_message(&mut peer, &request(&s, u64::MAX, 1, u64::MAX as u32))
        .await
        .unwrap();
    let prompt = Yes::default();
    let result = receive(&s, ours, prompt.clone()).await;
    assert!(matches!(result, Err(TransferError::Plan(_))), "{result:?}");
    assert_eq!(prompt.asked(), 0);
}

/// A chunk size over the 16 MiB cap would make the receiver hold that much
/// in memory per chunk. Refused before anything is allocated or asked.
#[tokio::test]
async fn a_chunk_size_over_the_cap_is_refused() {
    let s = setup();
    for chunk_size in [MAX_CHUNK_SIZE + 1, u32::MAX] {
        let (mut peer, ours) = duplex(64 * 1024);
        let size = u64::from(chunk_size);
        write_message(&mut peer, &request(&s, size, chunk_size, 1))
            .await
            .unwrap();
        let prompt = Yes::default();
        let result = receive(&s, ours, prompt.clone()).await;
        assert!(
            matches!(result, Err(TransferError::Plan(_))),
            "{chunk_size}: {result:?}"
        );
        assert_eq!(prompt.asked(), 0);
    }
}

// ------------------------------------------------------ oversized / malformed

/// A frame header claiming a gigabyte is refused from the header alone: the
/// payload is never read, so nothing that size is ever allocated.
#[tokio::test]
async fn an_oversized_frame_is_refused_from_its_header() {
    let s = setup();
    let (mut peer, ours) = duplex(64 * 1024);
    let mut header = vec![1u8]; // TRANSFER_REQUEST
    header.extend_from_slice(&(1u32 << 30).to_be_bytes());
    peer.write_all(&header).await.unwrap();
    // Deliberately no payload: if the receiver tried to read it, it would
    // hang until the accept timeout instead of failing at once.
    let started = std::time::Instant::now();
    let result = receive(&s, ours, Yes::default()).await;
    assert!(matches!(result, Err(TransferError::Frame(_))), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn malformed_frames_are_refused_without_a_prompt() {
    let s = setup();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("an unknown frame type", frame(0x7f, b"{}")),
        ("JSON that is not JSON", frame(1, b"{not json")),
        (
            "a request with an extra field",
            frame(1, br#"{"transfer_id":"00112233445566778899aabbccddeeff","sender_public_key":"x","file_name":"a","size":1,"chunk_size":1,"chunk_count":1,"file_sha256":"00","auto_accept":true}"#),
        ),
        ("a request missing a field", frame(1, br#"{"file_name":"a"}"#)),
        ("a chunk before any request", frame(4, br#"{"index":0,"len":1,"sha256":"00"}"#)),
    ];
    for (what, bytes) in cases {
        let (mut peer, ours) = duplex(64 * 1024);
        peer.write_all(&bytes).await.unwrap();
        let prompt = Yes::default();
        let result = receive(&s, ours, prompt.clone()).await;
        assert!(result.is_err(), "{what} was accepted");
        assert_eq!(prompt.asked(), 0, "{what} reached the prompt");
    }
}

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind];
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

// ------------------------------------------------------------ holding still

/// Accepted, then silent: the receiver gives up after the stall timeout
/// rather than holding the transfer slot for ever.
#[tokio::test]
async fn a_sender_that_goes_quiet_after_accept_is_given_up_on() {
    let s = setup();
    let (mut peer, ours) = duplex(64 * 1024);
    write_message(&mut peer, &request(&s, 10, 10, 1))
        .await
        .unwrap();
    let receiving = tokio::spawn({
        let known = s.known.clone();
        let options = s.options.clone();
        async move {
            receive_file(
                ours,
                &known,
                &options,
                Yes::default(),
                &mut SilentReporter,
                &mut HashSet::new(),
            )
            .await
        }
    });
    assert!(matches!(reply(&mut peer).await, Some(Message::Accept(_))));
    // ...and then nothing, with the stream still open.
    let result = tokio::time::timeout(Duration::from_secs(5), receiving)
        .await
        .expect("the receiver waited for ever")
        .unwrap();
    assert!(
        matches!(result, Err(TransferError::Stalled { .. })),
        "{result:?}"
    );
    drop(peer);
}

/// The same, half-way through a chunk.
#[tokio::test]
async fn a_sender_that_goes_quiet_mid_chunk_is_given_up_on() {
    let s = setup();
    let (mut peer, ours) = duplex(64 * 1024);
    write_message(&mut peer, &request(&s, 10, 10, 1))
        .await
        .unwrap();
    let receiving = tokio::spawn({
        let known = s.known.clone();
        let options = s.options.clone();
        async move {
            receive_file(
                ours,
                &known,
                &options,
                Yes::default(),
                &mut SilentReporter,
                &mut HashSet::new(),
            )
            .await
        }
    });
    assert!(matches!(reply(&mut peer).await, Some(Message::Accept(_))));
    write_message(
        &mut peer,
        &Message::ChunkStart(ChunkStart {
            index: 0,
            len: 10,
            sha256: sha256_hex(&[0u8; 10]),
        }),
    )
    .await
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), receiving)
        .await
        .expect("the receiver waited for ever")
        .unwrap();
    assert!(
        matches!(result, Err(TransferError::Stalled { .. })),
        "{result:?}"
    );
    drop(peer);
}

/// And the other way round: a receiver that accepts and then never
/// acknowledges does not hold the sender for ever either.
#[tokio::test]
async fn a_receiver_that_never_acknowledges_is_given_up_on() {
    let s = setup();
    let path = s.files.join("a.bin");
    std::fs::write(&path, [7u8; 100]).unwrap();
    let (mut ours, mut peer) = duplex(64 * 1024);
    let mut options = SendOptions::new(&path, encode_public_key(&s.sender.verifying_key()));
    options.stall_timeout = Duration::from_millis(300);
    let sending =
        tokio::spawn(async move { send_file(&mut ours, &options, &mut SilentReporter).await });

    let Some(Message::TransferRequest(req)) = reply(&mut peer).await else {
        panic!("no request")
    };
    write_message(
        &mut peer,
        &Message::Accept(beam::transfer::message::Accept {
            transfer_id: req.transfer_id,
            have_bitmap: None,
        }),
    )
    .await
    .unwrap();
    // Read what it sends, but never answer.
    let result = tokio::time::timeout(Duration::from_secs(5), sending)
        .await
        .expect("the sender waited for ever")
        .unwrap();
    assert!(
        matches!(result, Err(TransferError::Stalled { .. })),
        "{result:?}"
    );
    drop(peer);
}

// -------------------------------------------------------- slow verification

#[derive(Default)]
struct Log(Vec<Progress>);

impl Reporter for Log {
    fn report(&mut self, progress: Progress) {
        self.0.push(progress);
    }
}

/// Runs a real transfer whose final verification takes about a second,
/// against a sender whose stall timeout is 300 ms.
async fn slow_verification(
    keepalive_every: Duration,
) -> (Result<(), TransferError>, Vec<Progress>) {
    let s = setup();
    // 12 MiB, so verification reads 12 blocks, each followed by a 80 ms pause.
    let bytes: Vec<u8> = (0..12 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let path = s.files.join("big.bin");
    std::fs::write(&path, &bytes).unwrap();

    let (mut client, server) = duplex(1024 * 1024);
    let mut send_options = SendOptions::new(&path, encode_public_key(&s.sender.verifying_key()));
    send_options.stall_timeout = Duration::from_millis(300);
    let sending = tokio::spawn(async move {
        let mut log = Log::default();
        let result = send_file(&mut client, &send_options, &mut log).await;
        (result.map(|_| ()), log.0)
    });

    let mut options = s.options.clone();
    options.stall_timeout = Duration::from_secs(5);
    options.verify_pause = Duration::from_millis(80);
    options.keepalive_every = keepalive_every;
    let _ = receive_file(
        server,
        &s.known,
        &options,
        Yes::default(),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;
    sending.await.unwrap()
}

/// M6 answer 3: the stall timeout must not fire while the receiver verifies a
/// large file. Verification here takes about three times the sender's stall
/// timeout, and the transfer still completes, because the receiver says it is
/// verifying.
#[tokio::test]
async fn a_slow_final_verification_does_not_trip_the_stall_timeout() {
    let (result, progress) = slow_verification(Duration::from_millis(50)).await;
    result.expect("the transfer should survive a slow verification");
    let told = progress
        .iter()
        .filter(|p| matches!(p, Progress::PeerVerifying { .. }))
        .count();
    assert!(
        told >= 3,
        "the sender heard {told} keep-alives: {progress:?}"
    );
}

/// The control for the test above: with keep-alives too rare, the same slow
/// verification does trip the sender's stall timeout. Without this, the test
/// above could pass for the wrong reason.
#[tokio::test]
async fn without_keepalives_the_same_verification_would_stall() {
    let (result, _) = slow_verification(Duration::from_secs(60)).await;
    assert!(
        matches!(result, Err(TransferError::Stalled { .. })),
        "{result:?}"
    );
}
