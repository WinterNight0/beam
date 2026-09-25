//! The iroh endpoint: beam's device key, used as a QUIC identity.
//!
//! Two properties matter here, and both are tested:
//!
//! * **The endpoint id is the device's public key.** iroh's `SecretKey` wraps
//!   the same `ed25519_dalek::SigningKey` beam keeps in `~/.beam/id_ed25519`,
//!   so there is no second identity to keep in step. A connection's
//!   `remote_id()` is therefore a beam public key that the peer has *proved*
//!   it holds the private half of, during the TLS handshake. See ADR-0025.
//! * **No n0 discovery.** The endpoint is built from `presets::Minimal`, which
//!   installs a crypto provider and nothing else: no pkarr publisher, no DNS
//!   lookup. Addresses come from beam's own rendezvous server. The relay is set
//!   explicitly from `config.toml`. See `docs/n0-data.md` and S-17.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey};

use crate::config::Relay;
use crate::identity::Identity;

/// The ALPN protocol id of beam pairing. The version is in the name: a peer
/// that speaks a different pairing protocol fails the QUIC handshake instead
/// of misreading messages.
pub const PAIR_ALPN: &[u8] = b"beam/pair/1";

/// How long to wait for the relay before announcing direct addresses only.
const RELAY_WAIT: Duration = Duration::from_secs(10);

/// Where the endpoint's UDP socket is bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bind {
    /// Every interface, on a random port. What a real device uses.
    #[default]
    Any,
    /// `127.0.0.1` only, on a random port. For tests on one machine: nothing
    /// is reachable from the network, and the advertised address is the one
    /// that actually works.
    Loopback,
}

/// Why an endpoint could not be created.
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("could not open a network endpoint: {0}")]
    Bind(String),
}

/// Opens an iroh endpoint that uses this device's identity.
pub async fn bind(
    identity: &Identity,
    relay: &Relay,
    bind: Bind,
    alpns: &[&[u8]],
) -> Result<Endpoint, EndpointError> {
    let secret = SecretKey::from_bytes(&identity.signing_key().to_bytes());
    let relay_mode = match relay {
        Relay::Disabled => RelayMode::Disabled,
        Relay::Url(url) => RelayMode::custom([url.clone()]),
    };

    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(relay_mode)
        .alpns(alpns.iter().map(|a| a.to_vec()).collect());
    if bind == Bind::Loopback {
        builder = builder
            .clear_ip_transports()
            .bind_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .map_err(|e| EndpointError::Bind(e.to_string()))?;
    }
    builder
        .bind()
        .await
        .map_err(|e| EndpointError::Bind(e.to_string()))
}

/// The address to give the rendezvous server: this endpoint's id, its relay
/// if one is configured and reachable, and its direct addresses.
///
/// With a relay configured, this waits briefly for the relay connection so the
/// relay URL is included. If the relay cannot be reached the address still
/// carries the direct addresses, which is enough on a LAN.
pub async fn advertised_addr(endpoint: &Endpoint, relay: &Relay, bind: Bind) -> EndpointAddr {
    if let Relay::Url(_) = relay {
        let _ = tokio::time::timeout(RELAY_WAIT, endpoint.online()).await;
    }
    let addr = endpoint.addr();
    match bind {
        Bind::Any => addr,
        // A loopback endpoint knows its port, and nothing else is reachable.
        Bind::Loopback => {
            let mut only_loopback = EndpointAddr::new(addr.id);
            for socket in endpoint.bound_sockets() {
                if socket.ip().is_loopback() {
                    only_loopback = only_loopback.with_ip_addr(socket);
                }
            }
            only_loopback
        }
    }
}

/// The iroh endpoint id that belongs to a beam public key.
pub fn endpoint_id(key: &VerifyingKey) -> PublicKey {
    PublicKey::from_bytes(key.as_bytes()).expect("a valid Ed25519 key is a valid endpoint id")
}

/// The beam public key behind an iroh endpoint id.
pub fn verifying_key(id: &PublicKey) -> VerifyingKey {
    VerifyingKey::from_bytes(id.as_bytes()).expect("an endpoint id is a valid Ed25519 key")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors;

    #[test]
    fn the_endpoint_id_is_the_beam_public_key_byte_for_byte() {
        let identity = vectors::identity("alpha");
        let secret = SecretKey::from_bytes(&identity.signing_key().to_bytes());
        let id = secret.public();

        assert_eq!(id.as_bytes(), identity.verifying_key().as_bytes());
        assert_eq!(endpoint_id(&identity.verifying_key()), id);
        assert_eq!(verifying_key(&id), identity.verifying_key());
    }

    #[tokio::test]
    async fn a_bound_endpoint_uses_the_device_key() {
        let identity = vectors::identity("bravo");
        let endpoint = bind(&identity, &Relay::Disabled, Bind::Loopback, &[PAIR_ALPN])
            .await
            .unwrap();
        assert_eq!(endpoint.id(), endpoint_id(&identity.verifying_key()));

        let addr = advertised_addr(&endpoint, &Relay::Disabled, Bind::Loopback).await;
        assert_eq!(addr.id, endpoint.id());
        assert!(addr.relay_urls().next().is_none(), "relay is disabled");
        assert!(
            addr.ip_addrs().all(|a| a.ip().is_loopback()),
            "{addr:?} advertises a non-loopback address"
        );
        assert!(addr.ip_addrs().next().is_some(), "{addr:?} is not dialable");
        endpoint.close().await;
    }
}
