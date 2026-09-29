//! Direct TCP commands for Beam Stage 1.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::desk::{DeskPrompt, PromptDesk};
use super::terminal::{Keyboard, TerminalReporter};
use super::{App, CommandError, Io};
use crate::identity::{Identity, encode_public_key};
use crate::transfer::{DEFAULT_ACCEPT_TIMEOUT, Reporter, SendOptions, SilentReporter, TransferError, send_file};
use crate::transport::{client, fixed_route, PathKind};
use crate::ui;

#[derive(Serialize)]
pub(super) struct SendJson {
    pub(super) transfer_id: String,
    pub(super) peer: String,
    pub(super) bytes_sent: u64,
    pub(super) saved_as: Option<String>,
}

impl App {
    pub(super) fn runtime(&self) -> Result<tokio::runtime::Runtime, CommandError> {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(CommandError::Io)
    }

    pub(super) fn listen(
        &self,
        out_dir: Option<PathBuf>,
        port: u16,
        loopback: bool,
        _io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let out_dir = out_dir.unwrap_or_else(|| PathBuf::from("."));
        let bind = SocketAddr::new(
            if loopback { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { IpAddr::V4(Ipv4Addr::UNSPECIFIED) },
            port,
        );
        let desk = PromptDesk::terminal(Keyboard::start());
        let prompt = DeskPrompt::new(desk, DEFAULT_ACCEPT_TIMEOUT);
        let store = self.store.clone();
        let json = self.json;
        let runtime = self.runtime()?;
        let result = runtime.block_on(async move {
            if !json {
                println!("Beam daemon starting on {bind}");
                println!("Every Beam node can be a sender or receiver.");
                println!("Waiting for direct TCP connections...");
            }
            crate::listener::run::<_, TerminalReporter, _>(
                store,
                crate::listener::ListenOptions { bind, out_dir, accept_timeout: DEFAULT_ACCEPT_TIMEOUT },
                prompt,
                || TerminalReporter::new(),
            ).await.map_err(|e| CommandError::Message(e.to_string()))
        });
        runtime.shutdown_timeout(Duration::from_secs(1));
        result
    }

    pub(super) fn send(
        &self,
        peer: &str,
        file: &Path,
        chunk_size: Option<u32>,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        if !file.is_file() {
            return Err(CommandError::Message(format!("{} is not a file", file.display())));
        }
        let addr = parse_target(peer)?;
        let identity = load_or_create_identity(&self.store)?;
        let mut options = SendOptions::new(file, encode_public_key(&identity.verifying_key()));
        options.route = fixed_route(PathKind::Direct);
        if let Some(size) = chunk_size {
            if size == 0 { return Err(CommandError::Message("--chunk-size must be at least 1".into())); }
            options.chunk_size = size;
        }

        let runtime = self.runtime()?;
        let json = self.json;
        let peer_label = peer.to_string();
        let file_label = file.display().to_string();
        let result = runtime.block_on(async move {
            if !json { writeln!(io.out, "Sending {file_label} directly to {peer_label} ({addr})...")?; }
            let mut stream = client::connect(addr).await.map_err(|e| CommandError::Message(e.to_string()))?;
            let mut reporter = reporter_for(json);
            let outcome = send_file(&mut stream, &options, &mut reporter).await;
            reporter.finish();
            let summary = outcome?;
            if json {
                super::identity_cmds::write_json(io, &SendJson {
                    transfer_id: summary.transfer_id.to_string(), peer: peer_label,
                    bytes_sent: summary.bytes_sent, saved_as: summary.final_name.clone(),
                })?;
            } else {
                writeln!(io.out, "Sent {} to {peer_label}.", ui::format_bytes(summary.bytes_sent))?;
            }
            Ok::<(), CommandError>(())
        });
        runtime.shutdown_timeout(Duration::from_secs(1));
        result
    }
}

fn parse_target(text: &str) -> Result<SocketAddr, CommandError> {
    if let Ok(addr) = text.parse::<SocketAddr>() { return Ok(addr); }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, crate::config::DEFAULT_PORT));
    }
    if let Ok(mut addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(text, crate::config::DEFAULT_PORT)) {
        if let Some(addr) = addrs.next() { return Ok(addr); }
    }
    Err(CommandError::Message(format!("invalid Beam peer address: {text:?}")))
}

fn load_or_create_identity(store: &crate::identity::Store) -> Result<Identity, CommandError> {
    if store.has_identity() { return Ok(store.load_identity()?); }
    let identity = Identity::generate(&hostname()).map_err(|e| CommandError::Message(format!("could not create Beam identity: {e}")))?;
    store.save_identity(&identity, false)?;
    Ok(identity)
}

fn hostname() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(name) = std::env::var(key) && !name.trim().is_empty() { return name; }
    }
    String::new()
}

enum EitherReporter { Terminal(TerminalReporter), Silent(SilentReporter) }
fn reporter_for(json: bool) -> EitherReporter {
    if json { EitherReporter::Silent(SilentReporter) } else { EitherReporter::Terminal(TerminalReporter::new()) }
}
impl EitherReporter { fn finish(&mut self) { if let Self::Terminal(r) = self { r.finish(); } } }
impl Reporter for EitherReporter {
    fn report(&mut self, progress: crate::transfer::Progress) {
        match self { Self::Terminal(r) => r.report(progress), Self::Silent(r) => r.report(progress) }
    }
}
impl From<TransferError> for CommandError { fn from(error: TransferError) -> Self { CommandError::Transfer(Box::new(error)) } }
