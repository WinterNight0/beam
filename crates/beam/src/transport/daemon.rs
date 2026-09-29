//! The Beam daemon: a persistent Layer-4 TCP listener.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info};

use crate::identity::KnownPeers;
use crate::transfer::{Prompt, ReceiveOptions, Reporter, receive_file};

/// A Beam node listens for transfers regardless of whether it is currently a
/// sender or receiver. The role is chosen per connection.
pub struct Daemon {
    bind: SocketAddr,
}

impl Daemon {
    pub fn new(bind: SocketAddr) -> Self { Self { bind } }

    pub async fn run<Q, R, F>(self, options: ReceiveOptions, prompt: Q, reporter: F) -> anyhow::Result<()>
    where
        Q: Prompt + Clone + Send + Sync + 'static,
        R: Reporter + Send + 'static,
        F: Fn() -> R + Send + Sync + 'static,
    {
        let listener = TcpListener::bind(self.bind).await?;
        let local = listener.local_addr()?;
        info!(%local, "Beam daemon listening");
        println!("Beam daemon listening on {local}");

        let options = Arc::new(options);
        let prompt = Arc::new(prompt);
        let reporter = Arc::new(reporter);
        let known = Arc::new(KnownPeers::with_header());

        loop {
            let (stream, peer) = listener.accept().await?;
            stream.set_nodelay(true)?;
            let options = Arc::clone(&options);
            let prompt = Arc::clone(&prompt);
            let reporter = Arc::clone(&reporter);
            let known = Arc::clone(&known);
            tokio::spawn(async move {
                if let Err(e) = handle(stream, peer, options, prompt, reporter, known).await {
                    error!(%peer, error = %e, "Beam connection failed");
                    eprintln!("beam: connection from {peer} failed: {e}");
                }
            });
        }
    }
}

async fn handle<Q, R, F>(
    stream: TcpStream,
    peer: SocketAddr,
    options: Arc<ReceiveOptions>,
    prompt: Arc<Q>,
    reporter: Arc<F>,
    known: Arc<KnownPeers>,
) -> anyhow::Result<()>
where
    Q: Prompt + Clone + Send + Sync + 'static,
    R: Reporter + Send + 'static,
    F: Fn() -> R + Send + Sync + 'static,
{
    let mut opts = (*options).clone();
    opts.accept_unpaired = true;
    let prompt = (*prompt).clone();
    let mut reporter = reporter();
    let mut seen = std::collections::HashSet::new();

    match receive_file(stream, &known, &opts, prompt, &mut reporter, &mut seen).await {
        Ok(summary) => println!("Received {} from {peer}, saved as {}", crate::ui::format_bytes(summary.bytes), summary.final_name),
        Err(crate::transfer::TransferError::Rejected(reason)) => {
            eprintln!("beam: transfer from {peer} rejected: {}", reason.explain());
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
