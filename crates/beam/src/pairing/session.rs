//! Pairing over the network: the rendezvous lookup, the iroh connection, and
//! the protocol run, for each of the two roles.
//!
//! * The **waiter** (`beam pair --wait`) opens an endpoint, registers its
//!   Short ID with the rendezvous server, shows the code, and takes **one**
//!   attempt. Whatever happens in that attempt, the code is gone afterwards.
//! * The **joiner** (`beam pair <ID>`) looks the Short ID up, connects to each
//!   entry that checks out, and runs the protocol with the code the user typed.
//!
//! The peer key that comes back is always the iroh connection's `remote_id()`
//! — a key the peer proved it holds — and never a key taken from a message.

use std::future::Future;
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::io::Join;

use super::code::{CodeSlot, CodeUnavailable, PairingCode};
use super::protocol::{self, PairingError, Role, Session};
use crate::config::Relay;
use crate::identity::{Fingerprint, Identity, KnownPeers, ShortId};
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

/// The `[y/N]` question. Blocking; it runs on a blocking thread.
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
         `beam pair --wait` and read you the Short ID it shows"
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

/// What both roles need to know about this side of the pairing.
#[derive(Clone, Copy, Debug)]
pub struct Pairing<'a> {
    pub identity: &'a Identity,
    /// Used to refuse a taken name or an already-paired key before asking.
    pub known: &'a KnownPeers,
    /// The nickname the other device will be saved under.
    pub name: &'a str,
    pub network: &'a Network,
    pub timeouts: Timeouts,
}

/// Checks that `name` is free before anything touches the network, so a
/// person does not go through the whole exchange only to be told at the end.
pub fn check_name(known: &KnownPeers, name: &str) -> Result<(), PairError> {
    crate::identity::validate_name(name).map_err(|e| PairError::Io(std::io::Error::other(e)))?;
    if known.lookup(name).is_some() {
        return Err(PairError::NameTaken(name.to_string()));
    }
    Ok(())
}

/// Waits for one pairing attempt and runs it.
///
/// Returns the peer's proved key once both people have confirmed. The code in
/// `slot` is spent by the first connection, whatever its outcome.
pub async fn wait<C: Confirm + Clone>(
    pairing: &Pairing<'_>,
    mut slot: CodeSlot,
    confirm: C,
    mut events: impl FnMut(Event<'_>),
) -> Result<VerifyingKey, PairError> {
    let Pairing {
        identity,
        known,
        name,
        network,
        timeouts,
    } = *pairing;
    check_name(known, name)?;

    let endpoint = endpoint::bind(identity, &network.relay, network.bind, &[PAIR_ALPN]).await?;
    let result = async {
        let mut rendezvous = RendezvousClient::connect(&network.rendezvous).await?;
        let addr = endpoint::advertised_addr(&endpoint, &network.relay, network.bind).await;
        rendezvous.register(identity, &addr).await?;

        {
            // Peek at the code only to show it; `take` is what spends it.
            let code = slot_code_for_display(&slot);
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

        let peer_key = endpoint::verifying_key(&connection.remote_id());
        events(Event::Attempt {
            peer: Fingerprint::of(&peer_key),
        });
        let code = slot.take(Instant::now())?;

        let (send, recv) = connection
            .accept_bi()
            .await
            .map_err(|e| PairError::Connect(e.to_string()))?;
        let session = session(Role::Waiter, identity.short_id(), identity, peer_key, timeouts);
        run_on(connection, send, recv, &session, &code, pairing, confirm).await
    }
    .await;
    endpoint.close().await;
    result
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
) -> Result<VerifyingKey, PairError>
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
    check_name(known, name)?;
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
        let session = session(Role::Joiner, short_id, identity, peer_key, timeouts);
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
) -> Result<VerifyingKey, PairError> {
    let Pairing {
        identity,
        known,
        name,
        ..
    } = *pairing;
    let mut stream: Join<RecvStream, SendStream> = tokio::io::join(recv, send);

    let already_paired = std::sync::Mutex::new(None::<String>);
    let decide = |key: VerifyingKey| {
        // Checked before asking: a device that is already in known_peers
        // would be refused by `add` anyway, and the person should not be asked
        // a question whose "yes" cannot be honoured.
        let existing = known.lookup_key(&key).map(|p| p.name.clone());
        let request = ConfirmRequest {
            role: session.role,
            name: name.to_string(),
            peer_fingerprint: Fingerprint::of(&key),
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
        Ok(key) => Ok(key),
        Err(PairingError::Declined) => match already_paired.into_inner().expect("not poisoned") {
            Some(existing) => Err(PairError::AlreadyPaired(existing)),
            None => Err(PairError::Pairing(PairingError::Declined)),
        },
        Err(e) => Err(PairError::Pairing(e)),
    }
}

/// The code, for showing. [`CodeSlot`] only hands its code out through
/// `take`, so the waiter keeps a copy for display, made here once.
fn slot_code_for_display(slot: &CodeSlot) -> PairingCode {
    slot.peek().expect("a fresh slot holds its code")
}
