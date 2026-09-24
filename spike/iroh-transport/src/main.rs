//! SPIKE-001 prototype: beam's transfer engine over iroh, unmodified.
//!
//! Two processes, two commands:
//!
//! ```text
//! iroh-transport-spike listen  --beam-dir <dir> --out <dir>
//! iroh-transport-spike send    --beam-dir <dir> --to <endpoint-id> <file>
//! ```
//!
//! The point of the prototype is what is *absent*. There is no adapter type, no
//! framing shim, no copy loop bridging two worlds. iroh hands back a QUIC
//! stream pair that already implements `tokio::io::AsyncRead` and
//! `AsyncWrite`, and `beam::transfer::send_file` / `receive_file` are generic
//! over exactly that. `tokio::io::join` is the whole adapter.
//!
//! The identity is not a second identity either: iroh's `SecretKey` is an
//! `ed25519_dalek::SigningKey`, which is the type beam already keeps in
//! `~/.beam/id_ed25519`, so the endpoint id *is* the device's public key.
//!
//! This is throwaway code. It skips things the product would not skip — most
//! visibly, it does not consult `known_peers` before connecting out — and it
//! exists to answer questions, not to be extended.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use beam::identity::{KnownPeers, Store, encode_public_key};
use beam::transfer::{
    Prompt, PromptRequest, ReceiveOptions, SendOptions, SilentReporter, receive_file, send_file,
};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey, TransportAddr};

/// Identifies this protocol during the QUIC handshake.
const ALPN: &[u8] = b"beam/spike/0";

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();

    let mut beam_dir: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut to: Option<String> = None;
    let mut direct: Vec<String> = Vec::new();
    let mut file: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--beam-dir" => beam_dir = args.next().map(PathBuf::from),
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--to" => to = args.next(),
            "--addr" => direct.extend(args.next()),
            other => file = Some(PathBuf::from(other)),
        }
    }

    let store = Store::new(beam_dir.context("--beam-dir is required")?);
    let identity = store.load_identity().context("load identity")?;

    // The same 32 bytes beam already has. No second keypair, no conversion
    // beyond handing over the seed.
    let secret = SecretKey::from_bytes(&identity.signing_key().to_bytes());
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .context("bind iroh endpoint")?;

    match command.as_str() {
        "listen" => listen(&endpoint, &store, out_dir.context("--out is required")?).await,
        "send" => {
            send(
                &endpoint,
                &identity,
                &to.context("--to is required")?,
                &direct,
                &file.context("a file is required")?,
            )
            .await
        }
        other => bail!("unknown command {other:?}; expected `listen` or `send`"),
    }
}

async fn listen(endpoint: &Endpoint, store: &Store, out_dir: PathBuf) -> Result<()> {
    let known_peers = store.load_known_peers().context("load known_peers")?;

    println!("endpoint id : {}", endpoint.id());
    for bound in endpoint.bound_sockets() {
        // The sockets are bound to a wildcard address, which is not something
        // the other side can dial. For the one-machine demo, loopback on the
        // same port is. On two machines the sender uses discovery instead, or
        // the real LAN address.
        if bound.is_ipv4() {
            println!("local addr  : 127.0.0.1:{}", bound.port());
        }
    }
    println!("saving to   : {}", out_dir.display());
    println!(
        "\nGive the sender the endpoint id. On one machine, pass --addr as well so\n\
         the prototype does not need iroh's discovery service. Ctrl+C to stop.\n"
    );

    let mut seen = HashSet::new();

    while let Some(incoming) = endpoint.accept().await {
        // This is all F-11 needs: the progress line's [Direct P2P] / [Relay]
        // tag is a match on one value the transport already knows. It lives on
        // `Incoming`, so it is read before the handshake completes.
        let path = describe_path(incoming.remote_addr());

        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(e) => {
                eprintln!("spike: a connection failed to establish: {e}");
                continue;
            }
        };

        // Worth noticing in the findings: `remote_id` returns a `PublicKey`,
        // not a `Result<PublicKey>`. By the time a connection exists the peer's
        // key has already been proved by the TLS handshake — there is no state
        // in which we have a connection but only a *claimed* identity. That is
        // the whole of S-7a, for free.
        println!("connected: {}", connection.remote_id());
        println!("path     : {path}");

        let (send_half, recv_half) = match connection.accept_bi().await {
            Ok(halves) => halves,
            Err(e) => {
                eprintln!("spike: no stream: {e}");
                continue;
            }
        };

        // The entire adapter.
        let stream = tokio::io::join(recv_half, send_half);

        let options = ReceiveOptions::new(&out_dir, store.tmp_path());
        match receive_file(
            stream,
            &known_peers,
            &options,
            StdinPrompt,
            &mut SilentReporter,
            &mut seen,
        )
        .await
        {
            Ok(summary) => println!(
                "received {} from {}, saved as {}",
                summary.bytes, summary.peer_name, summary.final_name
            ),
            Err(e) => eprintln!("spike: transfer failed: {e}"),
        }
    }
    Ok(())
}

async fn send(
    endpoint: &Endpoint,
    identity: &beam::identity::Identity,
    to: &str,
    direct: &[String],
    file: &PathBuf,
) -> Result<()> {
    let peer: PublicKey = to.parse().context("parse the peer's endpoint id")?;

    // With no address hint, iroh looks the peer up through its discovery
    // service, which needs the internet and needs the other side to have
    // published. Passing addresses explicitly is what beam's own signaling
    // server would do in M4, and is what makes this prototype work offline.
    let mut addr = EndpointAddr::new(peer);
    if !direct.is_empty() {
        let mut parsed = Vec::new();
        for text in direct {
            let socket: std::net::SocketAddr =
                text.parse().with_context(|| format!("parse {text:?}"))?;
            parsed.push(TransportAddr::Ip(socket));
        }
        addr = addr.with_addrs(parsed);
    }
    println!("connecting to {peer}...");

    let connection = endpoint.connect(addr, ALPN).await.context("connect")?;
    let (send_half, recv_half) = connection.open_bi().await.context("open a stream")?;
    let mut stream = tokio::io::join(recv_half, send_half);

    let options = SendOptions::new(file, encode_public_key(&identity.verifying_key()));
    let summary = send_file(&mut stream, &options, &mut SilentReporter)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!(
        "sent {} bytes; the peer saved it as {}",
        summary.bytes_sent,
        summary.final_name.as_deref().unwrap_or("(not reported)")
    );
    Ok(())
}

/// Whether the connection is direct or going through a relay.
fn describe_path(addr: iroh::endpoint::IncomingAddr) -> String {
    match addr {
        iroh::endpoint::IncomingAddr::Ip(addr) => format!("[Direct P2P] {addr}"),
        iroh::endpoint::IncomingAddr::Relay { url, .. } => format!("[Relay] {url}"),
        other => format!("[Other] {other:?}"),
    }
}

/// Asks at the terminal, because even a throwaway prototype should not be the
/// thing that quietly invents an auto-accept.
struct StdinPrompt;

impl Prompt for StdinPrompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        use std::io::Write as _;
        let mut out = std::io::stdout();
        writeln!(out, "\nIncoming file")?;
        writeln!(out, "  From        {}", request.peer_name)?;
        writeln!(out, "  Fingerprint {}", request.fingerprint)?;
        writeln!(out, "  File        {}", request.file_name)?;
        writeln!(out, "  Size        {}", request.size)?;
        if let Some(resume) = &request.resume {
            writeln!(out, "  Already have {} bytes", resume.have_bytes)?;
        }
        write!(out, "Accept? [y/N]: ")?;
        out.flush()?;

        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        Ok(answer == "y" || answer == "yes")
    }
}

/// Silences an unused-import warning when the file is read in isolation.
#[allow(dead_code)]
fn _known_peers_type(_: &KnownPeers) {}
