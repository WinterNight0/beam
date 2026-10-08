//! Throughput benchmark (performance plan, step 1). Not part of the normal
//! test run: it moves hundreds of megabytes and only means something in a
//! release build.
//!
//! ```text
//! cargo test --release --test throughput -- --ignored --nocapture
//! ```
//!
//! A real `listen` and a real sender talk over iroh on loopback, the same path
//! `beam send` takes, and the sender's progress is timed phase by phase. The
//! size is 512 MiB unless `BEAM_BENCH_MIB` says otherwise.
//!
//! Loopback has almost no round-trip time, so by default this measures beam's
//! own costs (hashing, framing, disk writes and flushes), not the network.
//! `BEAM_BENCH_RTT_MS=100` puts a proxy between the two that holds every UDP
//! packet for half that long in each direction, which is roughly what a
//! transfer through a distant relay sees. It adds delay only: no bandwidth
//! limit and no loss. Results are recorded in `docs/performance-plan.md`.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam::config::Relay;
use beam::identity::{Identity, Peer, Store};
use beam::listener::{ListenEvent, ListenOptions, run};
use beam::pairing::{Confirm, ConfirmRequest, Network, Policy, Timeouts};
use beam::transfer::{Progress, Prompt, PromptRequest, Reporter, SendOptions, SilentReporter};
use beam::transport::dial::{dial_transfer, send_on};
use beam::transport::endpoint::{self, Bind};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use tokio::sync::mpsc;

const MIB: u64 = 1024 * 1024;

/// Says yes to every transfer and no to every pairing.
#[derive(Clone)]
struct Yes;

impl Prompt for Yes {
    fn confirm(&mut self, _request: &PromptRequest) -> std::io::Result<bool> {
        Ok(true)
    }
}

impl Confirm for Yes {
    fn confirm(&mut self, _request: &ConfirmRequest) -> std::io::Result<bool> {
        Ok(false)
    }
}

/// When each phase of a send began and ended, as the sender saw it.
#[derive(Default)]
struct Clock {
    hashing: Option<Instant>,
    asked: Option<Instant>,
    accepted: Option<Instant>,
    last_chunk: Option<Instant>,
    peer_verifying: Option<Instant>,
}

impl Reporter for Clock {
    fn report(&mut self, progress: Progress) {
        let now = Instant::now();
        match progress {
            Progress::Hashing { .. } => {
                self.hashing.get_or_insert(now);
            }
            Progress::AwaitingAccept => self.asked = Some(now),
            Progress::Accepted { .. } => self.accepted = Some(now),
            Progress::Transferring { .. } => self.last_chunk = Some(now),
            Progress::PeerVerifying { .. } => {
                self.peer_verifying.get_or_insert(now);
            }
            _ => {}
        }
    }
}

fn rate(bytes: u64, took: Duration) -> String {
    let mb_s = bytes as f64 / 1e6 / took.as_secs_f64();
    format!("{:>7.2} s  {:>7.1} MB/s", took.as_secs_f64(), mb_s)
}

/// A UDP relay that delays every packet by `one_way`, keeping their order.
///
/// It faces the sender on IPv6 loopback and the listener on IPv4 loopback.
/// The two endpoints share no address family, so iroh cannot find a direct
/// path around the relay (its hole punching would otherwise find one at once
/// on loopback). Returns the address to dial instead of the listener's.
///
/// Plain threads, not async: a thread per direction does nothing but read, so
/// the small default UDP buffers never overflow, and one thread sends each
/// packet the moment it is due, by polling the clock rather than sleeping.
/// Async timers wake about once a millisecond (every ~15 ms on Windows) and
/// would release packets in bursts that a real link never produces. Polling
/// keeps one core busy, which is acceptable for a benchmark.
fn delay_proxy(listener: SocketAddr, one_way: Duration) -> SocketAddr {
    let front = std::net::UdpSocket::bind("[::1]:0").unwrap();
    let back = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let at = front.local_addr().unwrap();
    let to_listener: Queue = Arc::default();
    let to_sender: Queue = Arc::default();
    let sender = Arc::new(Mutex::new(None::<SocketAddr>));

    {
        let (socket, queue, seen) = (
            front.try_clone().unwrap(),
            to_listener.clone(),
            sender.clone(),
        );
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65_536];
            loop {
                // Windows reports a stray ICMP "port unreachable" as an error
                // on the next receive; it is not fatal, so keep going.
                let Ok((len, from)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                *seen.lock().unwrap() = Some(from);
                let due = Instant::now() + one_way;
                queue
                    .lock()
                    .unwrap()
                    .push_back((due, buf[..len].to_vec(), listener));
            }
        });
    }
    {
        let (socket, queue, sender) = (back.try_clone().unwrap(), to_sender.clone(), sender);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65_536];
            loop {
                let Ok((len, _)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                let Some(to) = *sender.lock().unwrap() else {
                    continue;
                };
                let due = Instant::now() + one_way;
                queue
                    .lock()
                    .unwrap()
                    .push_back((due, buf[..len].to_vec(), to));
            }
        });
    }
    std::thread::spawn(move || {
        loop {
            let mut idle = true;
            for (queue, socket) in [(&to_listener, &back), (&to_sender, &front)] {
                let now = Instant::now();
                loop {
                    let next = {
                        let mut queue = queue.lock().unwrap();
                        match queue.front() {
                            Some((due, _, _)) if *due <= now => queue.pop_front(),
                            _ => None,
                        }
                    };
                    let Some((_, packet, to)) = next else { break };
                    let _ = socket.send_to(&packet, to);
                    idle = false;
                }
            }
            if idle {
                std::hint::spin_loop();
            }
        }
    });
    at
}

type Queue = Arc<Mutex<VecDeque<(Instant, Vec<u8>, SocketAddr)>>>;

struct Home {
    _tmp: tempfile::TempDir,
    store: Store,
    identity: Identity,
    dir: PathBuf,
}

fn home(name: &str) -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("beam"));
    let identity = Identity::generate(name).unwrap();
    store.save_identity(&identity, false).unwrap();
    let dir = tmp.path().join("files");
    std::fs::create_dir_all(&dir).unwrap();
    Home {
        _tmp: tmp,
        store,
        identity,
        dir,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
async fn throughput_over_loopback_iroh() {
    let mib: u64 = std::env::var("BEAM_BENCH_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let size = mib * MIB;
    let rtt_ms: u64 = std::env::var("BEAM_BENCH_RTT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let (alice, bob) = (home("alice"), home("bob"));
    for (store, name, key) in [
        (&alice.store, "bob", bob.identity.verifying_key()),
        (&bob.store, "alice", alice.identity.verifying_key()),
    ] {
        let mut known = store.load_known_peers().unwrap();
        known.add(Peer::new(name, key)).unwrap();
        store.save_known_peers(&known).unwrap();
    }

    // Bytes that do not repeat on a short period, written once up front.
    let path = alice.dir.join("payload.bin");
    let bytes: Vec<u8> = (0..size)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(&path, &bytes).unwrap();
    drop(bytes);

    let (tx, mut events) = mpsc::unbounded_channel();
    let policy = Policy::default();
    let options = ListenOptions {
        out_dir: bob.dir.clone(),
        accept_timeout: Duration::from_secs(30),
        stall_timeout: Duration::from_secs(60),
        pairing: policy,
        timeouts: Timeouts {
            code_ttl: policy.code_ttl,
            message: Duration::from_secs(30),
            decision: Duration::from_secs(30),
        },
        allow_pairing: true,
        port_mapping: true,
    };
    let network = Network {
        relay: Relay::Disabled,
        bind: Bind::Loopback,
        port: 0,
        advertise: Vec::new(),
    };
    let (identity, store) = (bob.identity.clone(), bob.store.clone());
    let listener = tokio::spawn(async move {
        let _ = run(
            identity,
            store,
            network,
            options,
            Yes,
            || SilentReporter,
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await;
    });
    let at = match tokio::time::timeout(Duration::from_secs(30), events.recv()).await {
        Ok(Some(ListenEvent::Ready { invite, .. })) => invite.endpoint_addr(),
        other => panic!("listen did not start: {other:?}"),
    };

    let (endpoint, at) = if rtt_ms == 0 {
        let endpoint = endpoint::bind(&alice.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
            .await
            .unwrap();
        (endpoint, at)
    } else {
        // beam's QUIC settings, but on IPv6 loopback only (see `delay_proxy`).
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::from_bytes(
                &alice.identity.signing_key().to_bytes(),
            ))
            .relay_mode(RelayMode::Disabled)
            .transport_config(endpoint::transport_config())
            .clear_ip_transports()
            .bind_addr("[::1]:0".parse::<SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let listener = at.ip_addrs().next().copied().expect("a direct address");
        let proxy = delay_proxy(listener, Duration::from_millis(rtt_ms / 2));
        (endpoint, EndpointAddr::new(at.id).with_ip_addr(proxy))
    };
    let connection = dial_transfer(&endpoint, at).await.expect("dial");
    let mut send_options = SendOptions::new(
        path,
        beam::identity::encode_public_key(&alice.identity.verifying_key()),
    );
    let mut clock = Clock::default();
    let started = Instant::now();
    let sent = send_on(&connection, &mut send_options, &mut clock)
        .await
        .expect("send");
    let finished = Instant::now();
    let stats = connection.stats();
    endpoint.close().await;
    listener.abort();
    assert_eq!(sent.bytes_sent, size);

    let at = |t: Option<Instant>, what: &str| t.unwrap_or_else(|| panic!("no {what} reported"));
    let hashing = at(clock.hashing, "hashing");
    let asked = at(clock.asked, "request");
    let accepted = at(clock.accepted, "accept");
    let last_chunk = at(clock.last_chunk, "chunk");
    let verifying = clock.peer_verifying.unwrap_or(last_chunk);

    println!();
    println!("beam throughput, {mib} MiB over loopback iroh, {rtt_ms} ms added round trip");
    println!("  sender hashing   {}", rate(size, asked - hashing));
    println!("  transfer         {}", rate(size, last_chunk - accepted));
    println!("  receiver check   {}", rate(size, finished - verifying));
    println!("  whole send       {}", rate(size, finished - started));
    println!(
        "  packets lost     {} of {} sent ({:.2}%)",
        stats.lost_packets,
        stats.udp_tx.datagrams,
        100.0 * stats.lost_packets as f64 / stats.udp_tx.datagrams.max(1) as f64
    );
}
