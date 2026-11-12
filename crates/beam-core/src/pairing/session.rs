//! Pairing over the network: the invite, the iroh connection, and the
//! protocol run, for each of the two roles.
//!
//! * The **waiter** shows an invite and a code and takes attempts on its
//!   endpoint. `beam pair --wait` takes exactly one and exits ([`wait`]).
//!   `beam listen` takes them for as long as it runs, renewing the code by the
//!   rules in [`super::rotation`], and calls [`serve`] or [`refuse_connection`]
//!   for each.
//! * The **joiner** (`beam pair <INVITE>`) connects to the address in the
//!   invite and runs the protocol with the code the user typed ([`join`]).
//!
//! No server is involved: the invite carries the waiter's key, relay and
//! direct addresses (ADR-0036).
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
use crate::invite::Invite;
use crate::transport::endpoint::{self, Bind, EndpointError, PAIR_ALPN};

/// How long the joiner gives the waiter to answer.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for the last message to be acknowledged before closing.
const LINGER: Duration = Duration::from_secs(3);

/// How this device uses the network.
#[derive(Clone, Debug)]
pub struct Network {
    pub relay: Relay,
    pub bind: Bind,
    /// The UDP port a waiting device binds, or 0 for a random one. Dialling
    /// always uses a random port.
    pub port: u16,
    /// Addresses a waiting device puts first in its invite (ADR-0038).
    pub advertise: Vec<std::net::SocketAddr>,
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
    /// The waiter's endpoint is up and the code is live.
    Waiting {
        invite: &'a Invite,
        code: &'a PairingCode,
        expires_in: Duration,
    },
    /// Someone connected to the waiter; the code is now spent.
    Attempt { peer: Fingerprint },
    /// The joiner is connecting to the device in the invite.
    Connecting { peer: Fingerprint },
}

/// Why pairing did not complete. In every case nothing has been saved.
#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error("{0}; run `beam pair --wait` again for a new code")]
    Code(#[from] CodeUnavailable),
    #[error(
        "could not connect to the device in the invite ({0}). Check that it is still \
         running `beam listen` (or `beam pair --wait`) and that this is the invite it \
         shows now"
    )]
    Connect(String),
    #[error(transparent)]
    Pairing(#[from] PairingError),
    #[error("that device is already paired as {0:?}; nothing was changed")]
    AlreadyPaired(String),
    #[error(
        "a peer named {0:?} already exists; choose another name.\n       \
         WARNING: if you are pairing again because {0} has a new key, remove the old one\n       \
         first (`beam remove {0}`) — but only after checking the new fingerprint with {0}\n       \
         in person. Someone who wanted to impersonate {0} would ask you to re-pair too."
    )]
    NameTaken(String),
    #[error("this is this device's own invite")]
    OwnInvite,
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

/// `beam pair --wait`: shows the invite and the code, and takes one attempt.
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

    let endpoint = endpoint::bind(
        identity,
        &network.relay,
        network.bind,
        network.port,
        &[PAIR_ALPN],
    )
    .await?;
    let result = async {
        let invite =
            Invite::new(&endpoint::advertised_addr(&endpoint, &network.relay, network.bind).await)
                .with_advertised(&network.advertise);
        {
            // Peek at the code only to show it; `take` is what spends it.
            let code = slot.peek().expect("a fresh slot holds its code");
            let expires_in = slot.expires_at().saturating_duration_since(Instant::now());
            events(Event::Waiting {
                invite: &invite,
                code: &code,
                expires_in,
            });
        }

        let expiry = tokio::time::Instant::from_std(slot.expires_at());
        let connection = loop {
            tokio::select! {
                _ = tokio::time::sleep_until(expiry) => {
                    return Err(PairError::Code(CodeUnavailable::Expired));
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

        events(Event::Attempt {
            peer: Fingerprint::of(&endpoint::verifying_key(&connection.remote_id())),
        });
        // This code gets exactly one attempt: the endpoint closes after it.
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

/// Pairs with the device in `invite`.
///
/// `read_code` is called before connecting: the waiter spends its code on the
/// first connection that arrives, and a person typing it must not be racing
/// the waiter's message timeout.
pub async fn join<C, R, Fut>(
    pairing: &Pairing<'_>,
    invite: &Invite,
    read_code: R,
    confirm: C,
    mut events: impl FnMut(Event<'_>),
) -> Result<Paired, PairError>
where
    C: Confirm,
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
    if invite.key == identity.verifying_key() {
        return Err(PairError::OwnInvite);
    }

    let code = read_code().await?;

    let endpoint = endpoint::bind(identity, &network.relay, network.bind, 0, &[]).await?;
    let result = async {
        events(Event::Connecting {
            peer: invite.fingerprint(),
        });
        let connection = tokio::time::timeout(
            CONNECT_TIMEOUT,
            endpoint.connect(invite.endpoint_addr(), PAIR_ALPN),
        )
        .await
        .map_err(|_| PairError::Connect("timed out".into()))?
        .map_err(|e| PairError::Connect(e.to_string()))?;

        // iroh dials by endpoint id, so a connection to anyone but the key in
        // the invite cannot complete the handshake. Checked anyway: this is
        // the key that ends up in known_peers.
        let peer_key = endpoint::verifying_key(&connection.remote_id());
        if peer_key != invite.key {
            connection.close(1u32.into(), b"wrong peer");
            return Err(PairError::Connect(
                "the device answered with a different key".into(),
            ));
        }

        let (send, recv) = connection
            .open_bi()
            .await
            .map_err(|e| PairError::Connect(e.to_string()))?;
        let mut session = session(
            Role::Joiner,
            invite.short_id(),
            identity,
            peer_key,
            timeouts,
        );
        let comment = identity.comment().trim();
        session.name_hint = (!comment.is_empty()).then(|| comment.to_string());
        run_on(connection, send, recv, &session, &code, pairing, confirm).await
    }
    .await;
    endpoint.close().await;
    result
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
