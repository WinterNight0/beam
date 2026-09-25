//! `beam listen` as a service, over real iroh endpoints on loopback and a real
//! rendezvous server in-process.
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
use beam::rendezvous::{ServerConfig, serve};
use beam::transfer::frame::{read_message, write_message};
use beam::transfer::message::{ChunkStart, Message, RejectReason, TransferId, TransferRequest};
use beam::transfer::{
    Progress, Prompt, PromptRequest, Reporter, SendOptions, SilentReporter, TransferError,
};
use beam::transport::PathKind;
use beam::transport::dial::{DialError, dial, send_on};
use beam::transport::endpoint::{self, Bind, XFER_ALPN};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

const PATIENCE: Duration = Duration::from_secs(30);

async fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, ServerConfig::default()));
    format!("ws://{addr}/v1")
}

fn network(url: &str) -> Network {
    Network {
        rendezvous: url.to_string(),
        relay: Relay::Disabled,
        bind: Bind::Loopback,
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
        pairing: policy,
        timeouts: Timeouts {
            code_ttl: policy.code_ttl,
            message: Duration::from_secs(10),
            decision: Duration::from_secs(10),
        },
    }
}

async fn listen(home: &Home, url: &str, prompt: Script, options: ListenOptions) -> Listening {
    let (tx, mut events) = mpsc::unbounded_channel();
    let identity = home.identity.clone();
    let store = home.store.clone();
    let network = network(url);
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
        Ok(Some(ListenEvent::Ready { code, .. })) => code,
        other => panic!("listen did not start: {other:?}"),
    };
    let mut listening = Listening { events, code, task };
    // Registered is only announced after a failure; give the first
    // registration a moment by looking ourselves up.
    wait_until_registered(url, &home.identity).await;
    let _ = &mut listening;
    listening
}

async fn wait_until_registered(url: &str, identity: &Identity) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let mut client = beam::rendezvous::RendezvousClient::connect(url)
            .await
            .unwrap();
        if client
            .lookup_key(&identity.verifying_key())
            .await
            .unwrap()
            .is_some()
        {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "never registered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Sends `bytes` as `name` from `from` to the peer it calls `to`.
async fn send(
    from: &Home,
    to: &Home,
    url: &str,
    name: &str,
    bytes: &[u8],
    tweak: impl FnOnce(&mut SendOptions),
    reporter: &mut impl Reporter,
) -> Result<beam::transfer::SendSummary, TransferError> {
    let path = from.files.join(name);
    std::fs::write(&path, bytes).unwrap();
    let endpoint = endpoint::bind(&from.identity, &Relay::Disabled, Bind::Loopback, &[])
        .await
        .unwrap();
    let connection = dial(&endpoint, url, &to.identity.verifying_key(), XFER_ALPN)
        .await
        .expect("dial");
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
    let url = start_server().await;
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default().transfers(&[true]);
    let mut listening = listen(&bob, &url, prompt.clone(), options(&bob, Policy::default())).await;

    let bytes = payload(300_000);
    let mut progress = Recorder::default();
    let sent = send(
        &alice,
        &bob,
        &url,
        "hello.bin",
        &bytes,
        |_| {},
        &mut progress,
    )
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
    let url = start_server().await;
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let _listening = listen(
        &bob,
        &url,
        Script::default().transfers(&[false]),
        options(&bob, Policy::default()),
    )
    .await;

    let sent = send(
        &alice,
        &bob,
        &url,
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
    let url = start_server().await;
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default();
    let _hold = prompt.hold_next_transfer();
    let mut opts = options(&bob, Policy::default());
    opts.accept_timeout = Duration::from_millis(500);
    let _listening = listen(&bob, &url, prompt, opts).await;

    let sent = send(
        &alice,
        &bob,
        &url,
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
    let url = start_server().await;
    let (stranger, bob) = (home("stranger"), home("bob"));
    // The stranger knows bob; bob does not know the stranger.
    let mut known = stranger.store.load_known_peers().unwrap();
    known
        .add(Peer::new("bob", bob.identity.verifying_key()))
        .unwrap();
    stranger.store.save_known_peers(&known).unwrap();
    let prompt = Script::default().transfers(&[true]);
    let _listening = listen(&bob, &url, prompt.clone(), options(&bob, Policy::default())).await;

    let sent = send(
        &stranger,
        &bob,
        &url,
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
    let url = start_server().await;
    let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    pair_by_hand(&carol, "bob", &bob, "carol");
    let prompt = Script::default().transfers(&[true]);
    let _listening = listen(&bob, &url, prompt.clone(), options(&bob, Policy::default())).await;

    // carol connects as herself but claims alice's key in the request.
    let sent = send(
        &carol,
        &bob,
        &url,
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
    let url = start_server().await;
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default();
    let hold = prompt.hold_next_transfer();
    let mut listening = listen(&bob, &url, prompt, options(&bob, Policy::default())).await;

    let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, &[])
        .await
        .unwrap();
    let connection = dial(&endpoint, &url, &bob.identity.verifying_key(), XFER_ALPN)
        .await
        .unwrap();
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
    let url = start_server().await;
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let prompt = Script::default().transfers(&[true, true]);
    let mut listening = listen(&bob, &url, prompt.clone(), options(&bob, Policy::default())).await;

    let bytes = payload(2_000_000);
    let path = alice.files.join("big.bin");
    std::fs::write(&path, &bytes).unwrap();

    // First attempt: killed after a few chunks, as a crash would.
    let (progressed, mut five_chunks) = mpsc::unbounded_channel();
    let first = {
        let identity = alice.identity.clone();
        let bob_key = bob.identity.verifying_key();
        let url = url.clone();
        let path = path.clone();
        tokio::spawn(async move {
            let endpoint = endpoint::bind(&identity, &Relay::Disabled, Bind::Loopback, &[])
                .await
                .unwrap();
            let connection = dial(&endpoint, &url, &bob_key, XFER_ALPN).await.unwrap();
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
        &url,
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
    let url = start_server().await;
    let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    pair_by_hand(&carol, "bob", &bob, "carol");
    let prompt = Script::default();
    let release = prompt.hold_next_transfer();
    let mut listening = listen(&bob, &url, prompt.clone(), options(&bob, Policy::default())).await;

    // alice's request sits at the prompt, holding the slot.
    let first = {
        let url = url.clone();
        let (alice_ref, bob_ref) = (&alice, &bob);
        let bytes = payload(1000);
        async move {
            send(
                alice_ref,
                bob_ref,
                &url,
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
            &url,
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
    url: &str,
    code: PairingCode,
    answer: bool,
) -> Result<beam::pairing::Paired, PairError> {
    let known = joiner.store.load_known_peers().unwrap();
    let network = network(url);
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
        listener.identity.short_id(),
        || async move { Ok(code) },
        script,
        |_| {},
    )
    .await
}

#[tokio::test]
async fn listen_pairs_and_names_the_device_from_its_hint() {
    let url = start_server().await;
    let (alice, bob) = (home("alices-laptop"), home("bob"));
    let prompt = Script::default().pairings(&[true]);
    let mut listening = listen(&bob, &url, prompt.clone(), options(&bob, Policy::default())).await;

    let code = listening.code.clone();
    let paired = join_listener(&alice, &bob, &url, code.clone(), true)
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
    let url = start_server().await;
    let (mallory, alice, bob) = (home("mallory"), home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let policy = Policy {
        first_backoff: Duration::from_millis(100),
        ..Policy::default()
    };
    let prompt = Script::default().pairings(&[true]).transfers(&[true]);
    let mut listening = listen(&bob, &url, prompt.clone(), options(&bob, policy)).await;

    let mut code = listening.code.clone();
    for attempt in 1..=3 {
        let wrong = PairingCode::parse(&format!(
            "{:06}",
            (code.as_str().parse::<u32>().unwrap() + 1) % 1_000_000
        ))
        .unwrap();
        let result = join_listener(&mallory, &bob, &url, wrong, true).await;
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
    let result = join_listener(&dave, &bob, &url, code, true).await;
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
        &url,
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
    let url = start_server().await;
    let (mallory, bob) = (home("mallory"), home("bob"));
    let policy = Policy {
        first_backoff: Duration::from_secs(30),
        ..Policy::default()
    };
    let mut listening = listen(&bob, &url, Script::default(), options(&bob, policy)).await;

    let wrong = PairingCode::parse("000000").unwrap();
    let wrong = if wrong == listening.code {
        PairingCode::parse("000001").unwrap()
    } else {
        wrong
    };
    join_listener(&mallory, &bob, &url, wrong, true)
        .await
        .unwrap_err();
    listening
        .expect("a pause", |e| {
            matches!(e, ListenEvent::PairingPaused { .. })
        })
        .await;

    for _ in 0..3 {
        let result = join_listener(
            &mallory,
            &bob,
            &url,
            PairingCode::parse("123456").unwrap(),
            true,
        )
        .await;
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

    let url = start_server().await;
    let (alice, carol, bob) = (home("alice"), home("carol"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    let (desk, typed, screen) = testing::desk();
    let prompt = DeskPrompt::new(desk, Duration::from_secs(20));

    let (tx, mut events) = mpsc::unbounded_channel();
    let task = {
        let identity = bob.identity.clone();
        let store = bob.store.clone();
        let network = network(&url);
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
        Ok(Some(ListenEvent::Ready { code, .. })) => code,
        other => panic!("{other:?}"),
    };
    wait_until_registered(&url, &bob.identity).await;

    let pairing = join_listener(&carol, &bob, &url, code, true);
    let report = payload(500);
    let mut silent = SilentReporter;
    let transfer = send(
        &alice,
        &bob,
        &url,
        "report.pdf",
        &report,
        |_| {},
        &mut silent,
    );
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

/// ADR-0031: `send` to a device that is not listening — or that ran `beam
/// init` again and so has a different key — finds nothing to dial.
#[tokio::test]
async fn dialling_a_key_nobody_holds_any_more_finds_nobody() {
    let url = start_server().await;
    let (alice, bob) = (home("alice"), home("bob"));
    pair_by_hand(&alice, "bob", &bob, "alice");
    // bob re-ran `beam init`: the device listening now has a new key.
    let reinit = home("bob-again");
    let _listening = listen(
        &reinit,
        &url,
        Script::default(),
        options(&reinit, Policy::default()),
    )
    .await;

    let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, &[])
        .await
        .unwrap();
    let result = dial(&endpoint, &url, &bob.identity.verifying_key(), XFER_ALPN).await;
    assert!(matches!(result, Err(DialError::NotListening)), "{result:?}");
    endpoint.close().await;
}
