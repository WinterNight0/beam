//! Pairing over the real thing: an in-process rendezvous server, two iroh
//! endpoints on loopback, and the two roles running concurrently.
//!
//! The protocol's own attack tests (a wrong code, a substituted key, a relay
//! in the middle) run over an in-memory pipe in `pairing::protocol`. These
//! tests prove the same properties survive being wired to the network, and
//! cover what only exists there: the rendezvous server, single-use codes as
//! seen from the outside, expiry, and the key that ends up being returned.

use std::time::{Duration, Instant};

use beam::config::Relay;
use beam::identity::{Fingerprint, Identity, KnownPeers, Peer};
use beam::pairing::{
    CodeSlot, CodeUnavailable, Confirm, ConfirmRequest, Event, Network, PairError, Pairing,
    PairingCode, PairingError, Role, Timeouts, join, wait,
};
use beam::rendezvous::proto::{ClientMessage, ServerMessage, sign_registration, unix_now};
use beam::rendezvous::{RendezvousClient, RendezvousError, ServerConfig, serve};
use beam::transport::endpoint::{Bind, endpoint_id};
use ed25519_dalek::VerifyingKey;
use futures_util::{SinkExt, StreamExt};
use iroh::EndpointAddr;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Starts a rendezvous server on a free loopback port and returns its URL.
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

fn quick() -> Timeouts {
    Timeouts {
        code_ttl: Duration::from_secs(60),
        message: Duration::from_secs(10),
        decision: Duration::from_secs(10),
    }
}

/// Answers the `[y/N]` question with a fixed answer, and records who asked.
#[derive(Clone)]
struct Answer {
    yes: bool,
    asked: std::sync::Arc<std::sync::Mutex<Vec<ConfirmRequest>>>,
}

impl Answer {
    fn yes() -> Self {
        Self {
            yes: true,
            asked: Default::default(),
        }
    }

    fn no() -> Self {
        Self {
            yes: false,
            ..Self::yes()
        }
    }

    fn questions(&self) -> Vec<ConfirmRequest> {
        self.asked.lock().unwrap().clone()
    }
}

impl Confirm for Answer {
    fn confirm(&mut self, request: &ConfirmRequest) -> std::io::Result<bool> {
        self.asked.lock().unwrap().push(request.clone());
        Ok(self.yes)
    }
}

/// A waiter running in the background, and the code it showed.
struct Waiting {
    code: PairingCode,
    task: tokio::task::JoinHandle<Result<VerifyingKey, PairError>>,
}

/// Starts `beam pair --wait` for `identity` and returns once the code is live.
async fn start_waiting(
    identity: &Identity,
    known: KnownPeers,
    url: &str,
    timeouts: Timeouts,
    answer: Answer,
) -> Waiting {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let identity = identity.clone();
    let network = network(url);
    let slot = CodeSlot::new(PairingCode::generate().unwrap(), timeouts.code_ttl);
    let task = tokio::spawn(async move {
        let pairing = Pairing {
            identity: &identity,
            known: &known,
            name: Some("joiner"),
            network: &network,
            timeouts,
        };
        wait(&pairing, slot, answer, move |event| {
            if let Event::Waiting { code, .. } = event {
                let _ = tx.send(code.clone());
            }
        })
        .await
        .map(|paired| paired.key)
    });
    let code = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the waiter never became ready")
        .expect("the waiter stopped before it was ready");
    Waiting { code, task }
}

async fn join_with(
    identity: &Identity,
    known: &KnownPeers,
    short_id: beam::identity::ShortId,
    url: &str,
    code: PairingCode,
    answer: Answer,
) -> Result<VerifyingKey, PairError> {
    let network = network(url);
    let pairing = Pairing {
        identity,
        known,
        name: Some("waiter"),
        network: &network,
        timeouts: quick(),
    };
    join(
        &pairing,
        short_id,
        || async move { Ok(code) },
        answer,
        |_| {},
    )
    .await
    .map(|paired| paired.key)
}

fn identity(comment: &str) -> Identity {
    Identity::generate(comment).unwrap()
}

#[tokio::test]
async fn pairing_returns_the_key_each_side_proved_on_the_connection() {
    let url = start_server().await;
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    let (joiner_answer, waiter_answer) = (Answer::yes(), Answer::yes());

    let waiting = start_waiting(
        &waiter,
        KnownPeers::with_header(),
        &url,
        quick(),
        waiter_answer.clone(),
    )
    .await;
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        waiter.short_id(),
        &url,
        waiting.code.clone(),
        joiner_answer.clone(),
    )
    .await;
    let waited = waiting.task.await.unwrap();

    // The key each side gets back is the other side's device key — which is
    // also its iroh endpoint id, i.e. what `remote_id()` returned.
    let joined = joined.unwrap();
    let waited = waited.unwrap();
    assert_eq!(joined, waiter.verifying_key());
    assert_eq!(waited, joiner.verifying_key());
    assert_eq!(endpoint_id(&joined), endpoint_id(&waiter.verifying_key()));

    // Both people were asked, and each was shown both fingerprints.
    let [asked_waiter] = waiter_answer.questions().try_into().unwrap();
    assert_eq!(asked_waiter.role, Role::Waiter);
    assert_eq!(asked_waiter.peer_fingerprint, joiner.fingerprint());
    assert_eq!(asked_waiter.own_fingerprint, waiter.fingerprint());
    let [asked_joiner] = joiner_answer.questions().try_into().unwrap();
    assert_eq!(asked_joiner.role, Role::Joiner);
    assert_eq!(asked_joiner.peer_fingerprint, waiter.fingerprint());
}

#[tokio::test]
async fn a_wrong_code_pairs_nobody_and_uses_the_code_up() {
    let url = start_server().await;
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    let (joiner_answer, waiter_answer) = (Answer::yes(), Answer::yes());

    let waiting = start_waiting(
        &waiter,
        KnownPeers::with_header(),
        &url,
        quick(),
        waiter_answer.clone(),
    )
    .await;
    let wrong = next_code(&waiting.code);
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        waiter.short_id(),
        &url,
        wrong,
        joiner_answer.clone(),
    )
    .await;
    let waited = waiting.task.await.unwrap();

    assert!(
        matches!(joined, Err(PairError::Pairing(PairingError::WrongCode))),
        "{joined:?}"
    );
    assert!(
        matches!(waited, Err(PairError::Pairing(PairingError::NotConfirmed))),
        "{waited:?}"
    );
    assert!(
        joiner_answer.questions().is_empty(),
        "the joiner was asked to confirm"
    );
    assert!(
        waiter_answer.questions().is_empty(),
        "the waiter was asked to confirm"
    );

    // The attempt used the code up: the waiter is gone from the rendezvous
    // server, so even the right code has nobody left to reach.
    let retry = join_with(
        &joiner,
        &KnownPeers::with_header(),
        waiter.short_id(),
        &url,
        waiting.code.clone(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(retry, Err(PairError::NotFound(_))), "{retry:?}");
}

#[tokio::test]
async fn an_expired_code_ends_the_wait_and_unregisters() {
    let url = start_server().await;
    let waiter = identity("waiter");
    let timeouts = Timeouts {
        code_ttl: Duration::from_millis(1500),
        ..quick()
    };
    let started = Instant::now();
    let waiting = start_waiting(
        &waiter,
        KnownPeers::with_header(),
        &url,
        timeouts,
        Answer::yes(),
    )
    .await;
    let waited = waiting.task.await.unwrap();

    assert!(
        matches!(waited, Err(PairError::Code(CodeUnavailable::Expired))),
        "{waited:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(1500));

    let mut client = RendezvousClient::connect(&url).await.unwrap();
    assert!(client.lookup(waiter.short_id()).await.unwrap().is_empty());
}

#[tokio::test]
async fn if_the_waiter_says_no_neither_side_pairs() {
    let url = start_server().await;
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    let waiting = start_waiting(
        &waiter,
        KnownPeers::with_header(),
        &url,
        quick(),
        Answer::no(),
    )
    .await;
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        waiter.short_id(),
        &url,
        waiting.code.clone(),
        Answer::yes(),
    )
    .await;
    let waited = waiting.task.await.unwrap();

    assert!(
        matches!(waited, Err(PairError::Pairing(PairingError::Declined))),
        "{waited:?}"
    );
    assert!(
        matches!(
            joined,
            Err(PairError::Pairing(PairingError::DeclinedByPeer))
        ),
        "{joined:?}"
    );
}

#[tokio::test]
async fn a_device_that_is_already_paired_is_not_offered_again() {
    let url = start_server().await;
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    // The waiter already has the joiner, under another name.
    let mut waiter_known = KnownPeers::with_header();
    waiter_known
        .add(Peer::new("old-name", joiner.verifying_key()))
        .unwrap();
    let waiter_answer = Answer::yes();

    let waiting = start_waiting(&waiter, waiter_known, &url, quick(), waiter_answer.clone()).await;
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        waiter.short_id(),
        &url,
        waiting.code.clone(),
        Answer::yes(),
    )
    .await;
    let waited = waiting.task.await.unwrap();

    assert!(
        matches!(&waited, Err(PairError::AlreadyPaired(name)) if name == "old-name"),
        "{waited:?}"
    );
    assert!(joined.is_err(), "{joined:?}");
    assert!(
        waiter_answer.questions().is_empty(),
        "asked a question whose yes could not be honoured"
    );
}

#[tokio::test]
async fn a_name_that_is_taken_fails_before_the_network() {
    let joiner = identity("joiner");
    let mut known = KnownPeers::with_header();
    known
        .add(Peer::new("waiter", identity("someone").verifying_key()))
        .unwrap();
    // No server is running at this URL; the name check must come first.
    let result = join_with(
        &joiner,
        &known,
        identity("waiter").short_id(),
        "ws://127.0.0.1:1/v1",
        PairingCode::parse("123456").unwrap(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(result, Err(PairError::NameTaken(_))), "{result:?}");
}

#[tokio::test]
async fn an_unknown_short_id_is_reported_as_not_waiting() {
    let url = start_server().await;
    let result = join_with(
        &identity("joiner"),
        &KnownPeers::with_header(),
        identity("nobody").short_id(),
        &url,
        PairingCode::parse("123456").unwrap(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(result, Err(PairError::NotFound(_))), "{result:?}");
    assert!(result.unwrap_err().to_string().contains("beam pair --wait"));
}

#[tokio::test]
async fn a_server_that_is_down_is_reported_as_unreachable() {
    // Bind and drop, so the port is very likely closed.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let url = format!("ws://127.0.0.1:{port}/v1");
    let result = join_with(
        &identity("joiner"),
        &KnownPeers::with_header(),
        identity("waiter").short_id(),
        &url,
        PairingCode::parse("123456").unwrap(),
        Answer::yes(),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(PairError::Rendezvous(RendezvousError::Unreachable { .. }))
        ),
        "{result:?}"
    );
    assert!(result.unwrap_err().to_string().contains("beam-server"));
}

#[tokio::test]
async fn pairing_with_your_own_short_id_is_refused() {
    let me = identity("me");
    let result = join_with(
        &me,
        &KnownPeers::with_header(),
        me.short_id(),
        "ws://127.0.0.1:1/v1",
        PairingCode::parse("123456").unwrap(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(result, Err(PairError::OwnShortId)), "{result:?}");
}

/// Sends one raw request to the server, bypassing the client's own checks.
async fn raw_request(url: &str, request: &ClientMessage) -> ServerMessage {
    let (mut ws, _) = tokio_websockets::ClientBuilder::new()
        .uri(url)
        .unwrap()
        .connect()
        .await
        .unwrap();
    let text = serde_json::to_string(request).unwrap();
    ws.send(tokio_websockets::Message::text(text))
        .await
        .unwrap();
    loop {
        let message = ws.next().await.unwrap().unwrap();
        if let Some(text) = message.as_text() {
            return serde_json::from_str(text).unwrap();
        }
    }
}

fn error_code(reply: &ServerMessage) -> &str {
    match reply {
        ServerMessage::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Condition 3 of the M4 approval, end to end: the server verifies the
/// signature, the timestamp and the Short ID derivation.
#[tokio::test]
async fn the_server_refuses_registrations_that_do_not_check_out() {
    let url = start_server().await;
    let alice = identity("alice");
    let addr = EndpointAddr::new(endpoint_id(&alice.verifying_key()))
        .with_ip_addr("127.0.0.1:9".parse().unwrap());

    // Signed a day ago.
    let stale = sign_registration(&alice, &addr, unix_now() - 86_400);
    assert_eq!(
        error_code(&raw_request(&url, &stale).await),
        "stale_timestamp"
    );

    // Signed now, then tampered with.
    let ClientMessage::Register { body, signature } = sign_registration(&alice, &addr, unix_now())
    else {
        unreachable!()
    };
    let tampered = ClientMessage::Register {
        body: body.replace("127.0.0.1:9", "127.0.0.1:10"),
        signature: signature.clone(),
    };
    assert_eq!(
        error_code(&raw_request(&url, &tampered).await),
        "bad_signature"
    );

    // Someone else's signature over alice's body.
    let bob = identity("bob");
    let ClientMessage::Register {
        signature: bobs, ..
    } = sign_registration(&bob, &addr, unix_now())
    else {
        unreachable!()
    };
    let forged = ClientMessage::Register {
        body: body.clone(),
        signature: bobs,
    };
    assert_eq!(
        error_code(&raw_request(&url, &forged).await),
        "bad_signature"
    );

    // Nothing was stored by any of those.
    let mut client = RendezvousClient::connect(&url).await.unwrap();
    assert!(client.lookup(alice.short_id()).await.unwrap().is_empty());

    // And the genuine one is accepted.
    let genuine = ClientMessage::Register { body, signature };
    assert!(matches!(
        raw_request(&url, &genuine).await,
        ServerMessage::Registered { ttl_secs: 90 }
    ));
}

#[tokio::test]
async fn a_registration_lasts_only_as_long_as_its_connection() {
    let url = start_server().await;
    let alice = identity("alice");
    let addr = EndpointAddr::new(endpoint_id(&alice.verifying_key()))
        .with_ip_addr("127.0.0.1:9".parse().unwrap());

    let mut registered = RendezvousClient::connect(&url).await.unwrap();
    registered.register(&alice, &addr).await.unwrap();

    let mut looker = RendezvousClient::connect(&url).await.unwrap();
    let found = looker.lookup(alice.short_id()).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].public_key, alice.verifying_key());
    assert_eq!(Fingerprint::of(&found[0].public_key), alice.fingerprint());

    registered.close().await;
    // The server notices the close asynchronously.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if looker.lookup(alice.short_id()).await.unwrap().is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the registration outlived its connection"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The code one higher than `code`, wrapping — certainly not `code`.
fn next_code(code: &PairingCode) -> PairingCode {
    let n: u32 = code.as_str().parse().unwrap();
    PairingCode::parse(&format!("{:06}", (n + 1) % 1_000_000)).unwrap()
}
