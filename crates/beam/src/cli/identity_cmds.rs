//! The identity commands: `init`, `whoami`, `peers`, `rename`, `remove`.

use serde::Serialize;

use super::{App, CommandError, Io};
use crate::identity::{Identity, KEY_TYPE, Peer, PeerError, StoreError, encode_public_key};
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
            "Give your Short ID to a peer so they can pair with you."
        )?;
        self.warn_permissions(io);
        Ok(())
    }

    pub(super) fn whoami(&self, io: &mut Io<'_>) -> Result<(), CommandError> {
        let identity = self.store.load_identity()?;

        if self.json {
            return write_json(io, &self.self_json(&identity));
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
                "No paired peers yet. Use `beam pair <ID> --name <name>` to add one."
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

fn write_json<T: Serialize>(io: &mut Io<'_>, value: &T) -> Result<(), CommandError> {
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
