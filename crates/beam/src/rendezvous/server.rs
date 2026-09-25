//! The rendezvous server: Short ID → iroh endpoint address, in memory.
//!
//! What it holds is small and short-lived: for each device that is currently
//! waiting, its public key and the address it announced, until the device
//! disconnects or stops refreshing (90 s). Nothing is written to disk, and
//! nothing is logged that ties a Short ID to an IP address.
//!
//! What it does **not** have to be is trustworthy. Registrations are signed,
//! and clients re-check every answer, so the worst a malicious server can do
//! is refuse service or hand out an address that fails to connect. It cannot
//! make one device pair with another. See ADR-0027.
//!
//! The `beam-server` binary is a thin wrapper around [`serve`]. The code lives
//! in the library so that beam's own tests can run a real server in-process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use futures_util::{SinkExt, StreamExt};
use iroh::EndpointAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_websockets::{Limits, Message, ServerBuilder};

use super::proto::{
    ClientMessage, MAX_MESSAGE, PATH, PeerRecord, RegisterError, Registration, ServerMessage,
    unix_now, verify_registration,
};
use crate::identity::{ShortId, encode_public_key};

/// Server tuning. The defaults are the ones ADR-0027 argues for.
#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    /// How long a registration lives without being refreshed.
    pub ttl: Duration,
    /// Most live registrations for one Short ID.
    pub max_per_id: usize,
    /// A connection that sends nothing for this long is closed.
    pub idle_timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(90),
            max_per_id: 8,
            idle_timeout: Duration::from_secs(120),
        }
    }
}

/// Which connection made a registration. It goes away with the connection.
type Owner = u64;

#[derive(Debug)]
struct Entry {
    key: VerifyingKey,
    addr: EndpointAddr,
    timestamp: i64,
    expires: Instant,
    owner: Owner,
}

/// The table itself. Separate from the network code so it can be tested
/// without sockets.
#[derive(Debug, Default)]
pub struct Registry {
    by_id: HashMap<ShortId, Vec<Entry>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or refreshes a verified registration.
    ///
    /// A key that is already registered under this Short ID is updated in
    /// place, but only by a strictly newer timestamp, so a captured
    /// registration cannot be replayed to put back an old address.
    pub fn register(
        &mut self,
        registration: Registration,
        owner: Owner,
        now: Instant,
        config: &ServerConfig,
    ) -> Result<(), RegisterError> {
        let entries = self.by_id.entry(registration.short_id).or_default();
        entries.retain(|e| e.expires > now);

        if let Some(existing) = entries
            .iter_mut()
            .find(|e| e.key == registration.public_key)
        {
            if registration.timestamp <= existing.timestamp {
                return Err(RegisterError::Replay);
            }
            existing.addr = registration.addr;
            existing.timestamp = registration.timestamp;
            existing.expires = now + config.ttl;
            existing.owner = owner;
            return Ok(());
        }

        if entries.len() >= config.max_per_id {
            return Err(RegisterError::Full);
        }
        entries.push(Entry {
            key: registration.public_key,
            addr: registration.addr,
            timestamp: registration.timestamp,
            expires: now + config.ttl,
            owner,
        });
        Ok(())
    }

    /// Every live entry for `short_id`. More than one means a collision,
    /// accidental or ground; the client sorts it out, see ADR-0027.
    pub fn lookup(&mut self, short_id: ShortId, now: Instant) -> Vec<PeerRecord> {
        let Some(entries) = self.by_id.get_mut(&short_id) else {
            return Vec::new();
        };
        entries.retain(|e| e.expires > now);
        let records = entries
            .iter()
            .map(|e| PeerRecord {
                public_key: encode_public_key(&e.key),
                addr: e.addr.clone(),
            })
            .collect();
        if entries.is_empty() {
            self.by_id.remove(&short_id);
        }
        records
    }

    /// Forgets everything a connection registered.
    pub fn release(&mut self, owner: Owner) {
        self.by_id.retain(|_, entries| {
            entries.retain(|e| e.owner != owner);
            !entries.is_empty()
        });
    }

    /// Number of live-or-not-yet-pruned entries. For tests and the status line.
    pub fn len(&self) -> usize {
        self.by_id.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Accepts connections forever.
pub async fn serve(listener: TcpListener, config: ServerConfig) -> std::io::Result<()> {
    let registry = Arc::new(Mutex::new(Registry::new()));
    let next_owner = AtomicU64::new(1);
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            // A single failed accept (e.g. too many open files) must not take
            // the server down.
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let owner = next_owner.fetch_add(1, Ordering::Relaxed);
        let registry = Arc::clone(&registry);
        tokio::spawn(async move {
            handle(stream, owner, &registry, &config).await;
            if let Ok(mut registry) = registry.lock() {
                registry.release(owner);
            }
        });
    }
}

async fn handle(
    stream: TcpStream,
    owner: Owner,
    registry: &Mutex<Registry>,
    config: &ServerConfig,
) {
    let limits = Limits::default().max_payload_len(Some(MAX_MESSAGE));
    let Ok((request, mut ws)) = ServerBuilder::new().limits(limits).accept(stream).await else {
        return;
    };
    if request.uri().path() != PATH {
        let _ = ws.close().await;
        return;
    }

    loop {
        let message = match tokio::time::timeout(config.idle_timeout, ws.next()).await {
            Ok(Some(Ok(message))) => message,
            // Closed, broken, or idle for too long.
            _ => break,
        };
        if message.is_close() {
            break;
        }
        let Some(text) = message.as_text() else {
            // Pings are answered by the library; anything else is ignored.
            continue;
        };
        let reply = respond(text, owner, registry, config);
        let reply = serde_json::to_string(&reply).expect("replies always serialise");
        if ws.send(Message::text(reply)).await.is_err() {
            break;
        }
    }
    let _ = ws.close().await;
}

/// Answers one request.
fn respond(
    text: &str,
    owner: Owner,
    registry: &Mutex<Registry>,
    config: &ServerConfig,
) -> ServerMessage {
    let request: ClientMessage = match serde_json::from_str(text) {
        Ok(request) => request,
        Err(e) => return error("malformed", &e.to_string()),
    };
    let Ok(mut registry) = registry.lock() else {
        return error("internal", "the server is shutting down");
    };
    match request {
        ClientMessage::Register { body, signature } => {
            let result = verify_registration(&body, &signature, unix_now())
                .and_then(|reg| registry.register(reg, owner, Instant::now(), config));
            match result {
                Ok(()) => ServerMessage::Registered {
                    ttl_secs: config.ttl.as_secs(),
                },
                Err(e) => error(e.code(), &e.to_string()),
            }
        }
        ClientMessage::Lookup { short_id } => match short_id.parse::<ShortId>() {
            Ok(id) => ServerMessage::Found {
                short_id: id.to_string(),
                peers: registry.lookup(id, Instant::now()),
            },
            Err(e) => error("malformed", &e.to_string()),
        },
    }
}

fn error(code: &str, message: &str) -> ServerMessage {
    ServerMessage::Error {
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, vectors};
    use crate::transport::endpoint::endpoint_id;

    fn registration(identity: &Identity, timestamp: i64, port: u16) -> Registration {
        Registration {
            short_id: identity.short_id(),
            public_key: identity.verifying_key(),
            timestamp,
            addr: EndpointAddr::new(endpoint_id(&identity.verifying_key()))
                .with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        }
    }

    /// A registration for `identity`'s key, filed under `short_id` — how a
    /// collision looks once the signature checks have passed. Producing a real
    /// one would mean grinding ~2^30 keys, which is exactly the attack
    /// ADR-0027 accepts; the table's job is only to keep both entries.
    fn colliding(identity: &Identity, short_id: ShortId, timestamp: i64) -> Registration {
        Registration {
            short_id,
            ..registration(identity, timestamp, 9)
        }
    }

    #[test]
    fn a_registration_can_be_looked_up_until_it_expires() {
        let config = ServerConfig::default();
        let alpha = vectors::identity("alpha");
        let mut registry = Registry::new();
        let t0 = Instant::now();

        registry
            .register(registration(&alpha, 100, 1), 1, t0, &config)
            .unwrap();
        let found = registry.lookup(alpha.short_id(), t0 + Duration::from_secs(89));
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].public_key,
            encode_public_key(&alpha.verifying_key())
        );

        assert!(
            registry
                .lookup(alpha.short_id(), t0 + config.ttl)
                .is_empty()
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn a_refresh_extends_the_lifetime_and_updates_the_address() {
        let config = ServerConfig::default();
        let alpha = vectors::identity("alpha");
        let mut registry = Registry::new();
        let t0 = Instant::now();

        registry
            .register(registration(&alpha, 100, 1), 1, t0, &config)
            .unwrap();
        let t1 = t0 + Duration::from_secs(60);
        registry
            .register(registration(&alpha, 160, 2), 1, t1, &config)
            .unwrap();

        let found = registry.lookup(alpha.short_id(), t0 + Duration::from_secs(120));
        assert_eq!(found.len(), 1, "refreshed, not duplicated");
        assert!(found[0].addr.ip_addrs().any(|a| a.port() == 2));
    }

    #[test]
    fn an_old_registration_cannot_be_replayed_over_a_newer_one() {
        let config = ServerConfig::default();
        let alpha = vectors::identity("alpha");
        let mut registry = Registry::new();
        let t0 = Instant::now();

        registry
            .register(registration(&alpha, 200, 2), 1, t0, &config)
            .unwrap();
        for stale in [200, 199, 100] {
            assert_eq!(
                registry.register(registration(&alpha, stale, 1), 7, t0, &config),
                Err(RegisterError::Replay),
                "timestamp {stale}"
            );
        }
        let found = registry.lookup(alpha.short_id(), t0);
        assert!(found[0].addr.ip_addrs().any(|a| a.port() == 2));
    }

    #[test]
    fn colliding_short_ids_return_every_entry() {
        let config = ServerConfig::default();
        let alpha = vectors::identity("alpha");
        let mallory = vectors::identity("mallory");
        let mut registry = Registry::new();
        let t0 = Instant::now();

        registry
            .register(registration(&alpha, 100, 1), 1, t0, &config)
            .unwrap();
        registry
            .register(colliding(&mallory, alpha.short_id(), 100), 2, t0, &config)
            .unwrap();

        let found = registry.lookup(alpha.short_id(), t0);
        let keys: Vec<&str> = found.iter().map(|r| r.public_key.as_str()).collect();
        assert_eq!(found.len(), 2, "a collision must not hide the real device");
        assert!(keys.contains(&encode_public_key(&alpha.verifying_key()).as_str()));
        assert!(keys.contains(&encode_public_key(&mallory.verifying_key()).as_str()));
    }

    #[test]
    fn a_short_id_holds_a_bounded_number_of_entries() {
        let config = ServerConfig {
            max_per_id: 2,
            ..ServerConfig::default()
        };
        let alpha = vectors::identity("alpha");
        let mut registry = Registry::new();
        let t0 = Instant::now();
        let id = alpha.short_id();

        registry
            .register(registration(&alpha, 1, 1), 1, t0, &config)
            .unwrap();
        registry
            .register(colliding(&vectors::identity("m1"), id, 1), 2, t0, &config)
            .unwrap();
        assert_eq!(
            registry.register(colliding(&vectors::identity("m2"), id, 1), 3, t0, &config),
            Err(RegisterError::Full)
        );
        // An existing key can still refresh when the Short ID is full.
        registry
            .register(registration(&alpha, 2, 1), 1, t0, &config)
            .unwrap();
    }

    #[test]
    fn closing_a_connection_removes_what_it_registered() {
        let config = ServerConfig::default();
        let alpha = vectors::identity("alpha");
        let bravo = vectors::identity("bravo");
        let mut registry = Registry::new();
        let t0 = Instant::now();

        registry
            .register(registration(&alpha, 1, 1), 1, t0, &config)
            .unwrap();
        registry
            .register(registration(&bravo, 1, 1), 2, t0, &config)
            .unwrap();
        registry.release(1);

        assert!(registry.lookup(alpha.short_id(), t0).is_empty());
        assert_eq!(registry.lookup(bravo.short_id(), t0).len(), 1);
    }

    #[test]
    fn an_unknown_short_id_finds_nothing() {
        let mut registry = Registry::new();
        let id = vectors::identity("alpha").short_id();
        assert!(registry.lookup(id, Instant::now()).is_empty());
    }
}
