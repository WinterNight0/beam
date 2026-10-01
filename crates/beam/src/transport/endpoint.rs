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
//!   lookup. Addresses travel in invites and are saved in `known_peers`; the
//!   relay is set explicitly from `config.toml`. See `docs/n0-data.md`, S-17
//!   and ADR-0036.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use iroh::endpoint::{BindOpts, IdleTimeout, QuicTransportConfig, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey};

use crate::config::Relay;
use crate::identity::Identity;

/// The ALPN protocol id of beam pairing. The version is in the name: a peer
/// that speaks a different pairing protocol fails the QUIC handshake instead
/// of misreading messages.
pub const PAIR_ALPN: &[u8] = b"beam/pair/1";

/// The ALPN protocol id of file transfers: the M2 engine, unchanged, on a
/// QUIC stream. Versioned like the pairing one.
pub const XFER_ALPN: &[u8] = b"beam/xfer/1";

/// How long a connection may go without hearing from the peer before it is
/// considered gone. iroh sends keep-alives every 5 s, so a live peer is never
/// idle this long; a crashed one is noticed within 15 s instead of noq's 30 s
/// default. That matters to `listen`, which holds its one transfer slot until
/// the sender is known to be gone (ADR-0030).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

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
///
/// `port` is the UDP port to bind, or 0 for a random one. `listen` asks for a
/// fixed port so that its invite, and the addresses its peers saved from it,
/// stay the same from one run to the next. If that port is taken — another
/// `listen`, or another program — this falls back to a random port rather than
/// failing; [`bound_port`] says which one it got.
pub async fn bind(
    identity: &Identity,
    relay: &Relay,
    bind: Bind,
    port: u16,
    alpns: &[&[u8]],
) -> Result<Endpoint, EndpointError> {
    match bind_on(identity, relay, bind, port, alpns).await {
        Err(_) if port != 0 => bind_on(identity, relay, bind, 0, alpns).await,
        result => result,
    }
}

async fn bind_on(
    identity: &Identity,
    relay: &Relay,
    bind: Bind,
    port: u16,
    alpns: &[&[u8]],
) -> Result<Endpoint, EndpointError> {
    let secret = SecretKey::from_bytes(&identity.signing_key().to_bytes());
    let relay_mode = match relay {
        Relay::Disabled => RelayMode::Disabled,
        Relay::Url(url) => RelayMode::custom([url.clone()]),
    };

    let transport = QuicTransportConfig::builder()
        .max_idle_timeout(Some(
            IdleTimeout::try_from(IDLE_TIMEOUT).expect("15 s is a valid idle timeout"),
        ))
        .build();
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(relay_mode)
        .transport_config(transport)
        .alpns(alpns.iter().map(|a| a.to_vec()).collect());
    let invalid = |e: iroh::endpoint::InvalidSocketAddr| EndpointError::Bind(e.to_string());
    match (bind, port) {
        (Bind::Loopback, port) => {
            builder = builder
                .clear_ip_transports()
                .bind_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
                .map_err(invalid)?;
        }
        // iroh's own default: both families, random ports.
        (Bind::Any, 0) => {}
        (Bind::Any, port) => {
            // IPv4 is required; IPv6 is welcome but may not exist.
            builder = builder
                .clear_ip_transports()
                .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
                .map_err(invalid)?
                .bind_addr_with_opts(
                    SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
                    BindOpts::default().set_is_required(false),
                )
                .map_err(invalid)?;
        }
    }
    builder
        .bind()
        .await
        .map_err(|e| EndpointError::Bind(e.to_string()))
}

/// The UDP port an endpoint ended up on, for telling the person when the
/// configured one was taken.
pub fn bound_port(endpoint: &Endpoint) -> Option<u16> {
    endpoint
        .bound_sockets()
        .into_iter()
        .find(|s| s.is_ipv4())
        .map(|s| s.port())
}

/// The address to put in an invite: this endpoint's id, its relay if one is
/// configured and reachable, and its direct addresses.
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
        let endpoint = bind(&identity, &Relay::Disabled, Bind::Loopback, 0, &[PAIR_ALPN])
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

    #[tokio::test]
    async fn a_taken_port_falls_back_to_a_random_one() {
        let first = bind(
            &vectors::identity("alpha"),
            &Relay::Disabled,
            Bind::Loopback,
            0,
            &[],
        )
        .await
        .unwrap();
        let taken = bound_port(&first).unwrap();

        let second = bind(
            &vectors::identity("bravo"),
            &Relay::Disabled,
            Bind::Loopback,
            taken,
            &[],
        )
        .await
        .expect("a taken port is not fatal");
        let got = bound_port(&second).unwrap();
        assert_ne!(got, taken);
        assert_ne!(got, 0);
        first.close().await;
        second.close().await;
    }

    #[tokio::test]
    async fn a_free_port_is_the_one_bound() {
        let free = std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let endpoint = bind(
            &vectors::identity("alpha"),
            &Relay::Disabled,
            Bind::Loopback,
            free,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(bound_port(&endpoint), Some(free));
        endpoint.close().await;
    }
}
