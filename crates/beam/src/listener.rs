//! `beam listen`: one iroh endpoint serving pairing and transfers.
//!
//! The endpoint answers two ALPNs (ADR-0030):
//!
//! * `beam/pair/1` — pairing, with a code that renews itself and backs off
//!   after failures ([`crate::pairing::rotation`], S-23).
//! * `beam/xfer/1` — transfers, through the M2 engine unchanged. The sender is
//!   identified by the key the connection proved, **not** by what its request
//!   claims (ADR-0031). One transfer at a time; a second is told the receiver
//!   is busy.
//!
//! Every question to the person at the keyboard goes through one prompt, which
//! the CLI backs with the desk that shows one question at a time (S-24).
//!
//! The service lives in the library, not the CLI, so that tests can run it
//! against real iroh endpoints and a real rendezvous server.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::Endpoint;
use iroh::endpoint::Connection;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::config::Relay;
use crate::identity::{Fingerprint, Identity, Peer, ShortId, Store};
use crate::pairing::rotation::{NewCodeReason, Notice, Policy, Rotation, Unavailable};
use crate::pairing::{
    Confirm, Network, Pairing, PairingCode, Timeouts, attempt_kind, refuse_connection, serve,
};
use crate::rendezvous::{REFRESH_EVERY, RendezvousClient};
use crate::transfer::{
    Prompt, ReceiveOptions, ReceiveSummary, RejectReason, Reporter, TransferId, receive_file,
    turn_away,
};
use crate::transport::dial::watch_route;
use crate::transport::endpoint::{self, EndpointError, PAIR_ALPN, XFER_ALPN, verifying_key};

/// How long to wait before trying the rendezvous server again after it failed.
const RETRY_EVERY: Duration = Duration::from_secs(5);

/// How long to wait for the peer to close after the last message, so that
/// message is not cut off by our own close.
const LINGER: Duration = Duration::from_secs(5);

/// What `listen` does, apart from the network.
#[derive(Clone, Debug)]
pub struct ListenOptions {
    pub out_dir: PathBuf,
    /// How long a transfer's Accept prompt waits (S-5).
    pub accept_timeout: Duration,
    /// How the pairing code renews itself.
    pub pairing: Policy,
    /// Pairing message and decision timeouts.
    pub timeouts: Timeouts,
}

/// Something worth telling the person watching `listen`.
#[derive(Clone, Debug)]
pub enum ListenEvent {
    /// Listening, registered or not; the first code, if pairing is on.
    Ready {
        short_id: ShortId,
        fingerprint: Fingerprint,
        code: PairingCode,
        relay: Relay,
    },
    /// The rendezvous server could not be reached or refused us; retrying.
    RegistrationFailed(String),
    /// Registered again after a failure.
    Registered,
    NewCode {
        code: PairingCode,
        reason: NewCodeReason,
    },
    PairingPaused {
        failures: u32,
        wait: Duration,
    },
    PairingDisabled {
        failures: u32,
    },
    /// Someone connected to pair; the code is now used up.
    PairingAttempt {
        peer: Fingerprint,
    },
    /// A pairing attempt was turned away without using a code.
    PairingRefused {
        peer: Fingerprint,
        reason: Unavailable,
    },
    Paired {
        name: String,
        fingerprint: Fingerprint,
    },
    PairingFailed {
        peer: Fingerprint,
        error: String,
    },
    /// A transfer arrived while another was in progress.
    TransferTurnedAway {
        peer: Fingerprint,
    },
    Received(ReceiveSummary),
    TransferFailed {
        peer: Fingerprint,
        error: String,
    },
}

/// Why `listen` could not start.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error("could not draw a pairing code: {0}")]
    Random(getrandom::Error),
}

/// What every connection handler shares.
struct Shared<Q, R> {
    identity: Identity,
    store: Store,
    network: Network,
    options: ListenOptions,
    rotation: Mutex<Rotation>,
    /// One transfer at a time (ADR-0030).
    transfer_slot: Arc<Semaphore>,
    /// Transfer ids already handled in this session (S-11).
    seen: tokio::sync::Mutex<HashSet<TransferId>>,
    prompt: Q,
    reporter: Box<dyn Fn() -> R + Send + Sync>,
    events: Box<dyn Fn(ListenEvent) + Send + Sync>,
}

impl<Q, R> Shared<Q, R> {
    fn tell(&self, event: ListenEvent) {
        (self.events)(event);
    }

    fn tell_notice(&self, notice: Notice) {
        self.tell(match notice {
            Notice::NewCode { code, reason } => ListenEvent::NewCode { code, reason },
            Notice::CoolingDown { failures, wait } => ListenEvent::PairingPaused { failures, wait },
            Notice::Disabled { failures } => ListenEvent::PairingDisabled { failures },
        });
    }
}

/// Runs `listen` until the task is dropped.
///
/// `prompt` answers both transfer and pairing questions; `reporter` makes a
/// progress reporter for each transfer; `events` hears everything else.
pub async fn run<Q, R>(
    identity: Identity,
    store: Store,
    network: Network,
    options: ListenOptions,
    prompt: Q,
    reporter: impl Fn() -> R + Send + Sync + 'static,
    events: impl Fn(ListenEvent) + Send + Sync + 'static,
) -> Result<(), ListenError>
where
    Q: Prompt + Confirm + Clone + Send + Sync + 'static,
    R: Reporter + Send + 'static,
{
    let endpoint = endpoint::bind(
        &identity,
        &network.relay,
        network.bind,
        &[PAIR_ALPN, XFER_ALPN],
    )
    .await?;
    let (rotation, first) =
        Rotation::start(options.pairing, Instant::now()).map_err(ListenError::Random)?;
    let Notice::NewCode { code, .. } = first else {
        unreachable!("a rotation starts with a code")
    };

    let shared = Arc::new(Shared {
        identity,
        store,
        network,
        options,
        rotation: Mutex::new(rotation),
        transfer_slot: Arc::new(Semaphore::new(1)),
        seen: tokio::sync::Mutex::new(HashSet::new()),
        prompt,
        reporter: Box::new(reporter),
        events: Box::new(events),
    });

    // Dropping the set aborts everything in it, so nothing outlives `run`.
    let mut tasks = JoinSet::new();
    let (first_attempt, registered) = tokio::sync::oneshot::channel();
    tasks.spawn(keep_registered(
        Arc::clone(&shared),
        endpoint.clone(),
        first_attempt,
    ));
    tasks.spawn(tick_codes(Arc::clone(&shared)));

    // Only say "ready" once the first registration has been tried: before
    // that, nobody can find this device, and a Short ID shown too early sends
    // the other person to "nobody is waiting". With a relay this takes a few
    // seconds, because the relay's address is part of the registration.
    let _ = registered.await;

    shared.tell(ListenEvent::Ready {
        short_id: shared.identity.short_id(),
        fingerprint: shared.identity.fingerprint(),
        code,
        relay: shared.network.relay.clone(),
    });

    while let Some(incoming) = endpoint.accept().await {
        // Reap finished handlers so the set does not grow without bound.
        while tasks.try_join_next().is_some() {}
        let shared = Arc::clone(&shared);
        tasks.spawn(async move {
            // A handshake that fails reaches nothing: no code, no prompt.
            let Ok(connection) = incoming.await else {
                return;
            };
            let alpn = connection.alpn().to_vec();
            if alpn == PAIR_ALPN {
                handle_pairing(&shared, connection).await;
            } else if alpn == XFER_ALPN {
                handle_transfer(&shared, connection).await;
            }
        });
    }
    Ok(())
}

/// Registers with the rendezvous server and keeps the registration fresh,
/// reconnecting when the server goes away.
///
/// `first_attempt` fires once the first attempt has succeeded or failed.
async fn keep_registered<Q, R>(
    shared: Arc<Shared<Q, R>>,
    endpoint: Endpoint,
    first_attempt: tokio::sync::oneshot::Sender<()>,
) {
    let network = &shared.network;
    let mut failing = false;
    let mut first_attempt = Some(first_attempt);
    loop {
        let result = async {
            let mut client = RendezvousClient::connect(&network.rendezvous).await?;
            loop {
                let addr = endpoint::advertised_addr(&endpoint, &network.relay, network.bind).await;
                client.register(&shared.identity, &addr).await?;
                if let Some(first) = first_attempt.take() {
                    let _ = first.send(());
                }
                if failing {
                    failing = false;
                    shared.tell(ListenEvent::Registered);
                }
                tokio::time::sleep(REFRESH_EVERY).await;
            }
        }
        .await;
        let error: crate::rendezvous::RendezvousError = match result {
            Ok(()) => unreachable!("the refresh loop only ends with an error"),
            Err(e) => e,
        };
        if !failing {
            failing = true;
            shared.tell(ListenEvent::RegistrationFailed(error.to_string()));
        }
        if let Some(first) = first_attempt.take() {
            let _ = first.send(());
        }
        tokio::time::sleep(RETRY_EVERY).await;
    }
}

/// Renews an expired code and ends a cool-down, once a second.
async fn tick_codes<Q, R>(shared: Arc<Shared<Q, R>>) {
    let mut every = tokio::time::interval(Duration::from_secs(1));
    loop {
        every.tick().await;
        let notice = shared
            .rotation
            .lock()
            .expect("not poisoned")
            .tick(Instant::now());
        match notice {
            Ok(Some(notice)) => shared.tell_notice(notice),
            Ok(None) => {}
            Err(e) => shared.tell(ListenEvent::PairingFailed {
                peer: shared.identity.fingerprint(),
                error: format!("could not draw a new pairing code: {e}"),
            }),
        }
    }
}

async fn handle_pairing<Q, R>(shared: &Shared<Q, R>, connection: Connection)
where
    Q: Confirm + Clone,
{
    let peer_key = verifying_key(&connection.remote_id());
    let peer = Fingerprint::of(&peer_key);

    let taken = shared
        .rotation
        .lock()
        .expect("not poisoned")
        .take(Instant::now());
    let code = match taken {
        Ok(code) => code,
        Err(reason) => {
            shared.tell(ListenEvent::PairingRefused {
                peer,
                reason: reason.clone(),
            });
            refuse_connection(connection, &reason.to_string()).await;
            return;
        }
    };
    shared.tell(ListenEvent::PairingAttempt { peer });

    let result = match shared.store.load_known_peers() {
        Ok(known) => {
            let pairing = Pairing {
                identity: &shared.identity,
                known: &known,
                name: None,
                network: &shared.network,
                timeouts: shared.options.timeouts,
            };
            serve(connection, &pairing, &code, shared.prompt.clone()).await
        }
        Err(e) => {
            connection.close(1u32.into(), b"internal error");
            Err(crate::pairing::PairError::Io(std::io::Error::other(e)))
        }
    };

    let attempt = attempt_kind(&result);
    match result {
        Ok(paired) => match save_peer(&shared.store, &paired.name, paired.key) {
            Ok(()) => shared.tell(ListenEvent::Paired {
                name: paired.name,
                fingerprint: Fingerprint::of(&paired.key),
            }),
            Err(e) => shared.tell(ListenEvent::PairingFailed {
                peer,
                error: format!("both sides agreed, but saving failed: {e}"),
            }),
        },
        Err(e) => shared.tell(ListenEvent::PairingFailed {
            peer,
            error: e.to_string(),
        }),
    }

    let notice = shared
        .rotation
        .lock()
        .expect("not poisoned")
        .finish(attempt, Instant::now());
    match notice {
        Ok(notice) => shared.tell_notice(notice),
        Err(e) => shared.tell(ListenEvent::PairingFailed {
            peer,
            error: format!("could not draw a new pairing code: {e}"),
        }),
    }
}

/// Adds a freshly paired peer, reading `known_peers` again first: another
/// command may have changed it while the prompt was up.
pub fn save_peer(
    store: &Store,
    name: &str,
    key: ed25519_dalek::VerifyingKey,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut known = store.load_known_peers()?;
    known.add(Peer::new(name, key))?;
    store.save_known_peers(&known)?;
    Ok(())
}

async fn handle_transfer<Q, R>(shared: &Shared<Q, R>, connection: Connection)
where
    Q: Prompt + Clone + Send + 'static,
    R: Reporter + Send + 'static,
{
    let peer_key = verifying_key(&connection.remote_id());
    let peer = Fingerprint::of(&peer_key);
    let timeout = shared.options.accept_timeout;

    let known = match shared.store.load_known_peers() {
        Ok(known) => known,
        Err(e) => {
            shared.tell(ListenEvent::TransferFailed {
                peer,
                error: e.to_string(),
            });
            connection.close(1u32.into(), b"internal error");
            return;
        }
    };

    // S-7, on the proved key: a stranger is refused before it can hold the
    // transfer slot or learn whether we are busy, and nobody is asked.
    if known.lookup_key(&peer_key).is_none() {
        if let Ok((send, recv)) = connection.accept_bi().await {
            let _ = turn_away(
                tokio::io::join(recv, send),
                RejectReason::UnknownPeer,
                timeout,
            )
            .await;
        }
        shared.tell(ListenEvent::TransferFailed {
            peer,
            error: crate::transfer::TransferError::Rejected(RejectReason::UnknownPeer).to_string(),
        });
        linger_then_close(&connection).await;
        return;
    }

    let Ok(_permit) = Arc::clone(&shared.transfer_slot).try_acquire_owned() else {
        shared.tell(ListenEvent::TransferTurnedAway { peer });
        if let Ok((send, recv)) = connection.accept_bi().await {
            let _ = turn_away(tokio::io::join(recv, send), RejectReason::Busy, timeout).await;
        }
        linger_then_close(&connection).await;
        return;
    };

    let (send, recv) = match connection.accept_bi().await {
        Ok(halves) => halves,
        Err(e) => {
            shared.tell(ListenEvent::TransferFailed {
                peer,
                error: e.to_string(),
            });
            return;
        }
    };

    let mut options = ReceiveOptions::new(&shared.options.out_dir, shared.store.tmp_path());
    options.accept_timeout = timeout;
    options.route = watch_route(&connection);
    options.proven_sender = Some(peer_key);

    let mut reporter = (shared.reporter)();
    let result = {
        let mut seen = shared.seen.lock().await;
        receive_file(
            tokio::io::join(recv, send),
            &known,
            &options,
            shared.prompt.clone(),
            &mut reporter,
            &mut seen,
        )
        .await
    };
    drop(reporter);

    match result {
        Ok(summary) => shared.tell(ListenEvent::Received(summary)),
        Err(e) => shared.tell(ListenEvent::TransferFailed {
            peer,
            error: e.to_string(),
        }),
    }
    linger_then_close(&connection).await;
}

/// The sender closes once it has read our last message; wait for that rather
/// than cutting it off, but not forever.
async fn linger_then_close(connection: &Connection) {
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
    connection.close(0u32.into(), b"done");
}
