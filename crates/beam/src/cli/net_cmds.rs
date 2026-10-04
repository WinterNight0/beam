//! `beam listen` and `beam send` over iroh — the real transport from M5 on.
//!
//! No server is involved. `listen` shows an invite; `send` reaches a peer by
//! its key, through its relay or at the addresses its invite named, as saved
//! in `known_peers` (ADR-0036).
//!
//! The M2 TCP stand-in is still there behind the hidden `--addr` flag, for
//! tests only (see `transfer_cmds.rs` and ADR-0018).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::desk::{DeskPrompt, PromptDesk};
use super::terminal::Keyboard;
use super::terminal::TerminalReporter;
use super::transfer_cmds::{EitherReporter, ReceiveJson, SendJson, reporter_for};
use super::{App, CommandError, Io};
use crate::config::Config;
use crate::identity::{Fingerprint, encode_public_key};
use crate::invite;
use crate::listen_status::Board;
use crate::listener::{ListenEvent, ListenOptions};
use crate::pairing::rotation::NewCodeReason;
use crate::pairing::{Network, Policy, Timeouts};
use crate::transfer::{
    DEFAULT_ACCEPT_TIMEOUT, DEFAULT_MAX_AGE, PartialStore, RejectReason, SendOptions, TransferError,
};
use crate::transport::dial::{DialError, dial_transfer, interrupt, send_on};
use crate::transport::endpoint::{self, Bind};
use crate::{ui, untrusted};

impl App {
    fn network(&self, loopback: bool) -> Result<Network, CommandError> {
        let config = Config::load(&self.store.config_path())
            .map_err(|e| CommandError::Message(e.to_string()))?;
        Ok(Network {
            relay: config.relay,
            bind: if loopback { Bind::Loopback } else { Bind::Any },
            port: config.port,
            advertise: config.advertise,
        })
    }

    pub(super) fn listen(
        &self,
        out_dir: Option<PathBuf>,
        loopback: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let identity = self.store.load_identity()?;
        // Read once up front so a broken file is reported now, not at the
        // first transfer.
        self.store.load_known_peers()?;
        // Two receivers on one identity would split the peers between them.
        if !matches!(
            crate::agent::status::read(&self.store),
            crate::agent::status::Running::No
        ) {
            return Err(CommandError::Message(
                "the background agent is running and already receives files (answer them with \
                 `beam inbox`). To pair a new device, use `beam pair --wait --name <name>`. To \
                 use `beam listen` instead, run `beam service stop` first"
                    .into(),
            ));
        }
        let network = self.network(loopback)?;
        let configured = Config::load(&self.store.config_path())
            .map_err(|e| CommandError::Message(e.to_string()))?
            .receive_dir;
        let out_dir = match (out_dir, configured) {
            (Some(dir), _) => dir,
            (None, Some(dir)) => dir,
            (None, None) => std::env::current_dir().map_err(CommandError::Io)?,
        };
        std::fs::create_dir_all(&out_dir).map_err(CommandError::Io)?;

        // Stale partials are swept here, at the one moment beam is both
        // long-lived and certainly idle. See ADR-0022.
        match PartialStore::new(self.store.tmp_path()).sweep_expired(DEFAULT_MAX_AGE) {
            Ok(removed) if !removed.is_empty() => writeln!(
                io.err,
                "beam: removed {} partial transfer(s) older than 7 days",
                removed.len()
            )?,
            Ok(_) => {}
            Err(e) => writeln!(io.err, "beam: warning: could not tidy old partials: {e}")?,
        }

        // `whoami` shows the current pairing code from here on; `listen`
        // prints it only once (ADR-0037).
        let policy = Policy::default();
        let board = Board::claim(&self.store, policy.code_ttl)?;
        if !board.is_publishing() {
            writeln!(
                io.err,
                "beam: warning: another `beam listen` is already running with this beam home.\n\
                 beam: warning: `beam whoami` shows that one's pairing code, not this one's."
            )?;
        }

        let desk = PromptDesk::terminal(Keyboard::start());
        let prompt = DeskPrompt::new(desk.clone(), DEFAULT_ACCEPT_TIMEOUT);
        let options = ListenOptions {
            out_dir: out_dir.clone(),
            accept_timeout: DEFAULT_ACCEPT_TIMEOUT,
            stall_timeout: crate::transfer::engine::DEFAULT_STALL_TIMEOUT,
            pairing: policy,
            timeouts: Timeouts::default(),
            allow_pairing: true,
            port_mapping: true,
        };
        let json = self.json;
        let screen = Screen {
            json,
            out_dir: out_dir.display().to_string(),
            store: self.store.clone(),
            desk: desk.clone(),
            board: std::sync::Mutex::new(board),
        };

        let runtime = self.runtime()?;
        // Ctrl+C stops `listen` cleanly: a sender mid-transfer is told, what
        // arrived is kept, and `whoami` stops showing the code (ADR-0041).
        let result = runtime.block_on(crate::listener::run_until(
            identity,
            self.store.clone(),
            network,
            options,
            prompt,
            move || {
                if json {
                    reporter_for(true)
                } else {
                    EitherReporter::Terminal(TerminalReporter::with_desk(desk.clone()))
                }
            },
            move |event| screen.show(event),
            interrupted(),
        ));
        runtime.shutdown_timeout(Duration::from_secs(1));
        result.map_err(|e| CommandError::Message(e.to_string()))
    }

    pub(super) fn send(
        &self,
        peer_name: &str,
        file: &Path,
        chunk_size: Option<u32>,
        loopback: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let identity = self.store.load_identity()?;
        let known_peers = self.store.load_known_peers()?;
        let peer = known_peers.lookup(peer_name).ok_or_else(|| {
            CommandError::Peer(crate::identity::PeerError::NotFound(peer_name.to_string()))
        })?;
        let peer_name = peer.name.clone();
        let peer_key = peer.public_key;
        let network = self.network(loopback)?;
        let peer_addr = invite::peer_addr(peer, &network.relay);
        if !file.is_file() {
            return Err(CommandError::Message(format!(
                "{} is not a file",
                file.display()
            )));
        }
        let mut options = SendOptions::new(file, encode_public_key(&identity.verifying_key()));
        // The receiver decides how long its question stays open: 60 s at
        // `beam listen`, five minutes at a background agent, whose user first
        // has to notice a notification (ADR-0042). Each answers "expired"
        // itself, so the sender only needs to outlast the longest.
        options.accept_timeout = crate::agent::AGENT_ACCEPT_TIMEOUT + Duration::from_secs(15);
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
                writeln!(io.out, "Looking for {peer_name}...")?;
                io.out.flush()?;
            }
            let endpoint = endpoint::bind(&identity, &network.relay, network.bind, 0, &[])
                .await
                .map_err(|e| CommandError::Message(e.to_string()))?;
            // The connection, once there is one, so Ctrl+C can tell the peer.
            let live = std::sync::Mutex::new(None);
            let outcome = async {
                let connection = dial_transfer(&endpoint, peer_addr)
                    .await
                    .map_err(|e| unreachable_message(&peer_name, &peer_key, e))?;
                *live.lock().expect("not poisoned") = Some(connection.clone());
                if !self.json {
                    writeln!(io.out, "Sending {} to {peer_name}", file.display())?;
                    io.out.flush()?;
                }
                let mut reporter = reporter_for(self.json);
                let sent = send_on(&connection, &mut options, &mut reporter).await;
                reporter.finish();
                sent.map_err(|e| refused_message(&peer_name, e))
            };
            // Ctrl+C: tell the receiver, so it does not wait for a timeout,
            // and say so here (ADR-0041).
            let (outcome, stopped) = tokio::select! {
                outcome = outcome => (outcome, false),
                () = interrupted() => {
                    let connection = live.lock().expect("not poisoned").take();
                    if let Some(connection) = &connection {
                        interrupt(connection);
                    }
                    (Err(interrupted_message(&peer_name, connection.is_some())), true)
                }
            };
            if stopped && !self.json {
                // End the progress line before the message.
                writeln!(io.out)?;
            }
            endpoint.close().await;
            let summary = outcome?;

            if self.json {
                super::identity_cmds::write_json(
                    io,
                    &SendJson {
                        transfer_id: summary.transfer_id.to_string(),
                        peer: peer_name.clone(),
                        bytes_sent: summary.bytes_sent,
                        saved_as: summary.final_name.clone(),
                    },
                )?;
            } else {
                // The name is the receiver's to choose, so it is shown safely.
                let saved = summary
                    .final_name
                    .as_deref()
                    .map(crate::untrusted::name)
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

/// Completes when the person presses Ctrl+C (ADR-0041). Once it has, a
/// second Ctrl+C ends the process at once, in case stopping cleanly hangs. If
/// Ctrl+C cannot be watched, it never completes, and Ctrl+C does what it
/// always did.
pub(super) async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            std::process::exit(1);
        }
    });
}

/// What `send` says when its own user stopped it.
fn interrupted_message(peer: &str, told: bool) -> CommandError {
    CommandError::Message(if told {
        format!(
            "cancelled: you stopped beam (Ctrl+C). {peer} was told the transfer was cancelled.\n       \
             Anything {peer} already received is kept: send the same file again to resume."
        )
    } else {
        format!("cancelled: you stopped beam (Ctrl+C) before {peer} was reached; nothing was sent.")
    })
}

/// Why `send` could not reach the peer, in words that say what to do.
fn unreachable_message(
    peer: &str,
    key: &ed25519_dalek::VerifyingKey,
    error: DialError,
) -> CommandError {
    let fingerprint = Fingerprint::of(key).short();
    CommandError::Message(match error {
        DialError::Unreachable(why) => format!(
            "{peer} ({fingerprint}) is not reachable ({why}).\n       \
             Either {peer} is not running `beam listen`, {peer} is somewhere beam was not\n       \
             told about, or {peer}'s key has changed because it ran `beam init` again.\n       \
             beam never follows a key change by itself.\n\n       \
             If {peer} is listening, ask for the invite it shows now and run\n           \
             beam pair <{peer}'s invite> --name {peer}\n       \
             That only updates where to find {peer}; it cannot change {peer}'s key.\n\n       \
             WARNING: if {peer} now has a new key, someone could be impersonating {peer}.\n       \
             Check the new fingerprint with {peer} in person before you re-pair:\n           \
             beam remove {peer}\n           \
             beam pair <{peer}'s new invite> --name {peer}"
        ),
        DialError::NoAddress => format!(
            "beam does not know where to find {peer} ({fingerprint}): no relay is set in\n       \
             config.toml and no address was saved for {peer}. Ask for the invite `beam listen`\n       \
             shows on {peer} and run\n           \
             beam pair <{peer}'s invite> --name {peer}"
        ),
        other => other.to_string(),
    })
}

/// A refusal from the peer, said the way a person would say it.
fn refused_message(peer: &str, error: TransferError) -> CommandError {
    match error {
        TransferError::PeerInterrupted => CommandError::Message(format!(
            "{peer} stopped beam on their side (Ctrl+C), so the transfer was cancelled.\n       \
             Anything {peer} already received is kept: send the same file again later to resume."
        )),
        TransferError::Rejected(RejectReason::Busy) => {
            CommandError::Message(format!("{peer} is receiving another file; try again later"))
        }
        TransferError::Rejected(RejectReason::UnknownPeer) => CommandError::Message(format!(
            "{peer} does not recognise this device's key.\n       \
             Either {peer} removed this device, or this device's key changed since pairing\n       \
             because `beam init` was run again here.\n\n       \
             WARNING: a changed key is exactly what an impersonator would present, so {peer}\n       \
             must not simply accept it. To re-pair, compare fingerprints in person:\n           \
             on {peer}:  beam remove <this device>, then beam listen\n           \
             here:     beam remove {peer}, then beam pair <{peer}'s invite> --name {peer}"
        )),
        TransferError::Rejected(RejectReason::Declined) => {
            CommandError::Message(format!("{peer} declined the transfer"))
        }
        TransferError::Rejected(RejectReason::Expired) => CommandError::Message(format!(
            "{peer} did not answer within a minute; the transfer was not accepted"
        )),
        other => other.into(),
    }
}

/// Prints what `listen` hears. Runs on the listener's tasks, so it writes to
/// the process's stdout and stderr directly.
struct Screen {
    json: bool,
    out_dir: String,
    store: crate::identity::Store,
    /// Everything `listen` prints goes through the desk, so a notice never
    /// lands in the middle of an open question without the question being
    /// drawn again (M6 item 5).
    desk: PromptDesk,
    /// Keeps `listen.json` current for `beam whoami` (ADR-0037).
    board: std::sync::Mutex<Board>,
}

impl Screen {
    fn show(&self, event: ListenEvent) {
        // Warnings and information share the one screen the desk manages.
        let mut text = Vec::new();
        let mut warnings = Vec::new();
        if let Err(e) = self.board.lock().expect("not poisoned").on_event(&event) {
            let _ = writeln!(
                warnings,
                "beam: warning: could not update the status `beam whoami` reads: {e}"
            );
        }
        let _ = self.write(event, &mut text, &mut warnings);
        text.extend_from_slice(&warnings);
        let text = String::from_utf8_lossy(&text);
        let text = text.trim_end_matches('\n');
        if !text.is_empty() {
            self.desk.notice(text);
        }
    }

    /// The name a fingerprint is paired under, if any.
    fn who(&self, fingerprint: &Fingerprint) -> String {
        self.store
            .load_known_peers()
            .ok()
            .and_then(|known| {
                known
                    .peers()
                    .into_iter()
                    .find(|p| p.fingerprint() == *fingerprint)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_else(|| fingerprint.short())
    }

    fn write(
        &self,
        event: ListenEvent,
        out: &mut dyn Write,
        err: &mut dyn Write,
    ) -> std::io::Result<()> {
        match event {
            ListenEvent::Ready {
                invite,
                fingerprint,
                code,
                relay,
            } => {
                ui::field(out, "Invite", &invite.to_string())?;
                ui::field(out, "Pairing code", &code.grouped())?;
                ui::field(out, "Fingerprint", &fingerprint.to_string())?;
                ui::field(out, "Relay", &relay.to_string())?;
                ui::field(out, "Saving to", &self.out_dir)?;
                writeln!(out)?;
                writeln!(
                    out,
                    "To pair a new device, send it the invite and run on it:  beam pair <invite> --name <a name for this one>"
                )?;
                writeln!(
                    out,
                    "The pairing code works once and changes every 10 minutes; \
                     `beam whoami` shows the current one."
                )?;
                writeln!(
                    out,
                    "Waiting for transfers. Every one has to be accepted by hand. Ctrl+C to stop."
                )
            }
            ListenEvent::PortTaken { wanted, got } => writeln!(
                err,
                "beam: warning: port {wanted} is in use, so this is listening on port {got}.\n\
                 beam: warning: peers that saved the old address need the new invite, unless the relay reaches them."
            ),
            // A new code is not printed (ADR-0037): it changes every ten
            // minutes, and printing each one buried long transfers. After an
            // attempt, a short line says where to find it.
            ListenEvent::NewCode { reason, .. } => match reason {
                NewCodeReason::Start | NewCodeReason::Expired => Ok(()),
                NewCodeReason::Used => writeln!(
                    out,
                    "The pairing code was used; `beam whoami` shows the new one."
                ),
                NewCodeReason::CooledDown => {
                    writeln!(out, "Pairing is back on; `beam whoami` shows the new code.")
                }
            },
            ListenEvent::PairingPaused { failures, wait } => writeln!(
                out,
                "Pairing paused for {} s after a failed attempt ({failures} of 3).",
                wait.as_secs().max(1)
            ),
            ListenEvent::PairingDisabled { failures } => {
                writeln!(out)?;
                writeln!(
                    out,
                    "Pairing is OFF for the rest of this session: {failures} attempts in a row did not know the code."
                )?;
                writeln!(
                    out,
                    "Someone may be guessing. Paired devices can still send you files."
                )?;
                writeln!(out, "Restart `beam listen` to pair again.")
            }
            ListenEvent::PairingAttempt { peer } => writeln!(
                out,
                "\nA device is pairing ({}). The code is now used up.",
                peer.short()
            ),
            ListenEvent::PairingRefused { peer, reason } => writeln!(
                out,
                "Turned away a pairing attempt from {}: {reason}.",
                peer.short()
            ),
            ListenEvent::Paired { name, fingerprint } => {
                let name = untrusted::name(&name);
                writeln!(out, "Paired with {name} ({fingerprint}).")?;
                writeln!(
                    out,
                    "It is saved as {name:?}; `beam rename {name} <new name>` changes that."
                )
            }
            ListenEvent::PairingFailed { peer, error } => writeln!(
                out,
                "Not paired with {}: {}. Nothing was saved.",
                peer.short(),
                untrusted::text(&error)
            ),
            ListenEvent::TransferTurnedAway { peer } => writeln!(
                out,
                "Turned away a file from {}: already receiving one. They were told to try later.",
                self.who(&peer)
            ),
            ListenEvent::Received(summary) => {
                if self.json {
                    let text = serde_json::to_string_pretty(&ReceiveJson {
                        transfer_id: summary.transfer_id.to_string(),
                        peer: summary.peer_name.clone(),
                        fingerprint: summary.fingerprint.clone(),
                        saved_as: summary.final_name.clone(),
                        bytes: summary.bytes,
                    })
                    .map_err(std::io::Error::other)?;
                    return writeln!(out, "{text}");
                }
                let how = if summary.resumed {
                    format!(
                        " (resumed; {} received this time)",
                        ui::format_bytes(summary.received_now)
                    )
                } else {
                    String::new()
                };
                writeln!(
                    out,
                    "Received {} from {} ({}), saved as {}{}",
                    ui::format_bytes(summary.bytes),
                    untrusted::name(&summary.peer_name),
                    summary.fingerprint,
                    untrusted::name(&summary.final_name),
                    how
                )
            }
            ListenEvent::TransferFailed { peer, error } => writeln!(
                err,
                "beam: transfer from {} failed: {}",
                self.who(&peer),
                untrusted::text(&error)
            ),
            ListenEvent::TransferInterrupted { peer } => {
                let who = self.who(&peer);
                writeln!(
                    err,
                    "beam: {who} stopped beam on their side (Ctrl+C), so the transfer from {who} \
                     was cancelled.\nbeam: What arrived is kept; it resumes if {who} sends the \
                     file again."
                )
            }
            ListenEvent::Stopped { cancelled } => {
                if cancelled.is_empty() {
                    return writeln!(out, "Stopped listening (Ctrl+C).");
                }
                let names: Vec<String> = cancelled.iter().map(|p| self.who(p)).collect();
                let names = names.join(", ");
                writeln!(
                    out,
                    "Stopped listening (Ctrl+C). Cancelled the transfer from {names}, and told \
                     {names}.\nWhat arrived is kept; it resumes if {names} sends the file again."
                )
            }
        }
    }
}
