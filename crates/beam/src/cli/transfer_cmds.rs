//! `beam listen` and `beam send`.
//!
//! Both use a plain TCP address given on the command line. That is a stand-in
//! for peer discovery, which arrives in M4, and for WebRTC, which arrives in
//! M5; see ADR-0018.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};

use super::terminal::{TerminalPrompt, TerminalReporter};
use super::{App, CommandError, Io};
use crate::identity::{Identity, KnownPeers, encode_public_key};
use crate::transfer::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_MAX_AGE, PartialError, ReceiveOptions, Reporter, SendOptions,
    SilentReporter, TransferError, TransferId, receive_file, send_file,
};
use crate::transport::is_loopback;
use crate::ui;

/// The `--json` shape of a finished send.
#[derive(Serialize)]
struct SendJson {
    transfer_id: String,
    peer: String,
    bytes_sent: u64,
    saved_as: Option<String>,
}

/// The `--json` shape of one partial transfer.
#[derive(Serialize)]
struct PartialJson {
    id: String,
    peer_fingerprint: String,
    file_name: String,
    size: u64,
    have_bytes: u64,
    percent: u8,
    expired: bool,
}

/// The `--json` shape of a finished receive.
#[derive(Serialize)]
struct ReceiveJson {
    transfer_id: String,
    peer: String,
    fingerprint: String,
    saved_as: String,
    bytes: u64,
}

impl App {
    /// Builds the runtime the two networked commands run on.
    ///
    /// The rest of the CLI stays synchronous; only these two need a runtime,
    /// and building it here keeps `cli::execute` and the M1 commands free of
    /// async plumbing.
    fn runtime(&self) -> Result<tokio::runtime::Runtime, CommandError> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(CommandError::Io)
    }

    fn identity_and_peers(&self) -> Result<(Identity, KnownPeers), CommandError> {
        let identity = self.store.load_identity()?;
        let known_peers = self.store.load_known_peers()?;
        Ok((identity, known_peers))
    }

    pub(super) fn listen(
        &self,
        addr: SocketAddr,
        out_dir: Option<PathBuf>,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let (identity, known_peers) = self.identity_and_peers()?;
        let out_dir = match out_dir {
            Some(dir) => dir,
            None => std::env::current_dir().map_err(CommandError::Io)?,
        };
        std::fs::create_dir_all(&out_dir).map_err(CommandError::Io)?;

        if !is_loopback(&addr) {
            writeln!(io.err)?;
            writeln!(
                io.err,
                "beam: warning: listening on {addr}, which is reachable from other machines."
            )?;
            writeln!(
                io.err,
                "beam: warning: in M2 the connection is NOT encrypted and the sender's identity"
            )?;
            writeln!(
                io.err,
                "beam: warning: is only claimed, not proven. Anyone who can reach this port and"
            )?;
            writeln!(
                io.err,
                "beam: warning: knows a paired peer's public key can impersonate it, and anyone"
            )?;
            writeln!(
                io.err,
                "beam: warning: on the path can read the file. Encryption arrives in M5 and"
            )?;
            writeln!(
                io.err,
                "beam: warning: proven identity in M6. Use 127.0.0.1 until then."
            )?;
            writeln!(io.err)?;
        }

        let mut options = ReceiveOptions::new(&out_dir, self.store.tmp_path());
        options.accept_timeout = DEFAULT_ACCEPT_TIMEOUT;

        // Stale partials are swept here, at the one moment beam is both
        // long-lived and certainly idle. See ADR-0022.
        match options.partials().sweep_expired(DEFAULT_MAX_AGE) {
            Ok(removed) if !removed.is_empty() => {
                writeln!(
                    io.err,
                    "beam: removed {} partial transfer(s) older than 7 days",
                    removed.len()
                )?;
            }
            Ok(_) => {}
            Err(e) => writeln!(io.err, "beam: warning: could not tidy old partials: {e}")?,
        }

        let runtime = self.runtime()?;
        let result = runtime.block_on(async {
            let listener = TcpListener::bind(addr).await.map_err(CommandError::Io)?;
            let bound = listener.local_addr().map_err(CommandError::Io)?;

            ui::field(io.out, "Short ID", &identity.short_id().grouped())?;
            ui::field(io.out, "Fingerprint", &identity.fingerprint().to_string())?;
            ui::field(io.out, "Listening", &bound.to_string())?;
            ui::field(io.out, "Saving to", &out_dir.display().to_string())?;
            writeln!(io.out)?;
            writeln!(
                io.out,
                "Waiting for transfers. Every one has to be accepted by hand. Ctrl+C to stop."
            )?;
            io.out.flush()?;

            // Within one run, a transfer id is never honoured twice (S-11).
            // M3 persists this across restarts.
            let mut seen: HashSet<TransferId> = HashSet::new();

            loop {
                let (stream, peer_addr) = listener.accept().await.map_err(CommandError::Io)?;
                let mut reporter = reporter_for(self.json);

                let outcome = receive_file(
                    stream,
                    &known_peers,
                    &options,
                    TerminalPrompt::new(options.accept_timeout),
                    &mut reporter,
                    &mut seen,
                )
                .await;
                reporter.finish();

                match outcome {
                    Ok(summary) => {
                        if self.json {
                            super::identity_cmds::write_json(
                                io,
                                &ReceiveJson {
                                    transfer_id: summary.transfer_id.to_string(),
                                    peer: summary.peer_name.clone(),
                                    fingerprint: summary.fingerprint.clone(),
                                    saved_as: summary.final_name.clone(),
                                    bytes: summary.bytes,
                                },
                            )?;
                        } else {
                            let how = if summary.resumed {
                                format!(
                                    " (resumed; {} received this time)",
                                    ui::format_bytes(summary.received_now)
                                )
                            } else {
                                String::new()
                            };
                            writeln!(
                                io.out,
                                "Received {} from {} ({}), saved as {}{}",
                                ui::format_bytes(summary.bytes),
                                summary.peer_name,
                                summary.fingerprint,
                                summary.final_name,
                                how
                            )?;
                        }
                    }
                    Err(e) => {
                        writeln!(io.err, "beam: transfer from {peer_addr} failed: {e}")?;
                    }
                }
                io.out.flush()?;
                io.err.flush()?;
            }
        });

        runtime.shutdown_timeout(Duration::from_secs(1));
        result
    }

    pub(super) fn send(
        &self,
        peer_name: &str,
        file: &Path,
        addr: SocketAddr,
        chunk_size: Option<u32>,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let (identity, known_peers) = self.identity_and_peers()?;

        // The name has to be one this machine has paired with. In M2 that is
        // all it buys: nothing yet proves the machine at `addr` is that peer.
        // See ADR-0019.
        let peer = known_peers.lookup(peer_name).ok_or_else(|| {
            CommandError::Peer(crate::identity::PeerError::NotFound(peer_name.to_string()))
        })?;

        if !file.is_file() {
            return Err(CommandError::Message(format!(
                "{} is not a file",
                file.display()
            )));
        }

        let mut options = SendOptions::new(file, encode_public_key(&identity.verifying_key()));
        options.accept_timeout = DEFAULT_ACCEPT_TIMEOUT;
        if let Some(chunk_size) = chunk_size {
            if chunk_size == 0 {
                return Err(CommandError::Message(
                    "--chunk-size must be at least 1".to_string(),
                ));
            }
            options.chunk_size = chunk_size;
        }

        let runtime = self.runtime()?;
        let result = runtime.block_on(async {
            if !self.json {
                writeln!(
                    io.out,
                    "Sending {} to {peer_name} at {addr}",
                    file.display()
                )?;
                io.out.flush()?;
            }

            let mut stream = TcpStream::connect(addr).await.map_err(CommandError::Io)?;
            let mut reporter = reporter_for(self.json);
            let outcome = send_file(&mut stream, &options, &mut reporter).await;
            reporter.finish();

            let summary = outcome?;
            if self.json {
                super::identity_cmds::write_json(
                    io,
                    &SendJson {
                        transfer_id: summary.transfer_id.to_string(),
                        peer: peer.name.clone(),
                        bytes_sent: summary.bytes_sent,
                        saved_as: summary.final_name.clone(),
                    },
                )?;
            } else {
                let saved = summary
                    .final_name
                    .clone()
                    .unwrap_or_else(|| "the peer did not say".to_string());
                let skipped = if summary.bytes_skipped > 0 {
                    format!(
                        " ({} was already there)",
                        ui::format_bytes(summary.bytes_skipped)
                    )
                } else {
                    String::new()
                };
                writeln!(
                    io.out,
                    "Sent {} to {peer_name}, saved on their side as {saved}{skipped}",
                    ui::format_bytes(summary.bytes_sent)
                )?;
            }
            io.out.flush()?;
            Ok::<(), CommandError>(())
        });

        runtime.shutdown_timeout(Duration::from_secs(1));
        result
    }
}

impl App {
    pub(super) fn transfers(
        &self,
        clear: bool,
        id: Option<&str>,
        assume_yes: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let partials = crate::transfer::PartialStore::new(self.store.tmp_path());
        let summaries = partials
            .list(DEFAULT_MAX_AGE)
            .map_err(|e| CommandError::Partial(Box::new(e)))?;

        if clear {
            return self.clear_transfers(&partials, &summaries, id, assume_yes, io);
        }

        if self.json {
            let list: Vec<PartialJson> = summaries
                .iter()
                .map(|p| PartialJson {
                    id: p.id.clone(),
                    peer_fingerprint: p.peer_fingerprint.clone(),
                    file_name: p.file_name.clone(),
                    size: p.size,
                    have_bytes: p.have_bytes,
                    percent: p.percent(),
                    expired: p.expired,
                })
                .collect();
            return super::identity_cmds::write_json(io, &list);
        }

        if summaries.is_empty() {
            writeln!(io.out, "No partially received transfers.")?;
            return Ok(());
        }

        let rows: Vec<Vec<String>> = summaries
            .iter()
            .map(|p| {
                let age = match (p.expired, p.age()) {
                    (true, _) => "expired".to_string(),
                    (false, Some(age)) => ui::format_age(age),
                    (false, None) => "-".to_string(),
                };
                vec![
                    p.id[..8.min(p.id.len())].to_string(),
                    p.file_name.clone(),
                    ui::format_bytes(p.size),
                    format!("{}%", p.percent()),
                    age,
                ]
            })
            .collect();
        ui::table(io.out, &["ID", "FILE", "SIZE", "HAVE", "UPDATED"], &rows)?;
        writeln!(io.out)?;
        writeln!(
            io.out,
            "Sending the same file again continues from where it stopped."
        )?;
        Ok(())
    }

    fn clear_transfers(
        &self,
        partials: &crate::transfer::PartialStore,
        summaries: &[crate::transfer::PartialSummary],
        id: Option<&str>,
        assume_yes: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        // An id may be given in the abbreviated form `beam transfers` prints.
        let targets: Vec<&crate::transfer::PartialSummary> = match id {
            Some(prefix) => {
                let matched: Vec<_> = summaries
                    .iter()
                    .filter(|p| p.id.starts_with(prefix))
                    .collect();
                if matched.is_empty() {
                    return Err(CommandError::Message(format!(
                        "no partial transfer starts with {prefix:?}"
                    )));
                }
                if matched.len() > 1 {
                    return Err(CommandError::Message(format!(
                        "{prefix:?} matches {} partial transfers; use more characters",
                        matched.len()
                    )));
                }
                matched
            }
            None => summaries.iter().collect(),
        };

        if targets.is_empty() {
            writeln!(io.out, "No partially received transfers.")?;
            return Ok(());
        }

        if !assume_yes {
            for target in &targets {
                ui::field(
                    io.out,
                    "Discarding",
                    &format!(
                        "{} ({} of {})",
                        target.file_name,
                        ui::format_bytes(target.have_bytes),
                        ui::format_bytes(target.size)
                    ),
                )?;
            }
            let question = format!(
                "Delete {} partial transfer(s)? The bytes already received are lost.",
                targets.len()
            );
            if !ui::confirm(io.input, io.out, &question)? {
                writeln!(io.out, "Cancelled.")?;
                return Ok(());
            }
        }

        let mut removed = 0;
        for target in targets {
            match partials.remove(&target.id) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(PartialError::Busy) => writeln!(
                    io.err,
                    "beam: {} is in use by another session; left alone",
                    target.file_name
                )?,
                Err(e) => return Err(CommandError::Partial(Box::new(e))),
            }
        }
        writeln!(io.out, "Deleted {removed} partial transfer(s).")?;
        Ok(())
    }
}

/// Progress goes to the terminal unless the caller asked for JSON, in which
/// case a progress bar would corrupt the output.
enum EitherReporter {
    Terminal(TerminalReporter),
    Silent(SilentReporter),
}

fn reporter_for(json: bool) -> EitherReporter {
    if json {
        EitherReporter::Silent(SilentReporter)
    } else {
        EitherReporter::Terminal(TerminalReporter::new())
    }
}

impl EitherReporter {
    fn finish(&mut self) {
        if let EitherReporter::Terminal(reporter) = self {
            reporter.finish();
        }
    }
}

impl Reporter for EitherReporter {
    fn report(&mut self, progress: crate::transfer::Progress) {
        match self {
            EitherReporter::Terminal(reporter) => reporter.report(progress),
            EitherReporter::Silent(reporter) => reporter.report(progress),
        }
    }
}

impl From<TransferError> for CommandError {
    fn from(error: TransferError) -> Self {
        CommandError::Transfer(Box::new(error))
    }
}
