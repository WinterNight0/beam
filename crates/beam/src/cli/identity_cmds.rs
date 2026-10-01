//! The identity commands: `init`, `whoami`, `peers`, `rename`, `remove`.

use serde::Serialize;

use super::{App, CommandError, Io};
use crate::identity::{Identity, KEY_TYPE, Peer, PeerError, StoreError, encode_public_key};
use crate::listen_status::{self, ListenStatus, Listening, PairingStatus};
use crate::pairing::PairingCode;
use crate::ui;

/// The `--json` shape of `beam whoami` and `beam init`.
#[derive(Serialize)]
struct SelfJson {
    short_id: String,
    fingerprint: String,
    key_type: &'static str,
    public_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    comment: String,
    dir: String,
    /// What a running `beam listen` offers, or `null` (ADR-0037). Only
    /// `whoami` fills it in.
    #[serde(skip_serializing_if = "Option::is_none")]
    listening: Option<ListeningJson>,
}

/// The `--json` shape of a running `listen`, as `whoami` sees it.
#[derive(Serialize)]
#[serde(untagged)]
enum ListeningJson {
    Status(ListenStatus),
    Unreadable { unreadable: bool },
    No(()),
}

/// The `--json` shape of one entry of `beam peers`.
#[derive(Serialize)]
struct PeerJson {
    name: String,
    short_id: String,
    fingerprint: String,
    key_type: &'static str,
    public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    added: Option<String>,
}

impl App {
    fn self_json(&self, identity: &Identity) -> SelfJson {
        SelfJson {
            short_id: identity.short_id().to_string(),
            fingerprint: identity.fingerprint().to_string(),
            key_type: KEY_TYPE,
            public_key: encode_public_key(&identity.verifying_key()),
            comment: identity.comment().to_string(),
            dir: self.store.dir().display().to_string(),
            listening: None,
        }
    }

    pub(super) fn init(&self, force: bool, io: &mut Io<'_>) -> Result<(), CommandError> {
        if self.store.has_identity() && !force {
            return Err(CommandError::Message(format!(
                "an identity already exists on this device at {}\n       \
                 --force generates a new key, and every peer that paired with you \
                 would have to pair again",
                self.store.private_key_path().display()
            )));
        }

        let hostname = hostname();
        let identity = Identity::generate(&hostname).map_err(|e| StoreError::Key {
            path: self.store.private_key_path(),
            source: e,
        })?;
        self.store.save_identity(&identity, force)?;

        if self.json {
            return write_json(io, &self.self_json(&identity));
        }
        writeln!(io.out, "Identity created.")?;
        writeln!(io.out)?;
        ui::field(io.out, "Short ID", &identity.short_id().grouped())?;
        ui::field(io.out, "Fingerprint", &identity.fingerprint().to_string())?;
        ui::field(io.out, "Directory", &self.store.dir().display().to_string())?;
        writeln!(io.out)?;
        writeln!(
            io.out,
            "To pair with another device, run `beam listen` and give it the invite shown."
        )?;
        self.warn_permissions(io);
        Ok(())
    }

    pub(super) fn whoami(&self, io: &mut Io<'_>) -> Result<(), CommandError> {
        let identity = self.store.load_identity()?;
        let listening = listen_status::read(&self.store);

        if self.json {
            let mut json = self.self_json(&identity);
            json.listening = Some(match listening {
                Listening::Yes(status) => ListeningJson::Status(status),
                Listening::Unreadable => ListeningJson::Unreadable { unreadable: true },
                Listening::No => ListeningJson::No(()),
            });
            return write_json(io, &json);
        }
        ui::field(io.out, "Short ID", &identity.short_id().grouped())?;
        ui::field(io.out, "Fingerprint", &identity.fingerprint().to_string())?;
        ui::field(
            io.out,
            "Public key",
            &format!(
                "{KEY_TYPE} {}",
                encode_public_key(&identity.verifying_key())
            ),
        )?;
        ui::field(io.out, "Directory", &self.store.dir().display().to_string())?;
        writeln!(io.out)?;
        show_listening(io, &listening)?;
        self.warn_permissions(io);
        Ok(())
    }

    pub(super) fn peers(&self, io: &mut Io<'_>) -> Result<(), CommandError> {
        let known = self.store.load_known_peers()?;
        let peers = known.peers();

        if self.json {
            let list: Vec<PeerJson> = peers.iter().map(|p| peer_json(p)).collect();
            return write_json(io, &list);
        }

        if peers.is_empty() {
            writeln!(
                io.out,
                "No paired peers yet. Use `beam pair <INVITE> --name <name>` to add one."
            )?;
            return Ok(());
        }

        let rows: Vec<Vec<String>> = peers
            .iter()
            .map(|p| {
                vec![
                    p.name.clone(),
                    p.short_id().grouped(),
                    p.fingerprint().short(),
                    p.added_date(),
                ]
            })
            .collect();
        ui::table(io.out, &["NAME", "SHORT ID", "FINGERPRINT", "ADDED"], &rows)?;
        self.warn_permissions(io);
        Ok(())
    }

    pub(super) fn rename(
        &self,
        old_name: &str,
        new_name: &str,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let mut known = self.store.load_known_peers()?;
        known.rename(old_name, new_name)?;
        self.store.save_known_peers(&known)?;
        writeln!(io.out, "Renamed {old_name} to {new_name}.")?;
        Ok(())
    }

    pub(super) fn remove(
        &self,
        name: &str,
        assume_yes: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let mut known = self.store.load_known_peers()?;
        let peer = known
            .lookup(name)
            .ok_or_else(|| PeerError::NotFound(name.to_string()))?;
        let peer_name = peer.name.clone();
        let fingerprint = peer.fingerprint().to_string();

        if !assume_yes {
            ui::field(io.out, "Name", &peer_name)?;
            ui::field(io.out, "Fingerprint", &fingerprint)?;
            let question = format!("Remove {peer_name}?");
            if !ui::confirm(io.input, io.out, &question)? {
                writeln!(io.out, "Cancelled.")?;
                return Ok(());
            }
        }

        known.remove(&peer_name)?;
        self.store.save_known_peers(&known)?;
        writeln!(io.out, "Removed {peer_name}.")?;
        Ok(())
    }
}

fn peer_json(peer: &Peer) -> PeerJson {
    PeerJson {
        name: peer.name.clone(),
        short_id: peer.short_id().to_string(),
        fingerprint: peer.fingerprint().to_string(),
        key_type: KEY_TYPE,
        public_key: encode_public_key(&peer.public_key),
        added: peer.added_rfc3339(),
    }
}

pub(super) fn write_json<T: Serialize>(io: &mut Io<'_>, value: &T) -> Result<(), CommandError> {
    let text = serde_json::to_string_pretty(value)?;
    writeln!(io.out, "{text}")?;
    Ok(())
}

/// A best-effort machine name for the public key comment. It is cosmetic, so a
/// missing value is not an error.
fn hostname() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(name) = std::env::var(key)
            && !name.trim().is_empty()
        {
            return name;
        }
    }
    String::new()
}

/// The part of `whoami` about a running `listen`: its invite and the pairing
/// code that works right now (ADR-0037).
fn show_listening(io: &mut Io<'_>, listening: &Listening) -> std::io::Result<()> {
    let status = match listening {
        Listening::No => {
            return writeln!(
                io.out,
                "beam listen is not running. Run it to pair with a device or receive files."
            );
        }
        Listening::Unreadable => {
            return writeln!(
                io.out,
                "beam listen is running, but what it offers could not be read. \
                 Restart it to see its invite and pairing code."
            );
        }
        Listening::Yes(status) => status,
    };
    let now = listen_status::unix(std::time::SystemTime::now());
    writeln!(io.out, "beam listen is running:")?;
    ui::field(io.out, "Invite", &status.invite)?;
    match &status.pairing {
        PairingStatus::Live { code, expires_at } => {
            let grouped = PairingCode::parse(code)
                .map(|c| c.grouped())
                .unwrap_or_else(|_| code.clone());
            ui::field(io.out, "Pairing code", &grouped)?;
            let left = expires_at.saturating_sub(now);
            let when = if left == 0 {
                "now; run `beam whoami` again in a moment for the next one".to_string()
            } else {
                format!("in {}", minutes(left))
            };
            ui::field(io.out, "Code expires", &when)?;
        }
        PairingStatus::InUse => ui::field(
            io.out,
            "Pairing code",
            "in use: a device is pairing right now",
        )?,
        PairingStatus::Paused { until } => ui::field(
            io.out,
            "Pairing code",
            &format!(
                "none: paused for {} s after a failed attempt",
                until.saturating_sub(now).max(1)
            ),
        )?,
        PairingStatus::Off { failures } => ui::field(
            io.out,
            "Pairing code",
            &format!(
                "none: pairing is off after {failures} failed attempts; restart `beam listen`"
            ),
        )?,
    }
    Ok(())
}

/// Whole minutes, rounded up, in words.
fn minutes(secs: u64) -> String {
    match secs.div_ceil(60) {
        1 => "1 minute".to_string(),
        n => format!("{n} minutes"),
    }
}
