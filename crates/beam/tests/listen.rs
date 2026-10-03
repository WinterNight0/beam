//! `beam listen` as a service, over real iroh endpoints on loopback. No server:
//! each test dials the address the listener put in its invite (ADR-0036).
//!
//! These are the M2/M3 guarantees re-proved on the M5 transport — every Accept
//! rule and resume — plus what only exists here: the sender proved by the
//! connection, one transfer at a time, pairing inside `listen` with a code that
//! renews itself and switches off after three failures, and a pairing request
//! and a transfer request arriving together.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beam::config::Relay;
use beam::identity::{Identity, Peer, Store};
use beam::listener::{ListenEvent, ListenOptions, run};
use beam::pairing::{
    Confirm, ConfirmRequest, Network, PairError, Pairing, PairingCode, PairingError, Policy,
    Timeouts, join,
};
use beam::transfer::frame::{read_message, write_message};
use beam::transfer::message::{ChunkStart, Message, RejectReason, TransferId, TransferRequest};
use beam::transfer::{
    Progress, Prompt, PromptRequest, Reporter, SendOptions, SilentReporter, TransferError,
};
use beam::transport::PathKind;
use beam::transport::dial::{DialError, dial, send_on};
use beam::transport::endpoint::{self, Bind, XFER_ALPN};
use iroh::EndpointAddr;
use tokio::sync::mpsc;

const PATIENCE: Duration = Duration::from_secs(30);

fn network() -> Network {
    Network {
        relay: Relay::Disabled,
        bind: Bind::Loopback,
        port: 0,
        advertise: Vec::new(),
    }
}

/// Answers questions from a script, and records them.
///
/// Answers are taken in order; a `None` answer blocks until released, which
/// is how a test holds a prompt open.
#[derive(Clone, Default)]
struct Script {
    transfers: Arc<Mutex<VecDeque<bool>>>,
    pairings: Arc<Mutex<VecDeque<bool>>>,
    asked: Arc<Mutex<Vec<String>>>,
    gate: Arc<Mutex<Option<std::sync::mpsc::Receiver<bool>>>>,
}

impl Script {
    fn transfers(self, answers: &[bool]) -> Self {
        self.transfers.lock().unwrap().extend(answers);
        self
    }
    fn pairings(self, answers: &[bool]) -> Self {
        self.pairings.lock().unwrap().extend(answers);
        self
    }
    /// The next transfer question waits for the test to answer it.
    fn hold_next_transfer(&self) -> std::sync::mpsc::Sender<bool> {
        let (tx, rx) = std::sync::mpsc::channel();
        *self.gate.lock().unwrap() = Some(rx);
        tx
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Prompt for Script {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        let resume = if request.resume.is_some() {
            " (resuming)"
        } else {
            ""
        };
        self.asked.lock().unwrap().push(format!(
            "transfer {} from {}{resume}",
            request.file_name, request.peer_name
        ));
        if let Some(gate) = self.gate.lock().unwrap().take() {
            return Ok(gate.recv().unwrap_or(false));
        }
        Ok(self.transfers.lock().unwrap().pop_front().unwrap_or(false))
    }
}

impl Confirm for Script {
    fn confirm(&mut self, request: &ConfirmRequest) -> std::io::Result<bool> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("pair {:?} as {}", request.role, request.name));
        Ok(self.pairings.lock().unwrap().pop_front().unwrap_or(false))
    }
}

/// A beam home on disk.
struct Home {
    _tmp: tempfile::TempDir,
    store: Store,
    identity: Identity,
    inbox: PathBuf,
    files: PathBuf,
    /// The address this home's `listen` put in its invite, once it has one.
    listening_at: Mutex<Option<EndpointAddr>>,
}

impl Home {
    /// Where a peer that saved this home's invite would dial it.
    fn at(&self) -> EndpointAddr {
        self.listening_at
            .lock()
            .unwrap()
            .clone()
            .expect("this home is not listening")
    }
}

fn home(comment: &str) -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("beam"));
    let identity = Identity::generate(comment).unwrap();
    store.save_identity(&identity, false).unwrap();
    let inbox = tmp.path().join("inbox");
    let files = tmp.path().join("files");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&files).unwrap();
    Home {
        _tmp: tmp,
        store,
        identity,
        inbox,
        files,
        listening_at: Mutex::new(None),
    }
}

fn pair_by_hand(a: &Home, a_name_for_b: &str, b: &Home, b_name_for_a: &str) {
    for (store, name, key) in [
        (&a.store, a_name_for_b, b.identity.verifying_key()),
        (&b.store, b_name_for_a, a.identity.verifying_key()),
    ] {
        let mut known = store.load_known_peers().unwrap();
        known.add(Peer::new(name, key)).unwrap();
        store.save_known_peers(&known).unwrap();
    }
}

/// A running `listen`, its events, and the first pairing code it showed.
struct Listening {
    events: mpsc::UnboundedReceiver<ListenEvent>,
    code: PairingCode,
    task: tokio::task::JoinHandle<()>,
}

impl Listening {
    /// Waits for an event matching `want`, skipping others.
    async fn expect(&mut self, what: &str, want: impl Fn(&ListenEvent) -> bool) -> ListenEvent {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(event)) if want(&event) => return event,
                Ok(Some(_)) => continue,
                _ => panic!("never saw {what}"),
            }
        }
    }

    /// The next new code, after an attempt used the last one.
    async fn next_code(&mut self) -> PairingCode {
        match self
            .expect("a new code", |e| matches!(e, ListenEvent::NewCode { .. }))
            .await
        {
            ListenEvent::NewCode { code, .. } => code,
            _ => unreachable!(),
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn options(home: &Home, policy: Policy) -> ListenOptions {
    ListenOptions {
        out_dir: home.inbox.clone(),
        accept_timeout: Duration::from_secs(10),
        stall_timeout: Duration::from_secs(10),
        pairing: policy,
        timeouts: Timeouts {
            code_ttl: policy.code_ttl,
            message: Duration::from_secs(10),
            decision: Duration::from_secs(10),
        },
    }
}

async fn listen(home: &Home, prompt: Script, options: ListenOptions) -> Listening {
    let (tx, mut events) = mpsc::unbounded_channel();
    let identity = home.identity.clone();
    let store = home.store.clone();
    let network = network();
    let task = tokio::spawn(async move {
        let _ = run(
            identity,
            store,
            network,
            options,
            prompt,
            || SilentReporter,
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await;
    });
    let code = match tokio::time::timeout(PATIENCE, events.recv()).await {
        Ok(Some(ListenEvent::Ready { code, invite, .. })) => {
            assert_eq!(invite.key, home.identity.verifying_key());
            *home.listening_at.lock().unwrap() = Some(invite.endpoint_addr());
            code
        }
        other => panic!("listen did not start: {other:?}"),
    };
    Listening { events, code, task }
}

/// Sends `bytes` as `name` from `from` to the peer it calls `to`.
async fn send(
    from: &Home,
    to: &Home,
    name: &str,
    bytes: &[u8],
    tweak: impl FnOnce(&mut SendOptions),
    reporter: &mut impl Reporter,
) -> Result<beam::transfer::SendSummary, TransferError> {
    let path = from.files.join(name);
    std::fs::write(&path, bytes).unwrap();
    let endpoint = endpoint::bind(&from.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();
    let connection = dial(&endpoint, to.at(), XFER_ALPN).await.expect("dial");
    let mut options = SendOptions::new(
        path,
        beam::identity::encode_public_key(&from.identity.verifying_key()),
    );
    tweak(&mut options);
    let result = send_on(&connection, &mut options, reporter).await;
    endpoint.close().await;
    result
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 17 + 3) % 251) as u8).collect()
}

// ------------------------------------------------------------- transfers

#[tokio::test]
async fn a_paired_peer_sends_a_file_over_iroh() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default().transfers(&[true]);
    let mut listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

    let bytes = payload(300_000);
    let mut progress = Recorder::default();
    let sent = send(&alice, &bob, "hello.bin", &bytes, |_| {}, &mut progress)
        .await
        .expect("send");

    assert_eq!(sent.bytes_sent, bytes.len() as u64);
    match listening
        .expect("the receive", |e| matches!(e, ListenEvent::Received(_)))
        .await
    {
        ListenEvent::Received(summary) => assert_eq!(summary.peer_name, "alice"),
        _ => unreachable!(),
    }
    assert_eq!(std::fs::read(bob.inbox.join("hello.bin")).unwrap(), bytes);
    assert_eq!(prompt.asked(), ["transfer hello.bin from alice"]);
    // Loopback with the relay off: the path is direct, and says so.
    assert!(progress.0.contains(&Progress::Accepted {
        path: PathKind::Direct
    }));
}

/// ADR-0040: a new sender offers `beam/xfer/2` and `beam/xfer/1`. A current
/// `listen` picks 2, so several chunks go in flight; a receiver that only
/// knows 1 picks 1, so the sender falls back to one chunk at a time. (A
/// sender that only knows 1 is every other test in this file: they dial
/// `XFER_ALPN`.)
#[tokio::test]
async fn the_transfer_protocol_version_is_agreed_and_sets_the_window() {
    use beam::transport::dial::{dial_transfer, window_for};
    use beam::transport::endpoint::XFER_ALPN_V2;

    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let _listening = listen(&bob, Script::default(), options(&bob, Policy::default())).await;
    let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();

    let current = dial_transfer(&endpoint, bob.at()).await.expect("dial");
    assert_eq!(current.alpn(), XFER_ALPN_V2);
    assert_eq!(window_for(current.alpn()), beam::transfer::PIPELINE_WINDOW);

    // An old receiver: knows only version 1.
    let old = home("old");
    let old_endpoint = endpoint::bind(
        &old.identity,
        &Relay::Disabled,
        Bind::Loopback,
        0,
        &[XFER_ALPN],
    )
    .await
    .unwrap();
    let old_at = endpoint::advertised_addr(&old_endpoint, &Relay::Disabled, Bind::Loopback).await;
    let accepting = old_endpoint.clone();
    let held = tokio::spawn(async move { accepting.accept().await.unwrap().await.unwrap() });

    let fallback = dial_transfer(&endpoint, old_at)
        .await
        .expect("dial the old receiver");
    assert_eq!(fallback.alpn(), XFER_ALPN);
    assert_eq!(window_for(fallback.alpn()), 1);
    let _ = held.await;
    endpoint.close().await;
    old_endpoint.close().await;
}

#[derive(Default)]
struct Recorder(Vec<Progress>);

impl Reporter for Recorder {
    fn report(&mut self, progress: Progress) {
        self.0.push(progress);
    }
}

/// S-1 / S-2 over iroh: a decline saves nothing and the sender is told.
#[tokio::test]
async fn a_declined_transfer_saves_nothing() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let _listening = listen(
        &bob,
        Script::default().transfers(&[false]),
        options(&bob, Policy::default()),
    )
    .await;

    let sent = send(
        &alice,
        &bob,
        "no.bin",
        &payload(1000),
        |_| {},
        &mut SilentReporter,
    )
    .await;
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Declined))),
        "{sent:?}"
    );
    assert!(std::fs::read_dir(&bob.inbox).unwrap().next().is_none());
}

/// S-5 over iroh: an unanswered request expires and counts as a Reject.
#[tokio::test]
async fn an_unanswered_transfer_expires() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default();
    let _hold = prompt.hold_next_transfer();
    let mut opts = options(&bob, Policy::default());
    opts.accept_timeout = Duration::from_millis(500);
    let _listening = listen(&bob, prompt, opts).await;

    let sent = send(
        &alice,
        &bob,
        "late.bin",
        &payload(10),
        |_| {},
        &mut SilentReporter,
    )
    .await;
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Expired))),
        "{sent:?}"
    );
    assert!(std::fs::read_dir(&bob.inbox).unwrap().next().is_none());
}

/// S-7 over iroh, on the *proved* key: a device bob never paired with is
/// refused and bob is never asked.
#[tokio::test]
async fn an_unpaired_device_is_refused_without_a_prompt() {
    let (stranger, bob) = (home("stranger"), home("bob"));
    // The stranger knows bob; bob does not know the stranger.
    let mut known = stranger.store.load_known_peers().unwrap();
    known
        .add(Peer::new("bob", bob.identity.verifying_key()))
        .unwrap();
    stranger.store.save_known_peers(&known).unwrap();
    let prompt = Script::default().transfers(&[true]);
    let _listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

    let sent = send(
        &stranger,
        &bob,
        "x.bin",
        &payload(10),
        |_| {},
        &mut SilentReporter,
    )
    .await;
    assert!(
        matches!(
            sent,
            Err(TransferError::Rejected(RejectReason::UnknownPeer))
        ),
        "{sent:?}"
    );
    assert!(prompt.asked().is_empty(), "{:?}", prompt.asked());
}

/// ADR-0031: the transport's proof outranks the request. A paired device
/// that claims to be *another* paired device is refused, unasked.
#[tokio::test]
async fn a_paired_device_claiming_another_ones_key_is_refused() {
    let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    pair_by_hand(&carol, "bob", &bob, "carol");
    let prompt = Script::default().transfers(&[true]);
    let _listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

    // carol connects as herself but claims alice's key in the request.
    let sent = send(
        &carol,
        &bob,
        "forged.bin",
        &payload(10),
        |o| {
            o.sender_public_key = beam::identity::encode_public_key(&alice.identity.verifying_key())
        },
        &mut SilentReporter,
    )
    .await;
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::BadRequest))),
        "{sent:?}"
    );
    assert!(prompt.asked().is_empty(), "{:?}", prompt.asked());
}

/// S-3 / S-4 over iroh: file data before ACCEPT ends the transfer, and nothing
/// is kept.
#[tokio::test]
async fn data_before_accept_is_discarded() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default();
    let hold = prompt.hold_next_transfer();
    let mut listening = listen(&bob, prompt, options(&bob, Policy::default())).await;

    let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();
    let connection = dial(&endpoint, bob.at(), XFER_ALPN).await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    let data = payload(1000);
    let request = TransferRequest {
        transfer_id: TransferId::generate().unwrap(),
        sender_public_key: beam::identity::encode_public_key(&alice.identity.verifying_key()),
        file_name: "pushy.bin".into(),
        size: data.len() as u64,
        chunk_size: 1000,
        chunk_count: 1,
        file_sha256: beam::transfer::sha256_hex(&data),
    };
    write_message(&mut send, &Message::TransferRequest(request))
        .await
        .unwrap();
    // Do not wait for ACCEPT.
    write_message(
        &mut send,
        &Message::ChunkStart(ChunkStart {
            index: 0,
            len: data.len() as u32,
            sha256: beam::transfer::sha256_hex(&data),
        }),
    )
    .await
    .unwrap();

    let failed = listening
        .expect("the transfer to fail", |e| {
            matches!(e, ListenEvent::TransferFailed { .. })
        })
        .await;
    drop(hold);
    let ListenEvent::TransferFailed { error, .. } = failed else {
        unreachable!()
    };
    assert!(
        error.contains("before the transfer was accepted"),
        "{error}"
    );
    // Whatever the receiver sends back, it is not an ACCEPT.
    if let Ok(reply) = read_message(&mut recv).await {
        assert!(!matches!(reply, Message::Accept(_)), "{reply:?}");
    }
    assert!(std::fs::read_dir(&bob.inbox).unwrap().next().is_none());
    endpoint.close().await;
}

/// Resume over iroh: an interrupted transfer keeps what it has, and sending
/// again asks again (S-2) and sends only the rest.
#[tokio::test]
async fn an_interrupted_transfer_resumes_with_a_new_accept() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default().transfers(&[true, true]);
    let mut listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

    let bytes = payload(2_000_000);
    let path = alice.files.join("big.bin");
    std::fs::write(&path, &bytes).unwrap();

    // First attempt: killed after a few chunks, as a crash would.
    let (progressed, mut five_chunks) = mpsc::unbounded_channel();
    let first = {
        let identity = alice.identity.clone();
        let bob_at = bob.at();
        let path = path.clone();
        tokio::spawn(async move {
            let endpoint = endpoint::bind(&identity, &Relay::Disabled, Bind::Loopback, 0, &[])
                .await
                .unwrap();
            let connection = dial(&endpoint, bob_at, XFER_ALPN).await.unwrap();
            let mut options = SendOptions::new(
                &path,
                beam::identity::encode_public_key(&identity.verifying_key()),
            );
            options.chunk_size = 10_000;
            let mut signal = SignalAfter(5, Some(progressed));
            let _ = send_on(&connection, &mut options, &mut signal).await;
            panic!("the first attempt should have been killed before it finished");
        })
    };
    tokio::time::timeout(PATIENCE, five_chunks.recv())
        .await
        .expect("five chunks")
        .expect("signal");
    first.abort();
    let _ = first.await;
    listening
        .expect("the first attempt to fail", |e| {
            matches!(e, ListenEvent::TransferFailed { .. })
        })
        .await;

    let sent = send(
        &alice,
        &bob,
        "big.bin",
        &bytes,
        |o| o.chunk_size = 10_000,
        &mut SilentReporter,
    )
    .await
    .expect("the resumed send");
    assert!(sent.bytes_skipped > 0, "nothing was resumed: {sent:?}");
    assert_eq!(std::fs::read(bob.inbox.join("big.bin")).unwrap(), bytes);
    assert_eq!(
        prompt.asked(),
        [
            "transfer big.bin from alice",
            "transfer big.bin from alice (resuming)"
        ]
    );
}

/// Says when `n` chunks have gone through, so the test can kill the sender.
struct SignalAfter(u32, Option<mpsc::UnboundedSender<()>>);

impl Reporter for SignalAfter {
    fn report(&mut self, progress: Progress) {
        if let Progress::Transferring { .. } = progress {
            self.0 = self.0.saturating_sub(1);
            if self.0 == 0
                && let Some(tx) = self.1.take()
            {
                let _ = tx.send(());
            }
        }
    }
}

/// ADR-0030: one transfer at a time. A second sender is told, in words, that
/// the receiver is busy — and the first is unaffected.
#[tokio::test]
async fn a_second_transfer_while_one_is_open_is_told_to_try_later() {
    let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    pair_by_hand(&carol, "bob", &bob, "carol");
    let prompt = Script::default();
    let release = prompt.hold_next_transfer();
    let mut listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

    // alice's request sits at the prompt, holding the slot.
    let first = {
        let (alice_ref, bob_ref) = (&alice, &bob);
        let bytes = payload(1000);
        async move {
            send(
                alice_ref,
                bob_ref,
                "first.bin",
                &bytes,
                |_| {},
                &mut SilentReporter,
            )
            .await
        }
    };
    let second = async {
        // Wait until alice's question is on screen.
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while prompt.asked().is_empty() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let busy = send(
            &carol,
            &bob,
            "second.bin",
            &payload(10),
            |_| {},
            &mut SilentReporter,
        )
        .await;
        release.send(true).unwrap();
        busy
    };
    let (first, busy) = tokio::join!(first, second);

    assert!(
        matches!(busy, Err(TransferError::Rejected(RejectReason::Busy))),
        "{busy:?}"
    );
    assert!(
        busy.unwrap_err()
            .to_string()
            .contains("receiving another file")
    );
    first.expect("the first transfer is unaffected");
    listening
        .expect("the turned-away event", |e| {
            matches!(e, ListenEvent::TransferTurnedAway { .. })
        })
        .await;
    assert_eq!(prompt.asked(), ["transfer first.bin from alice"]);
}

// ---------------------------------------------------------------- pairing

async fn join_listener(
    joiner: &Home,
    listener: &Home,
    code: PairingCode,
    answer: bool,
) -> Result<beam::pairing::Paired, PairError> {
    let known = joiner.store.load_known_peers().unwrap();
    let network = network();
    let invite = beam::invite::Invite::new(&listener.at());
    let pairing = Pairing {
        identity: &joiner.identity,
        known: &known,
        name: Some("bob"),
        network: &network,
        timeouts: Timeouts {
            code_ttl: Duration::from_secs(600),
            message: Duration::from_secs(10),
            decision: Duration::from_secs(10),
        },
    };
    let script = Script::default().pairings(&[answer]);
    join(
        &pairing,
        &invite,
        || async move { Ok(code) },
        script,
        |_| {},
    )
    .await
}

#[tokio::test]
async fn listen_pairs_and_names_the_device_from_its_hint() {
    let (alice, bob) = (home("alices-laptop"), home("bob"));
    let prompt = Script::default().pairings(&[true]);
    let mut listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

    let code = listening.code.clone();
    let paired = join_listener(&alice, &bob, code.clone(), true)
        .await
        .expect("pair");
    assert_eq!(paired.key, bob.identity.verifying_key());

    listening
        .expect("the pairing", |e| matches!(e, ListenEvent::Paired { .. }))
        .await;
    let known = bob.store.load_known_peers().unwrap();
    let saved = known.lookup("alices-laptop").expect("saved under the hint");
    assert_eq!(saved.public_key, alice.identity.verifying_key());

    // The code was used; listen issued a different one.
    let next = listening.next_code().await;
    assert_ne!(next, code);
}

/// Condition 1 of the M5 approval, end to end: three wrong codes switch
/// pairing off for the session; the right code then gets `Unavailable`
/// without using anything; transfers from paired peers still work.
#[tokio::test]
async fn three_failed_attempts_switch_pairing_off_but_not_transfers() {
    let (mallory, alice, bob) = (home("mallory"), home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let policy = Policy {
        first_backoff: Duration::from_millis(100),
        ..Policy::default()
    };
    let prompt = Script::default().pairings(&[true]).transfers(&[true]);
    let mut listening = listen(&bob, prompt.clone(), options(&bob, policy)).await;

    let mut code = listening.code.clone();
    for attempt in 1..=3 {
        let wrong = PairingCode::parse(&format!(
            "{:06}",
            (code.as_str().parse::<u32>().unwrap() + 1) % 1_000_000
        ))
        .unwrap();
        let result = join_listener(&mallory, &bob, wrong, true).await;
        assert!(
            matches!(result, Err(PairError::Pairing(PairingError::WrongCode))),
            "attempt {attempt}: {result:?}"
        );
        if attempt < 3 {
            listening
                .expect("a pause", |e| {
                    matches!(e, ListenEvent::PairingPaused { .. })
                })
                .await;
            code = listening.next_code().await;
        }
    }
    listening
        .expect("pairing to switch off", |e| {
            matches!(e, ListenEvent::PairingDisabled { failures: 3 })
        })
        .await;

    // Even the right code — the last one issued — gets nowhere now, and
    // nobody is asked.
    let dave = home("dave");
    let result = join_listener(&dave, &bob, code, true).await;
    match result {
        Err(PairError::Pairing(PairingError::Unavailable(reason))) => {
            assert!(reason.contains("restarted"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !prompt.asked().iter().any(|q| q.starts_with("pair")),
        "{:?}",
        prompt.asked()
    );

    // Transfers are untouched.
    send(
        &alice,
        &bob,
        "still.bin",
        &payload(100),
        |_| {},
        &mut SilentReporter,
    )
    .await
    .expect("a paired peer can still send");
}

/// A pairing attempt during a pause is told to wait, and does not count.
#[tokio::test]
async fn an_attempt_during_a_pause_is_refused_without_counting() {
    let (mallory, bob) = (home("mallory"), home("bob"));
    let policy = Policy {
        first_backoff: Duration::from_secs(30),
        ..Policy::default()
    };
    let mut listening = listen(&bob, Script::default(), options(&bob, policy)).await;

    let wrong = PairingCode::parse("000000").unwrap();
    let wrong = if wrong == listening.code {
        PairingCode::parse("000001").unwrap()
    } else {
        wrong
    };
    join_listener(&mallory, &bob, wrong, true)
        .await
        .unwrap_err();
    listening
        .expect("a pause", |e| {
            matches!(e, ListenEvent::PairingPaused { .. })
        })
        .await;

    for _ in 0..3 {
        let result =
            join_listener(&mallory, &bob, PairingCode::parse("123456").unwrap(), true).await;
        match result {
            Err(PairError::Pairing(PairingError::Unavailable(reason))) => {
                assert!(reason.contains("paused"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }
    // Three refusals during the pause did not switch pairing off.
    listening
        .expect("refusals", |e| {
            matches!(e, ListenEvent::PairingRefused { .. })
        })
        .await;
    let disabled = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            if let Some(ListenEvent::PairingDisabled { .. }) = listening.events.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(disabled.is_err(), "refusals were counted as failures");
}

/// Condition 2 of the M5 decisions: a pairing request and a transfer request
/// at once, through the real desk. Only one question is on screen at a time.
#[tokio::test]
async fn a_pairing_and_a_transfer_at_once_are_asked_one_at_a_time() {
    use beam::cli::desk::{DeskPrompt, testing};

    let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let (desk, typed, screen) = testing::desk();
    let prompt = DeskPrompt::new(desk, Duration::from_secs(20));

    let (tx, mut events) = mpsc::unbounded_channel();
    let task = {
        let identity = bob.identity.clone();
        let store = bob.store.clone();
        let network = network();
        let options = options(&bob, Policy::default());
        tokio::spawn(async move {
            let _ = run(
                identity,
                store,
                network,
                options,
                prompt,
                || SilentReporter,
                move |e| {
                    let _ = tx.send(e);
                },
            )
            .await;
        })
    };
    let code = match tokio::time::timeout(PATIENCE, events.recv()).await {
        Ok(Some(ListenEvent::Ready { code, invite, .. })) => {
            *bob.listening_at.lock().unwrap() = Some(invite.endpoint_addr());
            code
        }
        other => panic!("{other:?}"),
    };

    let pairing = join_listener(&carol, &bob, code, true);
    let report = payload(500);
    let mut silent = SilentReporter;
    let transfer = send(&alice, &bob, "report.pdf", &report, |_| {}, &mut silent);
    let driver = async {
        let screen = screen.clone();
        tokio::task::spawn_blocking(move || {
            // Whichever arrives first is on screen alone.
            let first_is_pairing = loop {
                let text = screen.text();
                if text.contains("PAIRING REQUEST") {
                    break true;
                }
                if text.contains("Incoming file") {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            std::thread::sleep(Duration::from_millis(1000));
            let text = screen.text();
            assert!(
                !(text.contains("PAIRING REQUEST") && text.contains("Incoming file")),
                "both questions were on screen at once:\n{text}"
            );
            typed
                .send(if first_is_pairing { "yes" } else { "y" }.into())
                .unwrap();
            let second = if first_is_pairing {
                "Incoming file"
            } else {
                "PAIRING REQUEST"
            };
            screen.wait_for(second, PATIENCE);
            typed
                .send(if first_is_pairing { "y" } else { "yes" }.into())
                .unwrap();
        })
        .await
        .unwrap();
    };
    let (paired, sent, ()) = tokio::join!(pairing, transfer, driver);
    paired.expect("pairing went through");
    sent.expect("the transfer went through");
    task.abort();
}

/// ADR-0031: `send` to a device that ran `beam init` again — so a different
/// key now answers at the saved address — reaches nobody.
#[tokio::test]
async fn dialling_a_key_nobody_holds_any_more_finds_nobody() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    // bob re-ran `beam init`: the device listening now has a new key.
    let reinit = home("bob-again");
    let _listening = listen(
        &reinit,
        Script::default(),
        options(&reinit, Policy::default()),
    )
    .await;

    let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();
    // The address bob's invite named, with bob's old key.
    let old = EndpointAddr::from_parts(
        endpoint::endpoint_id(&bob.identity.verifying_key()),
        reinit.at().addrs,
    );
    let result = dial(&endpoint, old, XFER_ALPN).await;
    assert!(
        matches!(result, Err(DialError::Unreachable(_))),
        "{result:?}"
    );
    endpoint.close().await;
}

/// M6 item 4: a paired peer that is accepted and then goes quiet — with its
/// connection alive — is dropped after the stall timeout, and the transfer
/// slot is free again for the next sender.
#[tokio::test]
async fn a_paired_peer_holding_the_slot_is_dropped_and_the_next_can_send() {
    let (mallory, alice, bob) = (home("mallory"), home("alice"), home("bob"));
    pair_by_hand(&mallory, "bob", &bob, "mallory");
    pair_by_hand(&alice, "bob", &bob, "alice");
    let mut opts = options(&bob, Policy::default());
    opts.stall_timeout = Duration::from_millis(800);
    let prompt = Script::default().transfers(&[true, true]);
    let mut listening = listen(&bob, prompt, opts).await;

    // mallory: a well-formed request, accepted, then nothing at all.
    let endpoint = endpoint::bind(&mallory.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();
    let connection = dial(&endpoint, bob.at(), XFER_ALPN).await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    let data = payload(1000);
    let request = TransferRequest {
        transfer_id: TransferId::generate().unwrap(),
        sender_public_key: beam::identity::encode_public_key(&mallory.identity.verifying_key()),
        file_name: "held.bin".into(),
        size: data.len() as u64,
        chunk_size: 1000,
        chunk_count: 1,
        file_sha256: beam::transfer::sha256_hex(&data),
    };
    write_message(&mut send, &Message::TransferRequest(request))
        .await
        .unwrap();
    assert!(matches!(
        read_message(&mut recv).await,
        Ok(Message::Accept(_))
    ));

    // While mallory holds it, alice is turned away...
    let busy = send_file_as(&alice, &bob).await;
    assert!(
        matches!(busy, Err(TransferError::Rejected(RejectReason::Busy))),
        "{busy:?}"
    );

    // ...until the stall timeout drops mallory, and then she gets through.
    match listening
        .expect("mallory to be dropped", |e| {
            matches!(e, ListenEvent::TransferFailed { .. })
        })
        .await
    {
        ListenEvent::TransferFailed { error, .. } => {
            assert!(error.contains("sent nothing"), "{error}")
        }
        _ => unreachable!(),
    }
    send_file_as(&alice, &bob)
        .await
        .expect("the slot is free again");
    drop((send, recv, connection));
    endpoint.close().await;
}

async fn send_file_as(
    from: &Home,
    to: &Home,
) -> Result<beam::transfer::SendSummary, TransferError> {
    send(
        from,
        to,
        "after.bin",
        &payload(100),
        |_| {},
        &mut SilentReporter,
    )
    .await
}

/// M6: impersonation, attempted at the transport level with real iroh
/// endpoints. Each test is one row of `docs/threat-model.md`. Together they
/// are the evidence that S-7a is met: a peer's identity is the key its
/// connection proved, and nothing a message says can stand in for that proof.
mod impersonation {
    use super::*;

    /// 1. An unknown key. A device bob never paired with connects and sends a
    ///    well-formed request under its own key: refused before any prompt.
    #[tokio::test]
    async fn an_unknown_key_is_refused_without_a_prompt() {
        let (stranger, bob) = (home("stranger"), home("bob"));
        let mut known = stranger.store.load_known_peers().unwrap();
        known
            .add(Peer::new("bob", bob.identity.verifying_key()))
            .unwrap();
        stranger.store.save_known_peers(&known).unwrap();
        let prompt = Script::default().transfers(&[true]);
        let _listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

        let sent = send(
            &stranger,
            &bob,
            "x.bin",
            &payload(10),
            |_| {},
            &mut SilentReporter,
        )
        .await;
        assert!(
            matches!(
                sent,
                Err(TransferError::Rejected(RejectReason::UnknownPeer))
            ),
            "{sent:?}"
        );
        assert!(prompt.asked().is_empty());
    }

    /// 2. A known key without its secret key. alice's public key is public:
    ///    the attacker puts it in a request. But it can only connect as
    ///    itself — an iroh endpoint's id *is* its secret key's public half —
    ///    so the connection proves the attacker's key, and the claim is
    ///    refused. This is the test the last `STRENGTHEN IN M6` marker in
    ///    `tests/transfer.rs` was waiting for.
    #[tokio::test]
    async fn a_known_public_key_without_its_secret_key_gets_nowhere() {
        let (alice, attacker, bob) = (home("alice"), home("attacker"), home("bob"));
        pair_by_hand(&alice, "bob", &bob, "alice");
        let mut known = attacker.store.load_known_peers().unwrap();
        known
            .add(Peer::new("bob", bob.identity.verifying_key()))
            .unwrap();
        attacker.store.save_known_peers(&known).unwrap();
        let prompt = Script::default().transfers(&[true]);
        let mut listening = listen(&bob, prompt.clone(), options(&bob, Policy::default())).await;

        let alices_key = beam::identity::encode_public_key(&alice.identity.verifying_key());
        let sent = send(
            &attacker,
            &bob,
            "from-alice.bin",
            &payload(10),
            |o| o.sender_public_key = alices_key,
            &mut SilentReporter,
        )
        .await;
        assert!(matches!(sent, Err(TransferError::Rejected(_))), "{sent:?}");
        assert!(prompt.asked().is_empty(), "{:?}", prompt.asked());
        assert!(std::fs::read_dir(&bob.inbox).unwrap().next().is_none());
        listening
            .expect("the refusal", |e| {
                matches!(e, ListenEvent::TransferFailed { .. })
            })
            .await;

        // And the endpoint id cannot be chosen: it follows from the secret.
        let attackers_endpoint =
            endpoint::bind(&attacker.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
                .await
                .unwrap();
        assert_ne!(
            attackers_endpoint.id(),
            endpoint::endpoint_id(&alice.identity.verifying_key())
        );
        attackers_endpoint.close().await;
    }

    /// An attacker endpoint on loopback that counts connections which
    /// complete their handshake.
    async fn attacker_endpoint(
        identity: &Identity,
    ) -> (iroh::Endpoint, std::net::SocketAddr, Arc<Mutex<usize>>) {
        let endpoint = endpoint::bind(identity, &Relay::Disabled, Bind::Loopback, 0, &[XFER_ALPN])
            .await
            .unwrap();
        let addr = endpoint::advertised_addr(&endpoint, &Relay::Disabled, Bind::Loopback).await;
        let socket = *addr.ip_addrs().next().unwrap();
        let completed = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&completed);
        let accepting = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                if incoming.await.is_ok() {
                    *counter.lock().unwrap() += 1;
                }
            }
        });
        (endpoint, socket, completed)
    }

    /// 3. A wrong address: bob's key, the attacker's socket — what a tampered
    ///    invite or a hand-edited `addrs=` would produce. alice dials the key
    ///    she paired with; the attacker cannot prove it, so the handshake
    ///    fails and not one byte of the file leaves alice.
    #[tokio::test]
    async fn a_wrong_address_for_a_paired_key_cannot_redirect_a_send() {
        let (alice, bob, attacker) = (home("alice"), home("bob"), home("attacker"));
        let (attackers, socket, completed) = attacker_endpoint(&attacker.identity).await;
        let lie = EndpointAddr::new(endpoint::endpoint_id(&bob.identity.verifying_key()))
            .with_ip_addr(socket);

        let alices = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
            .await
            .unwrap();
        let result = dial(&alices, lie, XFER_ALPN).await;
        assert!(
            matches!(result, Err(DialError::Unreachable(_))),
            "{result:?}"
        );
        assert_eq!(
            *completed.lock().unwrap(),
            0,
            "the attacker completed a handshake"
        );
        alices.close().await;
        attackers.close().await;
    }
}

// ------------------------------------------------- Ctrl+C on either side (ADR-0041)

/// Reports once, on the first chunk that has moved, then stays quiet.
struct FirstChunk(Option<mpsc::UnboundedSender<()>>);

impl Reporter for FirstChunk {
    fn report(&mut self, progress: Progress) {
        if matches!(progress, Progress::Transferring { .. })
            && let Some(tx) = self.0.take()
        {
            let _ = tx.send(());
        }
    }
}

/// How many chunks bob's partials hold, read from disk.
fn chunks_kept(home: &Home) -> u32 {
    beam::transfer::PartialStore::new(home.store.tmp_path())
        .list(Duration::from_secs(3600))
        .unwrap()
        .iter()
        .map(|p| p.have_chunks)
        .sum()
}

/// The receiver's user stops `listen` (Ctrl+C) while chunks are moving. The
/// sender is told at once, by name of side: `PeerInterrupted`, not a timeout.
/// `listen` reports which transfer it cancelled, and what arrived is kept.
#[tokio::test]
async fn stopping_listen_mid_transfer_tells_the_sender_and_keeps_what_arrived() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");

    let (moving_tx, mut moving) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (tx, mut events) = mpsc::unbounded_channel();
    let (identity, store) = (bob.identity.clone(), bob.store.clone());
    let opts = options(&bob, Policy::default());
    let task = tokio::spawn(async move {
        let _ = beam::listener::run_until(
            identity,
            store,
            network(),
            opts,
            Script::default().transfers(&[true]),
            move || FirstChunk(Some(moving_tx.clone())),
            move |event| {
                let _ = tx.send(event);
            },
            async {
                let _ = stop_rx.await;
            },
        )
        .await;
    });
    let mut listening = match tokio::time::timeout(PATIENCE, events.recv()).await {
        Ok(Some(ListenEvent::Ready { code, invite, .. })) => {
            *bob.listening_at.lock().unwrap() = Some(invite.endpoint_addr());
            Listening { events, code, task }
        }
        other => panic!("listen did not start: {other:?}"),
    };

    // Big enough to still be moving when the first chunk lands.
    let bytes = payload(48 * 1024 * 1024);
    let sending = {
        let path = alice.files.join("big.bin");
        std::fs::write(&path, &bytes).unwrap();
        let identity = alice.identity.clone();
        let at = bob.at();
        tokio::spawn(async move {
            let endpoint = endpoint::bind(&identity, &Relay::Disabled, Bind::Loopback, 0, &[])
                .await
                .unwrap();
            let connection = beam::transport::dial::dial_transfer(&endpoint, at)
                .await
                .unwrap();
            let mut options = SendOptions::new(
                path,
                beam::identity::encode_public_key(&identity.verifying_key()),
            );
            options.chunk_size = 1024 * 1024;
            let result = send_on(&connection, &mut options, &mut SilentReporter).await;
            endpoint.close().await;
            result
        })
    };

    tokio::time::timeout(PATIENCE, moving.recv())
        .await
        .expect("no chunk ever arrived");
    stop_tx.send(()).unwrap(); // Ctrl+C on bob's side

    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .expect("the sender was not told")
        .unwrap();
    assert!(
        matches!(sent, Err(TransferError::PeerInterrupted)),
        "expected PeerInterrupted, got {sent:?}"
    );
    match listening
        .expect("the stop", |e| matches!(e, ListenEvent::Stopped { .. }))
        .await
    {
        ListenEvent::Stopped { cancelled } => {
            assert_eq!(cancelled, [alice.identity.fingerprint()]);
        }
        _ => unreachable!(),
    }
    assert!(
        chunks_kept(&bob) > 0,
        "what arrived was not kept for a resume"
    );
    assert!(std::fs::read_dir(&bob.inbox).unwrap().next().is_none());
}

/// The sender's user stops `beam send` (Ctrl+C) while chunks are moving.
/// `listen` says the sender stopped it, rather than reporting a failure
/// after a timeout, and keeps what arrived.
#[tokio::test]
async fn a_sender_stopped_mid_transfer_is_reported_as_such_by_listen() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let mut listening = listen(
        &bob,
        Script::default().transfers(&[true]),
        options(&bob, Policy::default()),
    )
    .await;

    let bytes = payload(48 * 1024 * 1024);
    let path = alice.files.join("big.bin");
    std::fs::write(&path, &bytes).unwrap();
    let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();
    let connection = beam::transport::dial::dial_transfer(&endpoint, bob.at())
        .await
        .unwrap();
    let (moving_tx, mut moving) = mpsc::unbounded_channel();
    let sending = {
        let connection = connection.clone();
        let key = beam::identity::encode_public_key(&alice.identity.verifying_key());
        tokio::spawn(async move {
            let mut options = SendOptions::new(path, key);
            options.chunk_size = 1024 * 1024;
            send_on(&connection, &mut options, &mut FirstChunk(Some(moving_tx))).await
        })
    };

    tokio::time::timeout(PATIENCE, moving.recv())
        .await
        .expect("no chunk was ever acknowledged");
    beam::transport::dial::interrupt(&connection); // Ctrl+C on alice's side
    let _ = sending.await;

    let started = std::time::Instant::now();
    match listening
        .expect("the interruption", |e| {
            matches!(
                e,
                ListenEvent::TransferInterrupted { .. } | ListenEvent::TransferFailed { .. }
            )
        })
        .await
    {
        ListenEvent::TransferInterrupted { peer } => {
            assert_eq!(peer, alice.identity.fingerprint());
        }
        other => panic!("reported as a plain failure: {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "listen waited for a timeout instead of hearing the close"
    );
    endpoint.close().await;
    assert!(
        chunks_kept(&bob) > 0,
        "what arrived was not kept for a resume"
    );
}

/// Found by a real Ctrl+C: a sender still hashing a large file is connected
/// but has not opened its stream. Stopping `listen` then must still tell it
/// at once (not after it finishes hashing), name it as cancelled, and report
/// nothing more after `Stopped`.
#[tokio::test]
async fn stopping_listen_while_the_sender_is_still_hashing_tells_it_at_once() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (tx, mut events) = mpsc::unbounded_channel();
    let (identity, store) = (bob.identity.clone(), bob.store.clone());
    let opts = options(&bob, Policy::default());
    let _task = tokio::spawn(async move {
        let _ = beam::listener::run_until(
            identity,
            store,
            network(),
            opts,
            Script::default().transfers(&[true]),
            || SilentReporter,
            move |event| {
                let _ = tx.send(event);
            },
            async {
                let _ = stop_rx.await;
            },
        )
        .await;
    });
    match tokio::time::timeout(PATIENCE, events.recv()).await {
        Ok(Some(ListenEvent::Ready { invite, .. })) => {
            *bob.listening_at.lock().unwrap() = Some(invite.endpoint_addr());
        }
        other => panic!("listen did not start: {other:?}"),
    }

    // Large enough that hashing it takes several seconds in a test build.
    let path = alice.files.join("huge.bin");
    std::fs::write(&path, payload(512 * 1024 * 1024)).unwrap();
    let identity = alice.identity.clone();
    let at = bob.at();
    let sending = tokio::spawn(async move {
        let endpoint = endpoint::bind(&identity, &Relay::Disabled, Bind::Loopback, 0, &[])
            .await
            .unwrap();
        let connection = beam::transport::dial::dial_transfer(&endpoint, at)
            .await
            .unwrap();
        let mut options = SendOptions::new(
            path,
            beam::identity::encode_public_key(&identity.verifying_key()),
        );
        let result = send_on(&connection, &mut options, &mut SilentReporter).await;
        endpoint.close().await;
        result
    });

    tokio::time::sleep(Duration::from_secs(1)).await; // connected, still hashing
    let stopped_at = std::time::Instant::now();
    stop_tx.send(()).unwrap();

    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(sent, Err(TransferError::PeerInterrupted)),
        "expected PeerInterrupted, got {sent:?}"
    );
    assert!(
        stopped_at.elapsed() < Duration::from_secs(4),
        "the sender only noticed after hashing ({:?})",
        stopped_at.elapsed()
    );

    let mut after = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(500), events.recv()).await
    {
        after.push(event);
    }
    match after.as_slice() {
        [ListenEvent::Stopped { cancelled }] => {
            assert_eq!(cancelled, &[alice.identity.fingerprint()]);
        }
        other => panic!("expected exactly one Stopped, got {other:?}"),
    }
}
