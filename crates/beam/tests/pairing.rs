//! Pairing over the real thing: two iroh endpoints on loopback, an invite
//! passed from one to the other, and the two roles running concurrently.
//!
//! The protocol's own attack tests (a wrong code, a substituted key, a relay
//! in the middle) run over an in-memory pipe in `pairing::protocol`. These
//! tests prove the same properties survive being wired to the network, and
//! cover what only exists there: the invite, single-use codes as seen from the
//! outside, expiry, and the key that ends up being returned.

use std::time::{Duration, Instant};

use beam::config::Relay;
use beam::identity::{Identity, KnownPeers, Peer};
use beam::invite::Invite;
use beam::pairing::{
    CodeSlot, CodeUnavailable, Confirm, ConfirmRequest, Event, Network, PairError, Pairing,
    PairingCode, PairingError, Role, Timeouts, join, wait,
};
use beam::transport::endpoint::{Bind, endpoint_id};
use ed25519_dalek::VerifyingKey;
use tokio::sync::mpsc;

fn network() -> Network {
    Network {
        relay: Relay::Disabled,
        bind: Bind::Loopback,
        port: 0,
        advertise: Vec::new(),
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

/// A waiter running in the background, and the invite and code it showed.
struct Waiting {
    invite: Invite,
    code: PairingCode,
    task: tokio::task::JoinHandle<Result<VerifyingKey, PairError>>,
}

/// Starts `beam pair --wait` for `identity` and returns once the code is live.
async fn start_waiting(
    identity: &Identity,
    known: KnownPeers,
    timeouts: Timeouts,
    answer: Answer,
) -> Waiting {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let identity = identity.clone();
    let slot = CodeSlot::new(PairingCode::generate().unwrap(), timeouts.code_ttl);
    let task = tokio::spawn(async move {
        let network = network();
        let pairing = Pairing {
            identity: &identity,
            known: &known,
            name: Some("joiner"),
            network: &network,
            timeouts,
        };
        wait(&pairing, slot, answer, move |event| {
            if let Event::Waiting { invite, code, .. } = event {
                let _ = tx.send((invite.clone(), code.clone()));
            }
        })
        .await
        .map(|paired| paired.key)
    });
    let (invite, code) = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the waiter never became ready")
        .expect("the waiter stopped before it was ready");
    Waiting { invite, code, task }
}

async fn join_with(
    identity: &Identity,
    known: &KnownPeers,
    invite: &Invite,
    code: PairingCode,
    answer: Answer,
) -> Result<VerifyingKey, PairError> {
    let network = network();
    let pairing = Pairing {
        identity,
        known,
        name: Some("waiter"),
        network: &network,
        timeouts: quick(),
    };
    join(&pairing, invite, || async move { Ok(code) }, answer, |_| {})
        .await
        .map(|paired| paired.key)
}

fn identity(comment: &str) -> Identity {
    Identity::generate(comment).unwrap()
}

#[tokio::test]
async fn pairing_returns_the_key_each_side_proved_on_the_connection() {
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    let (joiner_answer, waiter_answer) = (Answer::yes(), Answer::yes());

    let waiting = start_waiting(
        &waiter,
        KnownPeers::with_header(),
        quick(),
        waiter_answer.clone(),
    )
    .await;
    // The invite is the waiter's: its key, and an address that reaches it.
    assert_eq!(waiting.invite.key, waiter.verifying_key());
    assert!(!waiting.invite.addrs.is_empty(), "{:?}", waiting.invite);

    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        &waiting.invite,
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
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    let (joiner_answer, waiter_answer) = (Answer::yes(), Answer::yes());

    let waiting = start_waiting(
        &waiter,
        KnownPeers::with_header(),
        quick(),
        waiter_answer.clone(),
    )
    .await;
    let wrong = next_code(&waiting.code);
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        &waiting.invite,
        wrong,
        joiner_answer.clone(),
    )
    .await;
    // The attempt used the code up: the waiter has stopped, so even the right
    // code has nobody left to reach.
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
}

/// ADR-0036: an invite is a routing hint, not a credential. One whose key was
/// swapped for an attacker's — while its address still points at the real
/// waiter — cannot complete a handshake, and does not spend the code: the
/// genuine invite still works afterwards.
#[tokio::test]
async fn an_invite_with_a_swapped_key_reaches_nobody_and_spends_nothing() {
    let (joiner, waiter, attacker) = (identity("joiner"), identity("waiter"), identity("x"));
    let waiting = start_waiting(&waiter, KnownPeers::with_header(), quick(), Answer::yes()).await;

    let swapped = Invite {
        key: attacker.verifying_key(),
        ..waiting.invite.clone()
    };
    let result = join_with(
        &joiner,
        &KnownPeers::with_header(),
        &swapped,
        waiting.code.clone(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(result, Err(PairError::Connect(_))), "{result:?}");

    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        &waiting.invite,
        waiting.code.clone(),
        Answer::yes(),
    )
    .await;
    assert_eq!(joined.unwrap(), waiter.verifying_key());
    assert_eq!(waiting.task.await.unwrap().unwrap(), joiner.verifying_key());
}

/// ADR-0038: `advertise` puts an address beam cannot discover — a port
/// forwarded by hand, with no relay — first in the invite.
#[tokio::test]
async fn advertised_addresses_lead_the_invite() {
    let waiter = identity("waiter");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut network = network();
        network.advertise = vec!["203.0.113.7:7820".parse().unwrap()];
        let known = KnownPeers::with_header();
        let pairing = Pairing {
            identity: &waiter,
            known: &known,
            name: Some("joiner"),
            network: &network,
            timeouts: quick(),
        };
        let slot = CodeSlot::new(PairingCode::generate().unwrap(), Duration::from_secs(60));
        let _ = wait(&pairing, slot, Answer::yes(), move |event| {
            if let Event::Waiting { invite, .. } = event {
                let _ = tx.send(invite.clone());
            }
        })
        .await;
    });
    let invite = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert_eq!(invite.addrs[0], "203.0.113.7:7820".parse().unwrap());
    assert!(invite.addrs.len() > 1, "the discovered address is kept too");
}

#[tokio::test]
async fn an_expired_code_ends_the_wait() {
    let waiter = identity("waiter");
    let timeouts = Timeouts {
        code_ttl: Duration::from_millis(1500),
        ..quick()
    };
    let started = Instant::now();
    let waiting = start_waiting(&waiter, KnownPeers::with_header(), timeouts, Answer::yes()).await;
    let waited = waiting.task.await.unwrap();

    assert!(
        matches!(waited, Err(PairError::Code(CodeUnavailable::Expired))),
        "{waited:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(1500));
}

#[tokio::test]
async fn if_the_waiter_says_no_neither_side_pairs() {
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    let waiting = start_waiting(&waiter, KnownPeers::with_header(), quick(), Answer::no()).await;
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        &waiting.invite,
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
    let (joiner, waiter) = (identity("joiner"), identity("waiter"));
    // The waiter already has the joiner, under another name.
    let mut waiter_known = KnownPeers::with_header();
    waiter_known
        .add(Peer::new("old-name", joiner.verifying_key()))
        .unwrap();
    let waiter_answer = Answer::yes();

    let waiting = start_waiting(&waiter, waiter_known, quick(), waiter_answer.clone()).await;
    let joined = join_with(
        &joiner,
        &KnownPeers::with_header(),
        &waiting.invite,
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

/// An invite for a device nobody is running: no network is touched before
/// the name check, so a taken name fails at once.
fn invite_for(identity: &Identity) -> Invite {
    Invite {
        key: identity.verifying_key(),
        relay: None,
        addrs: vec!["127.0.0.1:9".parse().unwrap()],
    }
}

#[tokio::test]
async fn a_name_that_is_taken_fails_before_the_network() {
    let joiner = identity("joiner");
    let mut known = KnownPeers::with_header();
    known
        .add(Peer::new("waiter", identity("someone").verifying_key()))
        .unwrap();
    let result = join_with(
        &joiner,
        &known,
        &invite_for(&identity("waiter")),
        PairingCode::parse("123456").unwrap(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(result, Err(PairError::NameTaken(_))), "{result:?}");
}

#[tokio::test]
async fn pairing_with_your_own_invite_is_refused() {
    let me = identity("me");
    let result = join_with(
        &me,
        &KnownPeers::with_header(),
        &invite_for(&me),
        PairingCode::parse("123456").unwrap(),
        Answer::yes(),
    )
    .await;
    assert!(matches!(result, Err(PairError::OwnInvite)), "{result:?}");
}

/// The code one higher than `code`, wrapping — certainly not `code`.
fn next_code(code: &PairingCode) -> PairingCode {
    let n: u32 = code.as_str().parse().unwrap();
    PairingCode::parse(&format!("{:06}", (n + 1) % 1_000_000)).unwrap()
}
