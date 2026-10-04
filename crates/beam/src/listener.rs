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
//! Nothing registers anywhere. `listen` shows an invite — its key, relay and
//! direct addresses — and paired peers reach it by its key through its relay
//! or at the addresses they saved (ADR-0036).
//!
//! The service lives in the library, not the CLI, so that tests can run it
//! against real iroh endpoints.

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::Connection;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::config::Relay;
use crate::identity::{Fingerprint, Identity, Peer, Store};
use crate::invite::Invite;
use crate::pairing::rotation::{NewCodeReason, Notice, Policy, Rotation, Unavailable};
use crate::pairing::{
    Confirm, Network, Pairing, PairingCode, Timeouts, attempt_kind, refuse_connection, serve,
};
use crate::transfer::{
    Prompt, ReceiveOptions, ReceiveSummary, RejectReason, Reporter, TransferId, receive_file,
    turn_away,
};
use crate::transport::dial::{interrupt, peer_interrupted, watch_route};
use crate::transport::endpoint::{
    self, EndpointError, PAIR_ALPN, XFER_ALPN, XFER_ALPN_V2, verifying_key,
};

/// How long to wait for the peer to close after the last message, so that
/// message is not cut off by our own close.
const LINGER: Duration = Duration::from_secs(5);

/// When `listen` is stopped, how long a cancelled transfer gets to save what
/// it received, and the endpoint to deliver its close frames.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// What `listen` does, apart from the network.
#[derive(Clone, Debug)]
pub struct ListenOptions {
    pub out_dir: PathBuf,
    /// How long a transfer's Accept prompt waits (S-5).
    pub accept_timeout: Duration,
    /// How long an accepted transfer may go without a frame before it is
    /// dropped and the transfer slot freed (ADR-0033).
    pub stall_timeout: Duration,
    /// How the pairing code renews itself.
    pub pairing: Policy,
    /// Pairing message and decision timeouts.
    pub timeouts: Timeouts,
    /// Whether this listener pairs at all. The background agent does not: it
    /// takes transfers from paired devices only, so nobody who finds it can
    /// try pairing codes (ADR-0042). Off means the pairing protocol is not
    /// even offered in the handshake.
    pub allow_pairing: bool,
    /// Whether iroh may ask the router to forward the port (UPnP, NAT-PMP,
    /// PCP). `beam listen` does; the agent only if turned on (ADR-0042).
    pub port_mapping: bool,
}

/// Something worth telling the person watching `listen`.
#[derive(Clone, Debug)]
pub enum ListenEvent {
    /// Listening: the invite to give a new device, and the first code.
    Ready {
        invite: Invite,
        fingerprint: Fingerprint,
        code: PairingCode,
        relay: Relay,
    },
    /// The configured port was taken, so `listen` is on another one. Its
    /// invite works, but peers that saved the old address will reach it only
    /// through the relay until they get the new invite.
    PortTaken {
        wanted: u16,
        got: u16,
    },
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
    /// The sender's user stopped beam (Ctrl+C) mid-transfer (ADR-0041).
    TransferInterrupted {
        peer: Fingerprint,
    },
    /// `listen` was stopped by its user. `cancelled` holds the peers whose
    /// transfers were in progress; each was told (ADR-0041).
    Stopped {
        cancelled: Vec<Fingerprint>,
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
    /// Transfers in progress, to be told if `listen` is stopped.
    active: Mutex<Vec<(Fingerprint, Connection)>>,
    /// Set once `listen` is stopping, so a cancelled transfer is reported
    /// once, as part of [`ListenEvent::Stopped`], not also as a failure.
    stopping: AtomicBool,
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
    run_until(
        identity,
        store,
        network,
        options,
        prompt,
        reporter,
        events,
        std::future::pending(),
    )
    .await
}

/// Like [`run`], and stops cleanly when `stop` completes: what `beam listen`
/// does on Ctrl+C (ADR-0041). A transfer in progress is closed with
/// [`CLOSE_INTERRUPTED`](crate::transport::dial::CLOSE_INTERRUPTED), so the
/// sender is told at once; it keeps what it received, for a resume; and
/// [`ListenEvent::Stopped`] says which transfers were cancelled.
#[allow(clippy::too_many_arguments)]
pub async fn run_until<Q, R>(
    identity: Identity,
    store: Store,
    network: Network,
    options: ListenOptions,
    prompt: Q,
    reporter: impl Fn() -> R + Send + Sync + 'static,
    events: impl Fn(ListenEvent) + Send + Sync + 'static,
    stop: impl Future<Output = ()>,
) -> Result<(), ListenError>
where
    Q: Prompt + Confirm + Clone + Send + Sync + 'static,
    R: Reporter + Send + 'static,
{
    // Newest transfer protocol first: the TLS server picks the first of these
    // that the sender offers (ADR-0040).
    let alpns: &[&[u8]] = if options.allow_pairing {
        &[PAIR_ALPN, XFER_ALPN_V2, XFER_ALPN]
    } else {
        &[XFER_ALPN_V2, XFER_ALPN]
    };
    let endpoint = endpoint::bind_with(
        &identity,
        &network.relay,
        network.bind,
        network.port,
        alpns,
        options.port_mapping,
    )
    .await?;
    let got = endpoint::bound_port(&endpoint).unwrap_or(0);
    let port_taken = (network.port != 0 && got != network.port).then_some(got);
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
        active: Mutex::new(Vec::new()),
        stopping: AtomicBool::new(false),
    });

    // Dropping the set aborts everything in it, so nothing outlives `run`.
    let mut tasks = JoinSet::new();
    tasks.spawn(tick_codes(Arc::clone(&shared)));

    if let Some(got) = port_taken {
        shared.tell(ListenEvent::PortTaken {
            wanted: shared.network.port,
            got,
        });
    }

    // The invite is only shown once the relay has had a chance to connect:
    // the relay URL is part of it, and is what lets a device on another
    // network reach this one. With no relay this is immediate.
    let invite = Invite::new(
        &endpoint::advertised_addr(&endpoint, &shared.network.relay, shared.network.bind).await,
    )
    .with_advertised(&shared.network.advertise);
    shared.tell(ListenEvent::Ready {
        invite,
        fingerprint: shared.identity.fingerprint(),
        code,
        relay: shared.network.relay.clone(),
    });

    let mut stop = std::pin::pin!(stop);
    loop {
        let incoming = tokio::select! {
            incoming = endpoint.accept() => incoming,
            () = &mut stop => break,
        };
        let Some(incoming) = incoming else {
            return Ok(());
        };
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
            } else if alpn == XFER_ALPN_V2 || alpn == XFER_ALPN {
                // One receiver serves both: it accepts any missing chunk in
                // any order, of which one at a time is a special case.
                handle_transfer(&shared, connection).await;
            }
        });
    }

    // Stopped by the person at the keyboard. Tell every sender mid-transfer,
    // let its handler save what arrived (its slot frees when it has), and
    // give the close frames a moment to leave before the endpoint goes.
    shared.stopping.store(true, Ordering::SeqCst);
    let active: Vec<_> = std::mem::take(&mut *shared.active.lock().expect("not poisoned"));
    for (_, connection) in &active {
        interrupt(connection);
    }
    let _ = tokio::time::timeout(STOP_GRACE, shared.transfer_slot.acquire()).await;
    shared.tell(ListenEvent::Stopped {
        cancelled: active.into_iter().map(|(peer, _)| peer).collect(),
    });
    let _ = tokio::time::timeout(STOP_GRACE, endpoint.close()).await;
    Ok(())
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

    let Ok(permit) = Arc::clone(&shared.transfer_slot).try_acquire_owned() else {
        shared.tell(ListenEvent::TransferTurnedAway { peer });
        if let Ok((send, recv)) = connection.accept_bi().await {
            let _ = turn_away(tokio::io::join(recv, send), RejectReason::Busy, timeout).await;
        }
        linger_then_close(&connection).await;
        return;
    };

    // Counted as in progress from the moment it holds the slot: a sender
    // still hashing its file has not opened its stream yet, and a stop must
    // tell it too (ADR-0041).
    shared
        .active
        .lock()
        .expect("not poisoned")
        .push((peer, connection.clone()));

    let (send, recv) = match connection.accept_bi().await {
        Ok(halves) => halves,
        Err(e) => {
            forget(shared, &connection);
            drop(permit);
            report_failure(shared, &connection, peer, e.to_string());
            return;
        }
    };

    let mut options = ReceiveOptions::new(&shared.options.out_dir, shared.store.tmp_path());
    options.accept_timeout = timeout;
    options.stall_timeout = shared.options.stall_timeout;
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
    forget(shared, &connection);
    // The transfer is over, whatever the peer does next: free the slot now,
    // not after lingering for its close below.
    drop(permit);

    match result {
        Ok(summary) => shared.tell(ListenEvent::Received(summary)),
        Err(e) => {
            if shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            report_failure(shared, &connection, peer, e.to_string());
        }
    }
    linger_then_close(&connection).await;
}

/// No longer counts `connection` as a transfer in progress.
fn forget<Q, R>(shared: &Shared<Q, R>, connection: &Connection) {
    shared
        .active
        .lock()
        .expect("not poisoned")
        .retain(|(_, c)| c.stable_id() != connection.stable_id());
}

/// Says why a transfer ended early: the sender's user stopped it, or it
/// failed. Nothing, if `listen` itself is stopping: [`ListenEvent::Stopped`]
/// names those transfers, once.
fn report_failure<Q, R>(
    shared: &Shared<Q, R>,
    connection: &Connection,
    peer: Fingerprint,
    error: String,
) {
    if shared.stopping.load(Ordering::SeqCst) {
        return;
    }
    if peer_interrupted(connection) {
        shared.tell(ListenEvent::TransferInterrupted { peer });
    } else {
        shared.tell(ListenEvent::TransferFailed { peer, error });
    }
}

/// The sender closes once it has read our last message; wait for that rather
/// than cutting it off, but not forever.
async fn linger_then_close(connection: &Connection) {
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
    connection.close(0u32.into(), b"done");
}
