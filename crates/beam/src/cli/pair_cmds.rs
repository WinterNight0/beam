//! `beam pair --wait` and `beam pair <ID>`.
//!
//! Pairing needs one device to wait and one to join:
//!
//! ```text
//! device B:  beam pair --wait --name alice      shows Short ID + code
//! device A:  beam pair 123456789 --name bob     asks for the code
//! ```
//!
//! In M4 the waiting side is its own command, because `beam listen` still
//! receives transfers over the development TCP transport. In M5 both move onto
//! iroh and the waiting merges into `listen`. See ADR-0028.

use std::time::Duration;

use super::terminal::{Keyboard, TerminalPairConfirm};
use super::{App, CommandError, Io};
use crate::config::Config;
use crate::identity::{Identity, Peer, ShortId};
use crate::pairing::{
    CodeSlot, Event, Network, PairError, Pairing, PairingCode, PairingError, Timeouts, join, wait,
};
use crate::transport::endpoint::Bind;
use crate::ui;

/// How long the joiner has to type the code.
const CODE_ENTRY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How many malformed codes the joiner may type before giving up. A malformed
/// entry never reaches the network, so it is not a guess.
const CODE_ENTRY_TRIES: usize = 3;

impl App {
    pub(super) fn pair(
        &self,
        short_id: Option<&str>,
        name: &str,
        wait_for_peer: bool,
        loopback: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let identity = self.store.load_identity()?;
        let known = self.store.load_known_peers()?;
        let config = Config::load(&self.store.config_path())
            .map_err(|e| CommandError::Message(e.to_string()))?;
        let network = Network {
            rendezvous: config.rendezvous,
            relay: config.relay,
            bind: if loopback { Bind::Loopback } else { Bind::Any },
        };
        let timeouts = Timeouts::default();

        // Parse the Short ID before anything else, so a typo is caught
        // without touching the network.
        let short_id: Option<ShortId> = match short_id {
            Some(text) => Some(
                text.parse()
                    .map_err(|e| CommandError::Message(format!("{text:?}: {e}")))?,
            ),
            None => None,
        };

        let keyboard = Keyboard::start();
        let confirm = TerminalPairConfirm::new(keyboard.clone(), timeouts.decision);
        let runtime = self.runtime()?;
        let pairing = Pairing {
            identity: &identity,
            known: &known,
            name,
            network: &network,
            timeouts,
        };

        let result = runtime.block_on(async {
            match short_id {
                None => {
                    let code = PairingCode::generate()
                        .map_err(|e| PairError::Io(std::io::Error::other(e)))?;
                    let slot = CodeSlot::new(code, timeouts.code_ttl);
                    wait(&pairing, slot, confirm, |event| {
                        show_event(io, &identity, &network, event)
                    })
                    .await
                }
                Some(short_id) => {
                    let keyboard = keyboard.clone();
                    let read_code = || async move {
                        tokio::task::spawn_blocking(move || read_code(&keyboard))
                            .await
                            .map_err(std::io::Error::other)?
                    };
                    writeln!(io.out, "Looking up {}...", short_id.grouped())?;
                    join(&pairing, short_id, read_code, confirm, |event| {
                        show_event(io, &identity, &network, event)
                    })
                    .await
                }
            }
        });

        let key = result.map_err(|e| not_paired(e, wait_for_peer))?;

        // Read the file again: it may have changed while the prompt was up.
        let mut known = self.store.load_known_peers()?;
        let peer = Peer::new(name, key);
        let fingerprint = peer.fingerprint();
        known.add(peer)?;
        self.store.save_known_peers(&known)?;

        writeln!(io.out)?;
        writeln!(io.out, "Paired with {name}.")?;
        ui::field(io.out, "Fingerprint", &fingerprint.to_string())?;
        Ok(())
    }
}

/// Prints one progress event. Errors writing to the terminal are ignored:
/// they must not abort a pairing that is otherwise going fine.
fn show_event(io: &mut Io<'_>, identity: &Identity, network: &Network, event: Event<'_>) {
    let out = &mut *io.out;
    let _ = match event {
        Event::Waiting {
            short_id,
            code,
            expires_in,
        } => (|| {
            writeln!(out, "Waiting for another device to pair with this one.")?;
            writeln!(out)?;
            ui::field(out, "Short ID", &short_id.grouped())?;
            ui::field(out, "Pairing code", &code.grouped())?;
            ui::field(out, "Expires", &format!("in {}", minutes(expires_in)))?;
            ui::field(out, "Fingerprint", &identity.fingerprint().to_string())?;
            ui::field(out, "Relay", &network.relay.to_string())?;
            writeln!(out)?;
            writeln!(out, "On the other device, run")?;
            writeln!(
                out,
                "    beam pair {short_id} --name <a name for this device>"
            )?;
            writeln!(
                out,
                "and type the code when it asks. The code works for one attempt only."
            )?;
            out.flush()
        })(),
        Event::RefreshFailed(e) => {
            writeln!(
                io.err,
                "beam: warning: could not refresh the registration: {e}"
            )
        }
        Event::Attempt { peer } => writeln!(
            out,
            "\nA device is pairing ({}). The code is now used up.",
            peer.short()
        ),
        Event::Found { count: 1 } => writeln!(out, "Found it."),
        Event::Found { count } => writeln!(
            out,
            "Found {count} devices with that Short ID; the code will tell them apart."
        ),
        Event::Connecting { peer } => writeln!(out, "Connecting to {}", peer.short()),
        Event::CandidateFailed { peer, reason } => {
            writeln!(out, "  {} did not work out: {reason}", peer.short())
        }
    };
}

/// Asks for the code, allowing a couple of typos. Malformed input never
/// reaches the network, so re-asking does not give anyone an extra guess.
fn read_code(keyboard: &Keyboard) -> std::io::Result<PairingCode> {
    for _ in 0..CODE_ENTRY_TRIES {
        let answer = keyboard.ask(CODE_ENTRY_TIMEOUT, |out| {
            write!(out, "Pairing code shown on the other device: ")
        })?;
        let Some(answer) = answer else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no pairing code was entered",
            ));
        };
        match PairingCode::parse(&answer) {
            Ok(code) => return Ok(code),
            Err(e) => {
                let mut out = std::io::stdout().lock();
                let _ = std::io::Write::write_fmt(&mut out, format_args!("{e}\n"));
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "no valid pairing code was entered",
    ))
}

/// The error a failed pairing reports, with the one hint that matters.
fn not_paired(error: PairError, waited: bool) -> CommandError {
    let hint = match (&error, waited) {
        (PairError::Pairing(PairingError::Declined | PairingError::DeclinedByPeer), _) => "",
        (PairError::Pairing(_), true) => {
            "\n       The code is used up. Run `beam pair --wait` again for a new one."
        }
        (PairError::Pairing(_), false) => {
            "\n       The other device's code is now used up; it has to run \
             `beam pair --wait` again for a new one."
        }
        _ => "",
    };
    CommandError::Message(format!("not paired: {error}. Nothing was saved.{hint}"))
}

fn minutes(duration: Duration) -> String {
    let minutes = duration.as_secs().div_ceil(60);
    if minutes == 1 {
        "1 minute".to_string()
    } else {
        format!("{minutes} minutes")
    }
}
