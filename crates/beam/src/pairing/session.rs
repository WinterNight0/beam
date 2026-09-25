//! Pairing over the network: the rendezvous lookup, the iroh connection, and
//! the protocol run, for each of the two roles.
//!
//! * The **waiter** has a code and takes attempts on its endpoint. `beam pair
//!   --wait` takes exactly one and exits ([`wait`]). `beam listen` takes them
//!   for as long as it runs, renewing the code by the rules in
//!   [`super::rotation`], and calls [`serve`] or [`refuse_connection`] for each.
//! * The **joiner** (`beam pair <ID>`) looks the Short ID up, connects to each
//!   entry that checks out, and runs the protocol with the code the user typed
//!   ([`join`]).
//!
//! The peer key that comes back is always the iroh connection's `remote_id()`
//! — a key the peer proved it holds — and never a key taken from a message.

use std::future::Future;
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::io::Join;

use super::code::{CodeSlot, CodeUnavailable, PairingCode};
use super::protocol::{self, Offer, PairingError, Role, Session};
use super::rotation::Attempt;
use crate::config::Relay;
use crate::identity::{Fingerprint, Identity, KnownPeers, ShortId, validate_name};
use crate::rendezvous::{REFRESH_EVERY, RendezvousClient, RendezvousError};
use crate::transport::endpoint::{self, Bind, EndpointError, PAIR_ALPN};

/// How long the joiner gives one candidate address to connect.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for the last message to be acknowledged before closing.
const LINGER: Duration = Duration::from_secs(3);

/// Where the infrastructure is.
#[derive(Clone, Debug)]
pub struct Network {
    pub rendezvous: String,
    pub relay: Relay,
    pub bind: Bind,
}

/// Timeouts, overridable by tests.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub code_ttl: Duration,
    pub message: Duration,
    pub decision: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            code_ttl: super::code::DEFAULT_CODE_TTL,
            message: protocol::MESSAGE_TIMEOUT,
            decision: protocol::DECISION_TIMEOUT,
        }
    }
}

/// What the person at the keyboard is asked to confirm.
#[derive(Clone, Debug)]
pub struct ConfirmRequest {
    pub role: Role,
    /// The nickname the peer will be saved under.
    pub name: String,
    pub peer_fingerprint: Fingerprint,
    pub own_fingerprint: Fingerprint,
}

/// The pairing question. Blocking; it runs on a blocking thread.
pub trait Confirm: Send + 'static {
    fn confirm(&mut self, request: &ConfirmRequest) -> std::io::Result<bool>;
}

/// Things worth telling the user while pairing runs.
#[derive(Debug)]
pub enum Event<'a> {
    /// The waiter is registered and the code is live.
    Waiting {
        short_id: ShortId,
        code: &'a PairingCode,
        expires_in: Duration,
    },
    /// The waiter's registration could not be refreshed. It keeps waiting.
    RefreshFailed(&'a RendezvousError),
    /// Someone connected to the waiter; the code is now spent.
    Attempt { peer: Fingerprint },
    /// The joiner found this many devices under the Short ID.
    Found { count: usize },
    /// The joiner is connecting to one of them.
    Connecting { peer: Fingerprint },
    /// A candidate did not work out; the joiner moves on to the next.
    CandidateFailed { peer: Fingerprint, reason: String },
}

/// Why pairing did not complete. In every case nothing has been saved.
#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error(transparent)]
    Rendezvous(#[from] RendezvousError),
    #[error(
        "no device with Short ID {0} is waiting to pair. Ask the other person to run \
         `beam listen` (or `beam pair --wait`) and read you the Short ID it shows"
    )]
    NotFound(String),
    #[error("{0}; run `beam pair --wait` again for a new code")]
    Code(#[from] CodeUnavailable),
    #[error("could not connect to the device: {0}")]
    Connect(String),
    #[error(transparent)]
    Pairing(#[from] PairingError),
    #[error("that device is already paired as {0:?}; nothing was changed")]
    AlreadyPaired(String),
    #[error("a peer named {0:?} already exists; choose another name, or `beam remove {0}` first")]
    NameTaken(String),
    #[error("this is your own Short ID")]
    OwnShortId,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A pairing that both people confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    /// The key the iroh connection proved: what goes into `known_peers`.
    pub key: VerifyingKey,
    /// The nickname it goes in under.
    pub name: String,
}

/// What both roles need to know about this side of the pairing.
#[derive(Clone, Copy, Debug)]
pub struct Pairing<'a> {
    pub identity: &'a Identity,
    /// Used to refuse a taken name or an already-paired key before asking.
    pub known: &'a KnownPeers,
    /// The nickname the other device will be saved under. `None` for `beam
    /// listen`, which takes it from the joiner's hint ([`choose_name`]).
    pub name: Option<&'a str>,
    pub network: &'a Network,
    pub timeouts: Timeouts,
}

/// Checks that `name` is free before anything touches the network, so a
/// person does not go through the whole exchange only to be told at the end.
pub fn check_name(known: &KnownPeers, name: &str) -> Result<(), PairError> {
    validate_name(name).map_err(|e| PairError::Io(std::io::Error::other(e)))?;
    if known.lookup(name).is_some() {
        return Err(PairError::NameTaken(name.to_string()));
    }
    Ok(())
}

/// The nickname `beam listen` saves a new peer under: the joiner's own
/// suggestion, made into a valid, unused name, or `peer-<fingerprint>` when
/// there is nothing usable. It is only a label; `beam rename` changes it.
pub fn choose_name(known: &KnownPeers, hint: Option<&str>, key: &VerifyingKey) -> String {
    let from_hint: String = hint
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(32)
        .collect::<String>()
        .trim_matches(|c| matches!(c, '-' | '.'))
        .to_string();
    let fallback = format!("peer-{}", &Fingerprint::of(key).hex()[..8]);

    let base = if !from_hint.is_empty() && validate_name(&from_hint).is_ok() {
        from_hint
    } else {
        fallback
    };
    if known.lookup(&base).is_none() {
        return base;
    }
    for n in 2.. {
        let suffix = format!("-{n}");
        let stem: String = base.chars().take(32 - suffix.len()).collect();
        let candidate = format!("{stem}{suffix}");
        if known.lookup(&candidate).is_none() {
            return candidate;
        }
    }
    unreachable!("some suffix is free")
}

/// How an attempt counts towards the rotation's failure limit (S-23): only an
/// attempt in which the code was **not** proved is a guess.
pub fn attempt_kind(result: &Result<Paired, PairError>) -> Attempt {
    match result {
        Ok(_) => Attempt::Paired,
        Err(PairError::AlreadyPaired(_))
        | Err(PairError::Pairing(PairingError::Declined | PairingError::DeclinedByPeer)) => {
            Attempt::ProvedButRefused
        }
        Err(_) => Attempt::Failed,
    }
}

/// `beam pair --wait`: registers, shows the code, and takes one attempt.
pub async fn wait<C: Confirm + Clone>(
    pairing: &Pairing<'_>,
    mut slot: CodeSlot,
    confirm: C,
    mut events: impl FnMut(Event<'_>),
) -> Result<Paired, PairError> {
    let Pairing {
        identity,
        known,
        name,
        network,
        ..
    } = *pairing;
    if let Some(name) = name {
        check_name(known, name)?;
    }

    let endpoint = endpoint::bind(identity, &network.relay, network.bind, &[PAIR_ALPN]).await?;
    let result = async {
        let mut rendezvous = RendezvousClient::connect(&network.rendezvous).await?;
        let addr = endpoint::advertised_addr(&endpoint, &network.relay, network.bind).await;
        rendezvous.register(identity, &addr).await?;

        {
            // Peek at the code only to show it; `take` is what spends it.
            let code = slot.peek().expect("a fresh slot holds its code");
            let expires_in = slot.expires_at().saturating_duration_since(Instant::now());
            events(Event::Waiting {
                short_id: identity.short_id(),
                code: &code,
                expires_in,
            });
        }

        let expiry = tokio::time::Instant::from_std(slot.expires_at());
        let mut refresh = tokio::time::interval_at(
            tokio::time::Instant::now() + REFRESH_EVERY,
            REFRESH_EVERY,
        );

        let connection = loop {
            tokio::select! {
                _ = tokio::time::sleep_until(expiry) => {
                    return Err(PairError::Code(CodeUnavailable::Expired));
                }
                _ = refresh.tick() => {
                    let addr = endpoint::advertised_addr(&endpoint, &network.relay, network.bind).await;
                    if let Err(e) = rendezvous.register(identity, &addr).await {
                        events(Event::RefreshFailed(&e));
                    }
                }
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else {
                        return Err(PairError::Connect("the endpoint closed".into()));
                    };
                    // A handshake that fails never reaches the protocol, and
                    // does not use the code: nobody has guessed anything yet.
                    match incoming.await {
                        Ok(connection) => break connection,
                        Err(_) => continue,
                    }
                }
            }
        };
        // Stop being findable: this code gets exactly one attempt.
        rendezvous.close().await;

        events(Event::Attempt {
            peer: Fingerprint::of(&endpoint::verifying_key(&connection.remote_id())),
        });
        let code = slot.take(Instant::now())?;
        serve(connection, pairing, &code, confirm).await
    }
    .await;
    endpoint.close().await;
    result
}

/// Runs one waiter-side attempt on a connection that arrived on the pairing
/// ALPN, with a code already taken for it.
pub async fn serve<C: Confirm>(
    connection: Connection,
    pairing: &Pairing<'_>,
    code: &PairingCode,
    confirm: C,
) -> Result<Paired, PairError> {
    let peer_key = endpoint::verifying_key(&connection.remote_id());
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(|e| PairError::Connect(e.to_string()))?;
    let session = session(
        Role::Waiter,
        pairing.identity.short_id(),
        pairing.identity,
        peer_key,
        pairing.timeouts,
    );
    run_on(connection, send, recv, &session, code, pairing, confirm).await
}

/// Turns a pairing attempt away without spending a code: the waiter is
/// cooling down, switched off, or busy. The joiner is told `reason`.
pub async fn refuse_connection(connection: Connection, reason: &str) {
    if let Ok(Ok((mut send, recv))) =
        tokio::time::timeout(protocol::MESSAGE_TIMEOUT, connection.accept_bi()).await
    {
        let mut stream = tokio::io::join(recv, &mut send);
        let _ = protocol::refuse(&mut stream, reason).await;
        drop(stream);
        let _ = send.finish();
        let _ = tokio::time::timeout(LINGER, send.stopped()).await;
    }
    connection.close(0u32.into(), b"unavailable");
}

/// Looks up `short_id` and pairs with the device behind it.
///
/// `read_code` is called once, after the lookup has found someone, so a
/// mistyped Short ID fails before the person is asked for the code.
pub async fn join<C, R, Fut>(
    pairing: &Pairing<'_>,
    short_id: ShortId,
    read_code: R,
    confirm: C,
    mut events: impl FnMut(Event<'_>),
) -> Result<Paired, PairError>
where
    C: Confirm + Clone,
    R: FnOnce() -> Fut,
    Fut: Future<Output = std::io::Result<PairingCode>>,
{
    let Pairing {
        identity,
        known,
        name,
        network,
        timeouts,
    } = *pairing;
    if let Some(name) = name {
        check_name(known, name)?;
    }
    if short_id == identity.short_id() {
        return Err(PairError::OwnShortId);
    }

    let mut rendezvous = RendezvousClient::connect(&network.rendezvous).await?;
    let found = rendezvous.lookup(short_id).await;
    rendezvous.close().await;
    let candidates: Vec<_> = found?
        .into_iter()
        .filter(|f| f.public_key != identity.verifying_key())
        .collect();
    if candidates.is_empty() {
        return Err(PairError::NotFound(short_id.grouped()));
    }
    events(Event::Found {
        count: candidates.len(),
    });

    let code = read_code().await?;

    let endpoint = endpoint::bind(identity, &network.relay, network.bind, &[]).await?;
    let mut last_error = None;
    let mut result = Err(PairError::NotFound(short_id.grouped()));
    for candidate in candidates {
        let peer = Fingerprint::of(&candidate.public_key);
        events(Event::Connecting { peer });

        let connected = tokio::time::timeout(
            CONNECT_TIMEOUT,
            endpoint.connect(candidate.addr.clone(), PAIR_ALPN),
        )
        .await;
        let connection = match connected {
            Ok(Ok(connection)) => connection,
            Ok(Err(e)) => {
                events(Event::CandidateFailed {
                    peer,
                    reason: e.to_string(),
                });
                last_error = Some(e.to_string());
                continue;
            }
            Err(_) => {
                events(Event::CandidateFailed {
                    peer,
                    reason: "timed out".into(),
                });
                last_error = Some("timed out".into());
                continue;
            }
        };

        // iroh dials by endpoint id, so a connection to anyone but the key we
        // asked for cannot complete the handshake. Checked anyway: this is
        // the key that ends up in known_peers.
        let peer_key = endpoint::verifying_key(&connection.remote_id());
        if peer_key != candidate.public_key {
            connection.close(1u32.into(), b"wrong peer");
            last_error = Some("the device answered with a different key".into());
            continue;
        }

        let (send, recv) = match connection.open_bi().await {
            Ok(halves) => halves,
            Err(e) => {
                last_error = Some(e.to_string());
                continue;
            }
        };
        let mut session = session(Role::Joiner, short_id, identity, peer_key, timeouts);
        let comment = identity.comment().trim();
        session.name_hint = (!comment.is_empty()).then(|| comment.to_string());
        let outcome = run_on(
            connection,
            send,
            recv,
            &session,
            &code,
            pairing,
            confirm.clone(),
        )
        .await;

        match outcome {
            // With a collision, a device that does not know the code fails
            // here; the real one may still be further down the list.
            Err(PairError::Pairing(PairingError::WrongCode)) => {
                events(Event::CandidateFailed {
                    peer,
                    reason: "did not know the code".into(),
                });
                result = Err(PairError::Pairing(PairingError::WrongCode));
            }
            other => {
                result = other;
                break;
            }
        }
    }
    endpoint.close().await;

    match (&result, last_error) {
        (Err(PairError::NotFound(_)), Some(e)) => Err(PairError::Connect(e)),
        _ => result,
    }
}

fn session(
    role: Role,
    short_id: ShortId,
    identity: &Identity,
    peer_key: VerifyingKey,
    timeouts: Timeouts,
) -> Session {
    let mut session = Session::new(role, short_id, identity.verifying_key(), peer_key);
    session.message_timeout = timeouts.message;
    session.decision_timeout = timeouts.decision;
    session
}

/// Runs the protocol on an open stream, then makes sure the last message is
/// delivered before the connection goes away.
async fn run_on<C: Confirm>(
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    session: &Session,
    code: &PairingCode,
    pairing: &Pairing<'_>,
    confirm: C,
) -> Result<Paired, PairError> {
    let Pairing {
        identity,
        known,
        name,
        ..
    } = *pairing;
    let mut stream: Join<RecvStream, SendStream> = tokio::io::join(recv, send);

    let already_paired = std::sync::Mutex::new(None::<String>);
    let chosen = std::sync::Mutex::new(None::<String>);
    let decide = |offer: Offer| {
        // Checked before asking: a device that is already in known_peers
        // would be refused by `add` anyway, and the person should not be asked
        // a question whose "yes" cannot be honoured.
        let existing = known.lookup_key(&offer.key).map(|p| p.name.clone());
        let save_as = match name {
            Some(name) => name.to_string(),
            None => choose_name(known, offer.name_hint.as_deref(), &offer.key),
        };
        *chosen.lock().expect("not poisoned") = Some(save_as.clone());
        let request = ConfirmRequest {
            role: session.role,
            name: save_as,
            peer_fingerprint: Fingerprint::of(&offer.key),
            own_fingerprint: identity.fingerprint(),
        };
        let mut confirm = confirm;
        let already_paired = &already_paired;
        async move {
            if let Some(existing) = existing {
                *already_paired.lock().expect("not poisoned") = Some(existing);
                return Ok(false);
            }
            tokio::task::spawn_blocking(move || confirm.confirm(&request))
                .await
                .map_err(std::io::Error::other)?
        }
    };

    let outcome = protocol::run(&mut stream, session, code, decide).await;

    // Deliver our last message before closing, so the peer does not see a
    // reset instead of our decision.
    let (_recv, mut send) = stream.into_inner();
    let _ = send.finish();
    let _ = tokio::time::timeout(LINGER, send.stopped()).await;
    connection.close(0u32.into(), b"done");

    match outcome {
        Ok(key) => Ok(Paired {
            key,
            name: chosen
                .into_inner()
                .expect("not poisoned")
                .expect("a successful run asked decide"),
        }),
        Err(PairingError::Declined) => match already_paired.into_inner().expect("not poisoned") {
            Some(existing) => Err(PairError::AlreadyPaired(existing)),
            None => Err(PairError::Pairing(PairingError::Declined)),
        },
        Err(e) => Err(PairError::Pairing(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Peer, vectors};

    fn key() -> VerifyingKey {
        vectors::verifying_key("alpha")
    }

    #[test]
    fn a_usable_hint_becomes_the_name() {
        let known = KnownPeers::with_header();
        assert_eq!(
            choose_name(&known, Some("alices-laptop"), &key()),
            "alices-laptop"
        );
        assert_eq!(
            choose_name(&known, Some("DESKTOP-7Q2"), &key()),
            "DESKTOP-7Q2"
        );
    }

    #[test]
    fn an_unusable_hint_is_cleaned_up_or_replaced() {
        let known = KnownPeers::with_header();
        assert_eq!(
            choose_name(&known, Some("Alice's Mac"), &key()),
            "Alice-s-Mac"
        );
        let fallback = format!("peer-{}", &Fingerprint::of(&key()).hex()[..8]);
        for hint in [None, Some(""), Some("   "), Some("!!!"), Some("ÄÖÜ")] {
            assert_eq!(choose_name(&known, hint, &key()), fallback, "{hint:?}");
        }
        assert_eq!(choose_name(&known, Some(&"x".repeat(50)), &key()).len(), 32);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let mut known = KnownPeers::with_header();
        known
            .add(Peer::new("laptop", vectors::verifying_key("bravo")))
            .unwrap();
        assert_eq!(choose_name(&known, Some("laptop"), &key()), "laptop-2");
        assert_eq!(choose_name(&known, Some("LAPTOP"), &key()), "LAPTOP-2");
    }

    #[test]
    fn only_an_unproved_code_counts_as_a_guess() {
        let paired: Result<Paired, PairError> = Ok(Paired {
            key: key(),
            name: "x".into(),
        });
        assert_eq!(attempt_kind(&paired), Attempt::Paired);
        for refused in [
            PairError::AlreadyPaired("x".into()),
            PairError::Pairing(PairingError::Declined),
            PairError::Pairing(PairingError::DeclinedByPeer),
        ] {
            assert_eq!(attempt_kind(&Err(refused)), Attempt::ProvedButRefused);
        }
        for failed in [
            PairError::Pairing(PairingError::WrongCode),
            PairError::Pairing(PairingError::NotConfirmed),
            PairError::Pairing(PairingError::KeyMismatch),
            PairError::Pairing(PairingError::Closed),
            PairError::Pairing(PairingError::Timeout),
        ] {
            assert_eq!(attempt_kind(&Err(failed)), Attempt::Failed);
        }
    }
}
