//! `beam pair --wait` and `beam pair <INVITE>`.
//!
//! Pairing needs one device to wait and one to join:
//!
//! ```text
//! device B:  beam listen                          shows an invite + code
//!       (or  beam pair --wait --name alice)
//! device A:  beam pair beam1… --name bob          asks for the code
//! ```
//!
//! The invite carries everything A needs to reach B — B's key, relay and
//! direct addresses — so no server is involved (ADR-0036). After pairing, A
//! saves the relay and addresses next to B's key, which is how `beam send`
//! finds B again.
//!
//! Given the invite of a device that is already paired, `beam pair` only
//! updates where to find it. That never changes a stored key: a device with a
//! new key is a new pairing (rule 3).

use std::time::Duration;

use super::desk::{DeskPrompt, PromptDesk};
use super::terminal::Keyboard;
use super::{App, CommandError, Io};
use crate::config::Config;
use crate::identity::{Identity, Peer};
use crate::invite::{self, Invite};
use crate::pairing::{
    CodeSlot, Event, Network, PairError, Pairing, PairingCode, PairingError, Timeouts, join, wait,
};
use crate::transport::endpoint::Bind;
use crate::{ui, untrusted};

/// How long the joiner has to type the code.
const CODE_ENTRY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How many malformed codes the joiner may type before giving up. A malformed
/// entry never reaches the network, so it is not a guess.
const CODE_ENTRY_TRIES: usize = 3;

impl App {
    pub(super) fn pair(
        &self,
        invite: Option<&str>,
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
            relay: config.relay,
            bind: if loopback { Bind::Loopback } else { Bind::Any },
            port: config.port,
        };
        let timeouts = Timeouts::default();

        // Read the invite before anything else, so a damaged one is caught
        // without touching the network.
        let invite: Option<Invite> = match invite {
            Some(text) => Some(
                text.parse()
                    .map_err(|e: invite::InviteError| CommandError::Message(e.to_string()))?,
            ),
            None => None,
        };

        // An invite from a device that is already paired: only where to find
        // it changes. No code, no network, and never the key.
        if let Some(invite) = &invite
            && known.lookup_key(&invite.key).is_some()
        {
            return self.update_location(invite, name, &network, io);
        }

        let keyboard = Keyboard::start();
        let confirm = DeskPrompt::new(PromptDesk::terminal(keyboard.clone()), timeouts.decision);
        let runtime = self.runtime()?;
        let pairing = Pairing {
            identity: &identity,
            known: &known,
            name: Some(name),
            network: &network,
            timeouts,
        };

        let result = runtime.block_on(async {
            match &invite {
                None => {
                    let code = PairingCode::generate()
                        .map_err(|e| PairError::Io(std::io::Error::other(e)))?;
                    let slot = CodeSlot::new(code, timeouts.code_ttl);
                    wait(&pairing, slot, confirm, |event| {
                        show_event(io, &identity, &network, event)
                    })
                    .await
                }
                Some(invite) => {
                    let keyboard = keyboard.clone();
                    let read_code = || async move {
                        tokio::task::spawn_blocking(move || read_code(&keyboard))
                            .await
                            .map_err(std::io::Error::other)?
                    };
                    join(&pairing, invite, read_code, confirm, |event| {
                        show_event(io, &identity, &network, event)
                    })
                    .await
                }
            }
        });

        let key = result.map_err(|e| not_paired(e, wait_for_peer))?.key;

        // Read the file again: it may have changed while the prompt was up.
        let mut known = self.store.load_known_peers()?;
        let mut peer = Peer::new(name, key);
        if let Some(invite) = &invite {
            invite::remember(&mut peer, invite, &network.relay);
        }
        let fingerprint = peer.fingerprint();
        known.add(peer)?;
        self.store.save_known_peers(&known)?;

        writeln!(io.out)?;
        writeln!(io.out, "Paired with {name}.")?;
        ui::field(io.out, "Fingerprint", &fingerprint.to_string())?;
        Ok(())
    }

    /// Saves where an already-paired device can be found now.
    fn update_location(
        &self,
        invite: &Invite,
        asked_name: &str,
        network: &Network,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let mut known = self.store.load_known_peers()?;
        let peer = known
            .lookup_key_mut(&invite.key)
            .expect("checked by the caller");
        invite::remember(peer, invite, &network.relay);
        let name = untrusted::name(&peer.name);
        let fingerprint = peer.fingerprint();
        self.store.save_known_peers(&known)?;

        writeln!(
            io.out,
            "{name} is already paired. Updated where to find it; its key is unchanged."
        )?;
        ui::field(io.out, "Fingerprint", &fingerprint.to_string())?;
        if !name.eq_ignore_ascii_case(asked_name) {
            writeln!(
                io.out,
                "It is still called {name}; `beam rename {name} {asked_name}` changes that."
            )?;
        }
        Ok(())
    }
}

/// Prints one progress event. Errors writing to the terminal are ignored:
/// they must not abort a pairing that is otherwise going fine.
fn show_event(io: &mut Io<'_>, identity: &Identity, network: &Network, event: Event<'_>) {
    let out = &mut *io.out;
    let _ = match event {
        Event::Waiting {
            invite,
            code,
            expires_in,
        } => (|| {
            writeln!(out, "Waiting for another device to pair with this one.")?;
            writeln!(out)?;
            ui::field(out, "Invite", &invite.to_string())?;
            ui::field(out, "Pairing code", &code.grouped())?;
            ui::field(out, "Expires", &format!("in {}", minutes(expires_in)))?;
            ui::field(out, "Fingerprint", &identity.fingerprint().to_string())?;
            ui::field(out, "Relay", &network.relay.to_string())?;
            writeln!(out)?;
            writeln!(
                out,
                "Send the invite to the other person. On their device, run"
            )?;
            writeln!(
                out,
                "    beam pair <the invite> --name <a name for this device>"
            )?;
            writeln!(
                out,
                "and type the code when it asks. The code works for one attempt only."
            )?;
            out.flush()
        })(),
        Event::Attempt { peer } => writeln!(
            out,
            "\nA device is pairing ({}). The code is now used up.",
            peer.short()
        ),
        Event::Connecting { peer } => writeln!(out, "Connecting to {}", peer.short()),
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
            "\n       The other device's code is now used up; it shows a new one \
             (or, with `beam pair --wait`, has to be run again)."
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
