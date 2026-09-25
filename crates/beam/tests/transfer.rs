//! Integration tests for the transfer engine.
//!
//! Most of these run over `tokio::io::duplex`, an in-memory pipe, so they
//! exercise the real engine on both sides without a socket. One test at the
//! bottom repeats the happy path over real TCP, to show that the engine does
//! not secretly depend on the stream being in-memory.
//!
//! The tests that pin down the six Accept rules are grouped under
//! `mod accept_rules` and named after the requirement they defend.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beam::identity::{Identity, KnownPeers, Peer, encode_public_key};
use beam::transfer::frame::{read_message, write_message};
use beam::transfer::message::{
    CHUNK_SIZE, ChunkStart, Message, RejectReason, TransferId, TransferRequest,
};
use beam::transfer::{
    Prompt, PromptRequest, ReceiveOptions, SendOptions, SilentReporter, TransferError,
    receive_file, send_file,
};
use tempfile::TempDir;

/// A prompt whose answer is fixed, which records whether it was ever asked.
///
/// "Was it asked?" is the interesting question for S-7: an unknown sender must
/// be turned away without a person ever being interrupted.
#[derive(Clone)]
struct ScriptedPrompt {
    answer: bool,
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
    ///
    /// Needed by any test that turns on something arriving *while* the prompt
    /// is open: a prompt answering in nanoseconds can win a race no real person
    /// would, which makes such a test flaky rather than strict.
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

/// A prompt that never answers, for the expiry test.
struct SilentPrompt {
    asked: Arc<Mutex<bool>>,
}

impl Prompt for SilentPrompt {
    fn confirm(&mut self, _request: &PromptRequest) -> std::io::Result<bool> {
        *self.asked.lock().expect("flag") = true;
        // Long enough that only the expiry timer can win the race, short
        // enough that it does not hold up the runtime's shutdown: tokio waits
        // for blocking tasks to finish, and this sleep is a real one even when
        // the test's clock is paused.
        std::thread::sleep(Duration::from_secs(2));
        Ok(true)
    }
}

/// Two peers that have paired with each other.
struct Pair {
    sender: Identity,
    receiver_known_peers: KnownPeers,
}

fn paired() -> Pair {
    let sender = Identity::generate("sender").expect("generate");
    let mut known = KnownPeers::with_header();
    known
        .add(Peer::new("alice", sender.verifying_key()))
        .expect("add peer");
    Pair {
        sender,
        receiver_known_peers: known,
    }
}

/// A beam home directory plus an output directory.
struct Dirs {
    _tmp: TempDir,
    out: PathBuf,
    work: PathBuf,
    files: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("out");
    let work = tmp.path().join("work");
    let files = tmp.path().join("files");
    for dir in [&out, &work, &files] {
        std::fs::create_dir_all(dir).expect("create dir");
    }
    Dirs {
        _tmp: tmp,
        out,
        work,
        files,
    }
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write test file");
    path
}

/// Deterministic pseudo-random bytes, so failures are reproducible.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + 7) % 251) as u8).collect()
}

fn options_for(dirs: &Dirs) -> ReceiveOptions {
    let mut options = ReceiveOptions::new(&dirs.out, &dirs.work);
    options.accept_timeout = Duration::from_secs(60);
    options
}

/// Runs a full transfer over an in-memory pipe and returns both outcomes.
async fn transfer(
    bytes: &[u8],
    file_name: &str,
    prompt: ScriptedPrompt,
) -> (
    Dirs,
    ScriptedPrompt,
    Result<beam::transfer::SendSummary, TransferError>,
    Result<beam::transfer::ReceiveSummary, TransferError>,
) {
    let pair = paired();
    let dirs = dirs();
    let path = write_file(&dirs.files, file_name, bytes);

    let (client, server) = tokio::io::duplex(16 * 1024);

    let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });

    let receive_options = options_for(&dirs);
    let known = pair.receiver_known_peers;
    let prompt_clone = prompt.clone();
    let received = receive_file(
        server,
        &known,
        &receive_options,
        prompt_clone,
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;

    let sent = send.await.expect("sender task");
    (dirs, prompt, sent, received)
}

#[tokio::test]
async fn a_file_arrives_byte_for_byte() {
    let bytes = payload(200_000);
    let (dirs, _prompt, sent, received) =
        transfer(&bytes, "project.zip", ScriptedPrompt::new(true)).await;

    let sent = sent.expect("send");
    let received = received.expect("receive");

    assert_eq!(sent.bytes_sent, bytes.len() as u64);
    assert_eq!(received.final_name, "project.zip");
    assert_eq!(received.peer_name, "alice");
    assert_eq!(sent.final_name.as_deref(), Some("project.zip"));
    assert_eq!(sent.transfer_id, received.transfer_id);

    let landed = std::fs::read(dirs.out.join("project.zip")).expect("read result");
    assert_eq!(landed, bytes, "the received file differs from the sent one");
}

#[tokio::test]
async fn an_empty_file_transfers() {
    let (dirs, _prompt, sent, received) =
        transfer(&[], "empty.bin", ScriptedPrompt::new(true)).await;
    sent.expect("send");
    received.expect("receive");
    assert_eq!(
        std::fs::read(dirs.out.join("empty.bin")).expect("read result"),
        Vec::<u8>::new()
    );
}

#[tokio::test]
async fn a_file_spanning_several_chunks_transfers() {
    // Just over two chunks, so the last one is short and the boundary logic is
    // exercised for real rather than in a unit test alone.
    let bytes = payload(CHUNK_SIZE as usize * 2 + 1234);
    let (dirs, _prompt, sent, received) =
        transfer(&bytes, "big.bin", ScriptedPrompt::new(true)).await;

    assert_eq!(sent.expect("send").bytes_sent, bytes.len() as u64);
    received.expect("receive");
    assert_eq!(
        std::fs::read(dirs.out.join("big.bin")).expect("read result"),
        bytes
    );
}

#[tokio::test]
async fn a_colliding_name_is_numbered_and_reported_to_both_sides() {
    let bytes = payload(1_000);
    let pair = paired();
    let dirs = dirs();
    let path = write_file(&dirs.files, "report.pdf", &bytes);

    // Something is already sitting in the way.
    std::fs::write(dirs.out.join("report.pdf"), b"existing").expect("seed");

    let (client, server) = tokio::io::duplex(16 * 1024);
    let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });

    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        ScriptedPrompt::new(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await
    .expect("receive");
    let sent = send.await.expect("task").expect("send");

    assert_eq!(received.final_name, "report (1).pdf");
    assert_eq!(sent.final_name.as_deref(), Some("report (1).pdf"));
    assert_eq!(
        std::fs::read(dirs.out.join("report.pdf")).expect("read"),
        b"existing",
        "the existing file was overwritten"
    );
    assert_eq!(
        std::fs::read(dirs.out.join("report (1).pdf")).expect("read"),
        bytes
    );
}

#[tokio::test]
async fn nothing_is_left_behind_when_a_transfer_is_declined() {
    let (dirs, prompt, sent, received) =
        transfer(&payload(5_000), "secret.bin", ScriptedPrompt::new(false)).await;

    assert!(matches!(
        received,
        Err(TransferError::Rejected(RejectReason::Declined))
    ));
    assert!(matches!(
        sent,
        Err(TransferError::Rejected(RejectReason::Declined))
    ));
    assert_eq!(prompt.asked().len(), 1, "the person was not asked");
    assert!(
        std::fs::read_dir(&dirs.out)
            .expect("read out dir")
            .next()
            .is_none(),
        "a declined transfer left a file behind"
    );
}

/// The six Accept rules from CLAUDE.md, one test each.
mod accept_rules {
    use super::*;

    /// S-1: there is no way to accept a transfer without a person answering.
    ///
    /// The type system carries most of this — `receive_file` cannot reach the
    /// accepting branch except through `Prompt::confirm` — so what is checked
    /// here is the other half: that no flag on `beam listen` can stand in for
    /// the answer. The CLI half of the rule is checked in `cli.rs`.
    #[tokio::test]
    async fn s1_the_prompt_is_the_only_route_to_accepting() {
        let (dirs, prompt, _sent, received) =
            transfer(&payload(1_000), "a.bin", ScriptedPrompt::new(false)).await;

        // Refusing at the prompt is enough to stop the transfer dead: no
        // setting, default or fallback rescues it.
        assert!(matches!(
            received,
            Err(TransferError::Rejected(RejectReason::Declined))
        ));
        assert_eq!(prompt.asked().len(), 1);
        assert!(
            std::fs::read_dir(&dirs.out).expect("read").next().is_none(),
            "a file appeared without anybody agreeing to it"
        );
    }

    /// S-3: the sender emits no file bytes before ACCEPT arrives.
    ///
    /// A hand-written peer plays the receiver and records the order of every
    /// frame it sees while deliberately never accepting.
    #[tokio::test]
    async fn s3_the_sender_emits_no_file_bytes_before_accept() {
        let identity = Identity::generate("sender").expect("generate");
        let dirs = dirs();
        let path = write_file(&dirs.files, "a.bin", &payload(300_000));

        let (client, mut server) = tokio::io::duplex(16 * 1024);
        let mut options = SendOptions::new(path, encode_public_key(&identity.verifying_key()));
        options.accept_timeout = Duration::from_millis(200);

        let send = tokio::spawn(async move {
            let mut client = client;
            send_file(&mut client, &options, &mut SilentReporter).await
        });

        // Read everything the sender is willing to say before it gives up.
        let mut seen = Vec::new();
        while let Ok(Ok(message)) =
            tokio::time::timeout(Duration::from_millis(500), read_message(&mut server)).await
        {
            seen.push(message.kind_name());
            if message.carries_file_data() {
                break;
            }
        }

        assert_eq!(
            seen,
            vec!["TRANSFER_REQUEST"],
            "the sender said more than the request before being accepted"
        );
        assert!(matches!(
            send.await.expect("task"),
            Err(TransferError::Rejected(RejectReason::Expired))
        ));
    }

    /// S-4: file bytes arriving before ACCEPT abort the transfer, and what was
    /// received is discarded.
    #[tokio::test]
    async fn s4_data_before_accept_aborts_and_discards() {
        let pair = paired();
        let dirs = dirs();
        let prompt = ScriptedPrompt::deliberate(true);

        let (mut client, server) = tokio::io::duplex(64 * 1024);

        // A rude sender: request, then chunks, without waiting to be accepted.
        let sender_key = encode_public_key(&pair.sender.verifying_key());
        let transfer_id = TransferId::generate().expect("id");
        let bytes = payload(1_000);
        let rude = tokio::spawn(async move {
            let request = TransferRequest {
                transfer_id,
                sender_public_key: sender_key,
                file_name: "rude.bin".to_string(),
                size: bytes.len() as u64,
                chunk_size: CHUNK_SIZE,
                chunk_count: 1,
                file_sha256: beam::transfer::sha256_hex(&bytes),
            };
            write_message(&mut client, &Message::TransferRequest(request))
                .await
                .expect("write request");
            write_message(
                &mut client,
                &Message::ChunkStart(ChunkStart {
                    index: 0,
                    len: bytes.len() as u32,
                    sha256: beam::transfer::sha256_hex(&bytes),
                }),
            )
            .await
            .expect("write chunk start");
            write_message(
                &mut client,
                &Message::ChunkData {
                    index: 0,
                    bytes: bytes.clone(),
                },
            )
            .await
            .expect("write chunk data");
            // Hold the stream open so the receiver's failure is its own doing.
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let received = receive_file(
            server,
            &pair.receiver_known_peers,
            &options_for(&dirs),
            prompt,
            &mut SilentReporter,
            &mut HashSet::new(),
        )
        .await;

        assert!(
            matches!(received, Err(TransferError::DataBeforeAccept)),
            "expected DataBeforeAccept, got {received:?}"
        );
        assert!(
            std::fs::read_dir(&dirs.out).expect("read").next().is_none(),
            "early data was written to the destination"
        );
        assert!(
            std::fs::read_dir(&dirs.work)
                .expect("read")
                .next()
                .is_none(),
            "early data was left in the work directory"
        );
        rude.abort();
    }

    /// S-5: an unanswered request expires, and the expiry counts as a Reject
    /// on both sides.
    ///
    /// The deadline here is 200 ms rather than the real 60 s, because a test
    /// that waits a minute does not get run. `tokio::time::pause` is not an
    /// option: the clock only auto-advances while the runtime is idle, and the
    /// prompt deliberately occupies a blocking thread. The 60 s figure itself
    /// is asserted in `s5_the_default_deadline_is_sixty_seconds` below, so
    /// between the two the whole rule is covered.
    #[tokio::test]
    async fn s5_an_unanswered_request_expires_as_a_reject() {
        let pair = paired();
        let dirs = dirs();
        let asked = Arc::new(Mutex::new(false));

        let (client, server) = tokio::io::duplex(16 * 1024);
        let path = write_file(&dirs.files, "a.bin", &payload(1_000));
        let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
        let send = tokio::spawn(async move {
            let mut client = client;
            send_file(&mut client, &options, &mut SilentReporter).await
        });

        let mut receive_options = options_for(&dirs);
        receive_options.accept_timeout = Duration::from_millis(200);

        let received = receive_file(
            server,
            &pair.receiver_known_peers,
            &receive_options,
            SilentPrompt {
                asked: Arc::clone(&asked),
            },
            &mut SilentReporter,
            &mut HashSet::new(),
        )
        .await;

        assert!(
            matches!(
                received,
                Err(TransferError::Rejected(RejectReason::Expired))
            ),
            "expected Expired, got {received:?}"
        );
        assert!(*asked.lock().expect("flag"), "the person was never asked");
        assert!(
            matches!(
                send.await.expect("task"),
                Err(TransferError::Rejected(RejectReason::Expired))
            ),
            "the sender was not told the request expired"
        );
        assert!(
            std::fs::read_dir(&dirs.out).expect("read").next().is_none(),
            "an expired transfer left a file behind"
        );
    }

    /// S-5, the other half: the deadline a real run uses is 60 seconds.
    #[test]
    fn s5_the_default_deadline_is_sixty_seconds() {
        assert_eq!(
            beam::transfer::DEFAULT_ACCEPT_TIMEOUT,
            Duration::from_secs(60)
        );
    }

    /// S-6: the prompt shows sender name, fingerprint, file name and size.
    #[tokio::test]
    async fn s6_the_prompt_shows_who_what_and_how_big() {
        let bytes = payload(4_321);
        let (_dirs, prompt, _sent, received) =
            transfer(&bytes, "invoice.pdf", ScriptedPrompt::new(true)).await;
        received.expect("receive");

        let asked = prompt.asked();
        assert_eq!(asked.len(), 1);
        let shown = &asked[0];

        assert_eq!(shown.peer_name, "alice", "the stored nickname is not shown");
        assert!(
            shown.fingerprint.starts_with("SHA256:") && shown.fingerprint.len() > 40,
            "the full fingerprint is not shown: {}",
            shown.fingerprint
        );
        assert_eq!(shown.file_name, "invoice.pdf");
        assert_eq!(shown.size, bytes.len() as u64);
    }

    /// S-7: a sender that is not in `known_peers` is refused without a prompt.
    ///
    /// This proves that an *unrecognised key* is turned away, over an
    /// in-memory pipe where the key is only claimed. Its sibling — a known
    /// peer's public key presented without the matching private key, which
    /// must also be refused — needs a transport that proves keys, so it runs
    /// over iroh: `tests/listen.rs::impersonation::
    /// a_known_public_key_without_its_secret_key_gets_nowhere`.
    #[tokio::test]
    async fn s7_an_unknown_sender_is_refused_without_a_prompt() {
        let stranger = Identity::generate("stranger").expect("generate");
        let dirs = dirs();
        let prompt = ScriptedPrompt::new(true);

        // The receiver has paired with somebody, just not with this sender.
        let mut known = KnownPeers::with_header();
        let other = Identity::generate("somebody-else").expect("generate");
        known
            .add(Peer::new("bob", other.verifying_key()))
            .expect("add");

        let (client, server) = tokio::io::duplex(16 * 1024);
        let path = write_file(&dirs.files, "a.bin", &payload(1_000));
        let options = SendOptions::new(path, encode_public_key(&stranger.verifying_key()));
        let send = tokio::spawn(async move {
            let mut client = client;
            send_file(&mut client, &options, &mut SilentReporter).await
        });

        let received = receive_file(
            server,
            &known,
            &options_for(&dirs),
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
            "an unknown sender reached the prompt"
        );
        assert!(
            matches!(
                send.await.expect("task"),
                Err(TransferError::Rejected(RejectReason::UnknownPeer))
            ),
            "the sender was not told why"
        );
        assert!(
            std::fs::read_dir(&dirs.out).expect("read").next().is_none(),
            "an unknown sender got a file written"
        );
    }
}

#[tokio::test]
async fn a_replayed_transfer_id_is_refused() {
    let pair = paired();
    let dirs = dirs();
    let mut seen = HashSet::new();

    // Pre-load the id the sender is about to use by running one transfer, then
    // replaying its id by hand.
    let transfer_id = TransferId::generate().expect("id");
    seen.insert(transfer_id);

    let bytes = payload(100);
    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let sender_key = encode_public_key(&pair.sender.verifying_key());
    let replay = tokio::spawn(async move {
        let request = TransferRequest {
            transfer_id,
            sender_public_key: sender_key,
            file_name: "again.bin".to_string(),
            size: bytes.len() as u64,
            chunk_size: CHUNK_SIZE,
            chunk_count: 1,
            file_sha256: beam::transfer::sha256_hex(&bytes),
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write");
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let prompt = ScriptedPrompt::new(true);
    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        prompt.clone(),
        &mut SilentReporter,
        &mut seen,
    )
    .await;

    assert!(
        matches!(received, Err(TransferError::ReplayedTransferId)),
        "expected ReplayedTransferId, got {received:?}"
    );
    assert!(prompt.asked().is_empty(), "a replay reached the prompt");
    replay.abort();
}

#[tokio::test]
async fn a_hostile_file_name_is_refused_before_anything_is_created() {
    let pair = paired();
    let dirs = dirs();

    let bytes = payload(100);
    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let sender_key = encode_public_key(&pair.sender.verifying_key());
    let hostile = tokio::spawn(async move {
        let request = TransferRequest {
            transfer_id: TransferId::generate().expect("id"),
            sender_public_key: sender_key,
            // Reduced to a base name it would be fine; `..` cannot be.
            file_name: "..".to_string(),
            size: bytes.len() as u64,
            chunk_size: CHUNK_SIZE,
            chunk_count: 1,
            file_sha256: beam::transfer::sha256_hex(&bytes),
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write");
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let prompt = ScriptedPrompt::new(true);
    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        prompt.clone(),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;

    assert!(
        matches!(received, Err(TransferError::Name(_))),
        "expected a name error, got {received:?}"
    );
    assert!(prompt.asked().is_empty(), "a bad name reached the prompt");
    hostile.abort();
}

#[tokio::test]
async fn a_chunk_count_that_does_not_match_the_size_is_refused() {
    let pair = paired();
    let dirs = dirs();

    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let sender_key = encode_public_key(&pair.sender.verifying_key());
    let liar = tokio::spawn(async move {
        let request = TransferRequest {
            transfer_id: TransferId::generate().expect("id"),
            sender_public_key: sender_key,
            file_name: "lie.bin".to_string(),
            size: 100,
            chunk_size: CHUNK_SIZE,
            chunk_count: 9_999,
            file_sha256: "0".repeat(64),
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write");
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let prompt = ScriptedPrompt::new(true);
    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        prompt.clone(),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;

    assert!(
        matches!(received, Err(TransferError::Plan(_))),
        "expected a plan error, got {received:?}"
    );
    assert!(prompt.asked().is_empty());
    liar.abort();
}

/// S-12, receiver side: a chunk whose bytes do not match its announced hash is
/// refused, asked for again, and only written once it is right.
#[tokio::test]
async fn the_receiver_refuses_a_chunk_that_fails_its_hash() {
    let pair = paired();
    let dirs = dirs();
    let bytes = payload(1_000);
    let digest = beam::transfer::sha256_hex(&bytes);

    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let sender_key = encode_public_key(&pair.sender.verifying_key());
    let file_sha256 = digest.clone();
    let expected = bytes.clone();

    let liar = tokio::spawn(async move {
        let transfer_id = TransferId::generate().expect("id");
        let request = TransferRequest {
            transfer_id,
            sender_public_key: sender_key,
            file_name: "fragile.bin".to_string(),
            size: bytes.len() as u64,
            chunk_size: CHUNK_SIZE,
            chunk_count: 1,
            file_sha256,
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write request");
        assert!(matches!(
            read_message(&mut client).await.expect("read"),
            Message::Accept(_)
        ));

        // First attempt announces the right hash but sends the wrong bytes;
        // the second sends what it promised.
        let mut corrupted = bytes.clone();
        corrupted[0] ^= 0xff;

        let mut naks = 0;
        for body in [corrupted, bytes.clone()] {
            write_message(
                &mut client,
                &Message::ChunkStart(ChunkStart {
                    index: 0,
                    len: bytes.len() as u32,
                    sha256: digest.clone(),
                }),
            )
            .await
            .expect("write chunk start");
            write_message(
                &mut client,
                &Message::ChunkData {
                    index: 0,
                    bytes: body,
                },
            )
            .await
            .expect("write chunk data");

            match read_message(&mut client).await.expect("read") {
                Message::ChunkNak(nak) => {
                    assert_eq!(nak.index, 0);
                    naks += 1;
                }
                Message::ChunkAck(ack) => {
                    assert_eq!(ack.index, 0);
                    break;
                }
                other => panic!("unexpected {}", other.kind_name()),
            }
        }
        assert_eq!(naks, 1, "the receiver accepted the corrupted bytes");

        write_message(
            &mut client,
            &Message::Complete(beam::transfer::Complete {
                transfer_id,
                final_name: None,
            }),
        )
        .await
        .expect("write complete");
        let _ = read_message(&mut client).await;
    });

    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        ScriptedPrompt::new(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await
    .expect("receive");

    liar.await.expect("sender task");
    assert_eq!(
        std::fs::read(dirs.out.join(&received.final_name)).expect("read"),
        expected,
        "the corrupted first attempt was written"
    );
}

/// S-12, receiver side: a chunk that keeps failing ends the transfer, and
/// nothing is written.
#[tokio::test]
async fn a_chunk_that_never_verifies_fails_the_transfer() {
    let pair = paired();
    let dirs = dirs();
    let bytes = payload(1_000);
    let digest = beam::transfer::sha256_hex(&bytes);

    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let sender_key = encode_public_key(&pair.sender.verifying_key());
    let file_sha256 = digest.clone();

    let liar = tokio::spawn(async move {
        let request = TransferRequest {
            transfer_id: TransferId::generate().expect("id"),
            sender_public_key: sender_key,
            file_name: "never.bin".to_string(),
            size: bytes.len() as u64,
            chunk_size: CHUNK_SIZE,
            chunk_count: 1,
            file_sha256,
        };
        write_message(&mut client, &Message::TransferRequest(request))
            .await
            .expect("write request");
        let _ = read_message(&mut client).await;

        let mut corrupted = bytes.clone();
        corrupted[0] ^= 0xff;
        loop {
            if write_message(
                &mut client,
                &Message::ChunkStart(ChunkStart {
                    index: 0,
                    len: bytes.len() as u32,
                    sha256: digest.clone(),
                }),
            )
            .await
            .is_err()
            {
                return;
            }
            if write_message(
                &mut client,
                &Message::ChunkData {
                    index: 0,
                    bytes: corrupted.clone(),
                },
            )
            .await
            .is_err()
            {
                return;
            }
            if read_message(&mut client).await.is_err() {
                return;
            }
        }
    });

    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        ScriptedPrompt::new(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;

    assert!(
        matches!(received, Err(TransferError::ChunkFailed { index: 0, .. })),
        "expected ChunkFailed, got {received:?}"
    );
    assert!(
        std::fs::read_dir(&dirs.out).expect("read").next().is_none(),
        "a file was written despite the chunk never verifying"
    );
    liar.abort();
}

/// S-12, sender side: the sender re-sends a chunk the receiver rejects.
#[tokio::test]
async fn the_sender_re_sends_a_chunk_that_is_nakked() {
    let identity = Identity::generate("sender").expect("generate");
    let dirs = dirs();
    let bytes = payload(2_000);
    let path = write_file(&dirs.files, "retry.bin", &bytes);

    let (client, mut server) = tokio::io::duplex(64 * 1024);
    let options = SendOptions::new(path, encode_public_key(&identity.verifying_key()));
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });

    // Play the receiver by hand: accept, reject the chunk once, then take it.
    let transfer_id = match read_message(&mut server).await.expect("read") {
        Message::TransferRequest(request) => request.transfer_id,
        other => panic!("unexpected {}", other.kind_name()),
    };
    write_message(
        &mut server,
        &Message::Accept(beam::transfer::Accept {
            transfer_id,
            have_bitmap: None,
        }),
    )
    .await
    .expect("write accept");

    let mut attempts = 0;
    loop {
        match read_message(&mut server).await.expect("read") {
            Message::ChunkStart(_) => {
                attempts += 1;
                let mut got = 0usize;
                while got < bytes.len() {
                    match read_message(&mut server).await.expect("read") {
                        Message::ChunkData { bytes, .. } => got += bytes.len(),
                        other => panic!("unexpected {}", other.kind_name()),
                    }
                }
                let answer = if attempts == 1 {
                    Message::ChunkNak(beam::transfer::ChunkNak {
                        index: 0,
                        reason: beam::transfer::NakReason::HashMismatch,
                    })
                } else {
                    Message::ChunkAck(beam::transfer::ChunkAck { index: 0 })
                };
                write_message(&mut server, &answer).await.expect("write");
            }
            Message::Complete(_) => break,
            other => panic!("unexpected {}", other.kind_name()),
        }
    }
    assert_eq!(attempts, 2, "the sender did not re-send the rejected chunk");

    write_message(
        &mut server,
        &Message::Complete(beam::transfer::Complete {
            transfer_id,
            final_name: Some("retry.bin".to_string()),
        }),
    )
    .await
    .expect("write complete");

    let summary = send.await.expect("task").expect("send");
    assert_eq!(summary.bytes_sent, bytes.len() as u64);
}

#[tokio::test]
async fn the_engine_works_over_real_tcp() {
    // The same happy path as the first test, but over a socket, to show the
    // engine is not quietly relying on an in-memory stream (ADR-0016).
    let pair = paired();
    let dirs = dirs();
    let bytes = payload(120_000);
    let path = write_file(&dirs.files, "over-tcp.bin", &bytes);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");

    let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    let send = tokio::spawn(async move {
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        send_file(&mut stream, &options, &mut SilentReporter).await
    });

    let (stream, _peer) = listener.accept().await.expect("accept");
    let received = receive_file(
        stream,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        ScriptedPrompt::new(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await
    .expect("receive");

    send.await.expect("task").expect("send");
    assert_eq!(received.final_name, "over-tcp.bin");
    assert_eq!(
        std::fs::read(dirs.out.join("over-tcp.bin")).expect("read"),
        bytes
    );
}

/// Records every progress event, and switches the route to the relay the first
/// time bytes move — as iroh does when a direct path drops mid-transfer.
struct RouteFlipper {
    events: Vec<beam::transfer::Progress>,
    flip: Option<tokio::sync::watch::Sender<beam::transport::PathKind>>,
}

impl beam::transfer::Reporter for RouteFlipper {
    fn report(&mut self, progress: beam::transfer::Progress) {
        if matches!(progress, beam::transfer::Progress::Transferring { .. })
            && let Some(flip) = self.flip.take()
        {
            // A watch receiver keeps the last value after the sender is gone.
            flip.send(beam::transport::PathKind::Relay).expect("route");
        }
        self.events.push(progress);
    }
}

/// F-11: when the path changes mid-transfer, the progress line says so and
/// every later update carries the new path.
#[tokio::test]
async fn a_path_change_mid_transfer_is_reported() {
    use beam::transfer::Progress;
    use beam::transport::PathKind;

    let bytes = payload(20_000);
    let pair = paired();
    let dirs = dirs();
    let path = write_file(&dirs.files, "moving.bin", &bytes);
    let (client, server) = tokio::io::duplex(64 * 1024);

    let (tx, rx) = tokio::sync::watch::channel(PathKind::Direct);
    let mut options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    options.chunk_size = 2_000;
    options.route = rx;
    let send = tokio::spawn(async move {
        let mut client = client;
        let mut reporter = RouteFlipper {
            events: Vec::new(),
            flip: Some(tx),
        };
        let result = send_file(&mut client, &options, &mut reporter).await;
        (result, reporter.events)
    });

    receive_file(
        server,
        &pair.receiver_known_peers,
        &options_for(&dirs),
        ScriptedPrompt::new(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await
    .expect("receive");
    let (sent, events) = send.await.expect("sender task");
    sent.expect("send");

    let paths: Vec<PathKind> = events
        .iter()
        .filter_map(|e| match e {
            Progress::Transferring { path, .. } => Some(*path),
            _ => None,
        })
        .collect();
    assert_eq!(paths.first(), Some(&PathKind::Direct), "{events:?}");
    assert_eq!(paths.last(), Some(&PathKind::Relay), "{events:?}");
    let changes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Progress::PathChanged { .. }))
        .collect();
    assert_eq!(
        changes,
        [&Progress::PathChanged {
            from: PathKind::Direct,
            to: PathKind::Relay
        }],
        "the change is reported exactly once"
    );
}

/// ADR-0031: when the transport has proved who the sender is, a request that
/// claims to be someone else is refused, even when the claimed key is a
/// paired peer's. Nobody is asked.
#[tokio::test]
async fn a_request_claiming_a_key_other_than_the_proven_one_is_refused() {
    let pair = paired();
    let dirs = dirs();
    let path = write_file(&dirs.files, "x.bin", &payload(100));
    let (client, server) = tokio::io::duplex(16 * 1024);

    // Claims alice (paired); the connection proved a stranger.
    let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });
    let mut receive_options = options_for(&dirs);
    receive_options.proven_sender = Some(Identity::generate("stranger").unwrap().verifying_key());
    let prompt = ScriptedPrompt::new(true);

    let received = receive_file(
        server,
        &pair.receiver_known_peers,
        &receive_options,
        prompt.clone(),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;

    assert!(
        matches!(received, Err(TransferError::BadRequest(_))),
        "{received:?}"
    );
    assert!(matches!(
        send.await.unwrap(),
        Err(TransferError::Rejected(RejectReason::BadRequest))
    ));
    assert!(prompt.asked().is_empty());
}

#[tokio::test]
async fn a_request_matching_the_proven_key_goes_through() {
    let pair = paired();
    let dirs = dirs();
    let bytes = payload(5_000);
    let path = write_file(&dirs.files, "ok.bin", &bytes);
    let (client, server) = tokio::io::duplex(16 * 1024);

    let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });
    let mut receive_options = options_for(&dirs);
    receive_options.proven_sender = Some(pair.sender.verifying_key());

    receive_file(
        server,
        &pair.receiver_known_peers,
        &receive_options,
        ScriptedPrompt::new(true),
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await
    .expect("receive");
    send.await.unwrap().expect("send");
    assert_eq!(std::fs::read(dirs.out.join("ok.bin")).unwrap(), bytes);
}

/// ADR-0030: a second transfer while one is in progress is turned away with
/// Busy, and the sender is told in words.
#[tokio::test]
async fn a_sender_turned_away_as_busy_is_told_to_try_later() {
    let pair = paired();
    let dirs = dirs();
    let path = write_file(&dirs.files, "later.bin", &payload(100));
    let (client, server) = tokio::io::duplex(16 * 1024);

    let options = SendOptions::new(path, encode_public_key(&pair.sender.verifying_key()));
    let send = tokio::spawn(async move {
        let mut client = client;
        send_file(&mut client, &options, &mut SilentReporter).await
    });
    beam::transfer::turn_away(server, RejectReason::Busy, Duration::from_secs(5))
        .await
        .expect("turn away");

    let sent = send.await.unwrap();
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Busy))),
        "{sent:?}"
    );
    assert!(sent.unwrap_err().to_string().contains("another file"));
    assert!(std::fs::read_dir(&dirs.out).unwrap().next().is_none());
}
