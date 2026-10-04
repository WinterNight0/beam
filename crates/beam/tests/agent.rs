//! The background agent (ADR-0042), over real iroh endpoints on loopback,
//! with `beam inbox` played by a test client on the agent's local port.
//!
//! What has to hold: a request waits for an answer from the inbox and only
//! that answer accepts it (rule 1); silence is a no; strangers cannot ask;
//! the agent does not pair; and nothing on the local port is said to, or
//! accepted from, a client without the token.

use std::path::PathBuf;
use std::time::Duration;

use beam::agent::ipc::{AgentMsg, Client, ClientMsg};
use beam::agent::status::{self, AgentStatus, Running};
use beam::agent::{self, AgentOptions};
use beam::config::Relay;
use beam::identity::{Identity, Peer, Store};
use beam::invite::Invite;
use beam::pairing::Network;
use beam::transfer::{RejectReason, SendOptions, SilentReporter, TransferError};
use beam::transport::dial::{dial_transfer, send_on};
use beam::transport::endpoint::{self, Bind, PAIR_ALPN};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PATIENCE: Duration = Duration::from_secs(30);

struct Home {
    _tmp: tempfile::TempDir,
    store: Store,
    identity: Identity,
    files: PathBuf,
    inbox: PathBuf,
}

fn home(name: &str) -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("beam"));
    let identity = Identity::generate(name).unwrap();
    store.save_identity(&identity, false).unwrap();
    let files = tmp.path().join("files");
    let inbox = tmp.path().join("downloads");
    std::fs::create_dir_all(&files).unwrap();
    Home {
        _tmp: tmp,
        store,
        identity,
        files,
        inbox,
    }
}

fn pair(a: &Home, a_calls_b: &str, b: &Home, b_calls_a: &str) {
    for (store, name, key) in [
        (&a.store, a_calls_b, b.identity.verifying_key()),
        (&b.store, b_calls_a, a.identity.verifying_key()),
    ] {
        let mut known = store.load_known_peers().unwrap();
        known.add(Peer::new(name, key)).unwrap();
        store.save_known_peers(&known).unwrap();
    }
}

/// A running agent in `home`, and how to stop it.
struct Running_ {
    status: AgentStatus,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), agent::AgentError>>,
}

impl Running_ {
    fn at(&self) -> iroh::EndpointAddr {
        self.status
            .invite
            .as_deref()
            .unwrap()
            .parse::<Invite>()
            .unwrap()
            .endpoint_addr()
    }
    async fn inbox(&self) -> Client {
        let (client, welcome) = Client::connect(self.status.port, &self.status.token)
            .await
            .expect("connect to the agent");
        assert!(matches!(welcome, AgentMsg::Welcome { .. }), "{welcome:?}");
        client
    }
}

async fn start_agent(home: &Home, accept_timeout: Duration) -> Running_ {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let options = AgentOptions {
        network: Network {
            relay: Relay::Disabled,
            bind: Bind::Loopback,
            port: 0,
            advertise: Vec::new(),
        },
        receive_dir: home.inbox.clone(),
        accept_timeout,
        port_mapping: false,
        notify: false,
        echo: false,
    };
    let (identity, store) = (home.identity.clone(), home.store.clone());
    let task = tokio::spawn(async move {
        agent::run(identity, store, options, async {
            let _ = stop_rx.await;
        })
        .await
    });
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Running::Yes(status) = status::read(&home.store)
            && status.invite.is_some()
        {
            return Running_ {
                status,
                stop: Some(stop_tx),
                task,
            };
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the agent never came up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Starts sending `bytes` from `from` to the agent at `at`.
fn send(
    from: &Home,
    at: iroh::EndpointAddr,
    name: &str,
    bytes: &[u8],
) -> tokio::task::JoinHandle<Result<beam::transfer::SendSummary, TransferError>> {
    let path = from.files.join(name);
    std::fs::write(&path, bytes).unwrap();
    let identity = from.identity.clone();
    tokio::spawn(async move {
        let endpoint = endpoint::bind(&identity, &Relay::Disabled, Bind::Loopback, 0, &[])
            .await
            .unwrap();
        let connection = dial_transfer(&endpoint, at).await.expect("dial the agent");
        let mut options = SendOptions::new(
            path,
            beam::identity::encode_public_key(&identity.verifying_key()),
        );
        options.accept_timeout = Duration::from_secs(60);
        let result = send_on(&connection, &mut options, &mut SilentReporter).await;
        endpoint.close().await;
        result
    })
}

/// The next message the inbox gets that `want` matches.
async fn expect(client: &mut Client, what: &str, want: impl Fn(&AgentMsg) -> bool) -> AgentMsg {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        match tokio::time::timeout_at(deadline, client.next()).await {
            Ok(Ok(msg)) if want(&msg) => return msg,
            Ok(Ok(_)) => continue,
            other => panic!("never saw {what}: {other:?}"),
        }
    }
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 13 + 1) % 251) as u8).collect()
}

/// The whole path: a paired device sends, the inbox is shown the request with
/// the usual fields, accepting it there saves the file in the receive folder.
#[tokio::test]
async fn a_request_is_accepted_in_the_inbox_and_saved_to_the_receive_folder() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair(&alice, "bob", &bob, "alice");
    let agent = start_agent(&bob, Duration::from_secs(60)).await;
    let mut inbox = agent.inbox().await;

    let bytes = payload(300_000);
    let sending = send(&alice, agent.at(), "report.bin", &bytes);

    let id = match expect(&mut inbox, "the request", |m| {
        matches!(m, AgentMsg::Request { .. })
    })
    .await
    {
        AgentMsg::Request {
            id,
            request,
            expires_in,
        } => {
            assert_eq!(request.peer_name, "alice", "bob's own name for alice");
            assert_eq!(request.file_name, "report.bin");
            assert_eq!(request.size, bytes.len() as u64);
            assert_eq!(
                request.fingerprint,
                alice.identity.fingerprint().to_string()
            );
            assert!(expires_in > 50 && expires_in <= 60, "{expires_in}");
            id
        }
        _ => unreachable!(),
    };
    inbox
        .send(&ClientMsg::Answer { id, accept: true })
        .await
        .unwrap();

    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent.expect("send").bytes_sent, bytes.len() as u64);
    match expect(&mut inbox, "the result", |m| {
        matches!(m, AgentMsg::Finished { .. })
    })
    .await
    {
        AgentMsg::Finished { ok, text } => assert!(ok, "{text}"),
        _ => unreachable!(),
    }
    assert_eq!(std::fs::read(bob.inbox.join("report.bin")).unwrap(), bytes);
}

#[tokio::test]
async fn declining_in_the_inbox_saves_nothing() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair(&alice, "bob", &bob, "alice");
    let agent = start_agent(&bob, Duration::from_secs(60)).await;
    let mut inbox = agent.inbox().await;

    let sending = send(&alice, agent.at(), "no.bin", &payload(1000));
    let AgentMsg::Request { id, .. } = expect(&mut inbox, "the request", |m| {
        matches!(m, AgentMsg::Request { .. })
    })
    .await
    else {
        unreachable!()
    };
    inbox
        .send(&ClientMsg::Answer { id, accept: false })
        .await
        .unwrap();

    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Declined))),
        "{sent:?}"
    );
    assert!(!bob.inbox.join("no.bin").exists());
}

/// S-5 at the agent: nobody answers, so it is refused when the time is up,
/// and every inbox stops showing it.
#[tokio::test]
async fn an_unanswered_request_expires_and_saves_nothing() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair(&alice, "bob", &bob, "alice");
    let agent = start_agent(&bob, Duration::from_secs(2)).await;
    let mut inbox = agent.inbox().await;

    let sending = send(&alice, agent.at(), "late.bin", &payload(1000));
    let AgentMsg::Request { id, .. } = expect(&mut inbox, "the request", |m| {
        matches!(m, AgentMsg::Request { .. })
    })
    .await
    else {
        unreachable!()
    };
    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Expired))),
        "{sent:?}"
    );
    expect(
        &mut inbox,
        "the request closing",
        |m| matches!(m, AgentMsg::Closed { id: closed } if *closed == id),
    )
    .await;
    // Answering now changes nothing.
    inbox
        .send(&ClientMsg::Answer { id, accept: true })
        .await
        .unwrap();
    expect(&mut inbox, "too late", |m| {
        matches!(m, AgentMsg::TooLate { .. })
    })
    .await;
    assert!(!bob.inbox.join("late.bin").exists());
}

/// Two inboxes open: the first answer decides, the second is told it came
/// too late. One request can never be accepted twice, or by both.
#[tokio::test]
async fn the_first_answer_decides_and_a_second_is_too_late() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair(&alice, "bob", &bob, "alice");
    let agent = start_agent(&bob, Duration::from_secs(60)).await;
    let (mut first, mut second) = (agent.inbox().await, agent.inbox().await);

    let sending = send(&alice, agent.at(), "once.bin", &payload(1000));
    let AgentMsg::Request { id, .. } = expect(&mut first, "the request", |m| {
        matches!(m, AgentMsg::Request { .. })
    })
    .await
    else {
        unreachable!()
    };
    expect(&mut second, "the request", |m| {
        matches!(m, AgentMsg::Request { .. })
    })
    .await;
    first
        .send(&ClientMsg::Answer { id, accept: false })
        .await
        .unwrap();
    expect(&mut second, "the close", |m| {
        matches!(m, AgentMsg::Closed { .. })
    })
    .await;
    second
        .send(&ClientMsg::Answer { id, accept: true })
        .await
        .unwrap();
    expect(&mut second, "too late", |m| {
        matches!(m, AgentMsg::TooLate { .. })
    })
    .await;

    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Declined))),
        "{sent:?}"
    );
}

/// The local port tells a client nothing, and takes nothing from it, until
/// it presents the token: not the pending request, not a file name.
#[tokio::test]
async fn a_client_without_the_token_learns_nothing_and_cannot_answer() {
    let (alice, bob) = (home("alice"), home("bob"));
    pair(&alice, "bob", &bob, "alice");
    let agent = start_agent(&bob, Duration::from_secs(60)).await;
    let mut inbox = agent.inbox().await;
    let sending = send(&alice, agent.at(), "secret-plans.bin", &payload(1000));
    let AgentMsg::Request { id, .. } = expect(&mut inbox, "the request", |m| {
        matches!(m, AgentMsg::Request { .. })
    })
    .await
    else {
        unreachable!()
    };

    for hello in [
        // a wrong token
        format!("{{\"cmd\":\"hello\",\"token\":\"{}\"}}\n", "0".repeat(64)),
        // no token: straight to answering
        format!("{{\"cmd\":\"answer\",\"id\":{id},\"accept\":true}}\n"),
        // not even JSON
        "GET / HTTP/1.1\r\n\r\n".to_string(),
    ] {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", agent.status.port))
            .await
            .unwrap();
        stream.write_all(hello.as_bytes()).await.unwrap();
        // Try to answer as well, in case the first line was let through.
        let _ = stream
            .write_all(format!("{{\"cmd\":\"answer\",\"id\":{id},\"accept\":true}}\n").as_bytes())
            .await;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
            .await
            .expect("the agent left a bad client hanging")
            .unwrap_or(0);
        assert_eq!(
            n, 0,
            "the agent said something to a client without the token: {line}"
        );
    }
    // A client that says nothing at all is dropped too.
    let mut silent = tokio::net::TcpStream::connect(("127.0.0.1", agent.status.port))
        .await
        .unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::AsyncReadExt::read(&mut silent, &mut buf),
    )
    .await
    .expect("a silent client was kept forever")
    .unwrap_or(0);
    assert_eq!(n, 0);

    // The request is still waiting: none of that answered it.
    inbox
        .send(&ClientMsg::Answer { id, accept: false })
        .await
        .unwrap();
    let sent = tokio::time::timeout(PATIENCE, sending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(sent, Err(TransferError::Rejected(RejectReason::Declined))),
        "{sent:?}"
    );
}

/// The agent does not pair: the pairing protocol is not even offered, so a
/// device that finds it cannot try codes (R-8).
#[tokio::test]
async fn the_agent_does_not_pair() {
    let (stranger, bob) = (home("stranger"), home("bob"));
    let agent = start_agent(&bob, Duration::from_secs(60)).await;
    let endpoint = endpoint::bind(&stranger.identity, &Relay::Disabled, Bind::Loopback, 0, &[])
        .await
        .unwrap();
    let attempt = tokio::time::timeout(PATIENCE, async {
        endpoint.connect(agent.at(), PAIR_ALPN).await
    })
    .await
    .expect("the attempt hung");
    assert!(attempt.is_err(), "the agent accepted a pairing connection");
    endpoint.close().await;
}

/// S-7 at the agent: a device that is not paired is refused, and the inbox
/// is never asked.
#[tokio::test]
async fn an_unpaired_device_is_refused_without_a_request() {
    let (stranger, bob) = (home("stranger"), home("bob"));
    let mut known = stranger.store.load_known_peers().unwrap();
    known
        .add(Peer::new("bob", bob.identity.verifying_key()))
        .unwrap();
    stranger.store.save_known_peers(&known).unwrap();
    let agent = start_agent(&bob, Duration::from_secs(60)).await;
    let mut inbox = agent.inbox().await;

    let sent = tokio::time::timeout(PATIENCE, send(&stranger, agent.at(), "x.bin", &payload(10)))
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            sent,
            Err(TransferError::Rejected(RejectReason::UnknownPeer))
        ),
        "{sent:?}"
    );
    let asked = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(AgentMsg::Request { .. }) = inbox.next().await {
                return;
            }
        }
    })
    .await;
    assert!(asked.is_err(), "the inbox was asked about a stranger");
}

/// `beam service stop`: the agent stops cleanly, and no longer counts as
/// running, so its token is gone from disk.
#[tokio::test]
async fn a_stop_request_stops_the_agent_and_takes_its_token_off_disk() {
    let bob = home("bob");
    let mut agent = start_agent(&bob, Duration::from_secs(60)).await;
    let mut inbox = agent.inbox().await;
    inbox.send(&ClientMsg::Stop).await.unwrap();
    let result = tokio::time::timeout(PATIENCE, &mut agent.task)
        .await
        .expect("the agent did not stop")
        .unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(status::read(&bob.store), Running::No);
    assert!(!bob.store.agent_status_path().exists());
    agent.stop.take();
}

/// One agent per beam home.
#[tokio::test]
async fn a_second_agent_in_the_same_home_is_refused() {
    let bob = home("bob");
    let _agent = start_agent(&bob, Duration::from_secs(60)).await;
    let options = AgentOptions {
        network: Network {
            relay: Relay::Disabled,
            bind: Bind::Loopback,
            port: 0,
            advertise: Vec::new(),
        },
        receive_dir: bob.inbox.clone(),
        accept_timeout: Duration::from_secs(60),
        port_mapping: false,
        notify: false,
        echo: false,
    };
    let second = agent::run(
        bob.identity.clone(),
        bob.store.clone(),
        options,
        std::future::ready(()),
    )
    .await;
    assert!(
        matches!(second, Err(agent::AgentError::AlreadyRunning)),
        "{second:?}"
    );
}
