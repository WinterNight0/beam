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
use iroh::endpoint::{BindOpts, IdleTimeout, QuicTransportConfig, VarInt, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey, Watcher};

use crate::config::Relay;
use crate::identity::Identity;

/// The ALPN protocol id of beam pairing. The version is in the name: a peer
/// that speaks a different pairing protocol fails the QUIC handshake instead
/// of misreading messages.
pub const PAIR_ALPN: &[u8] = b"beam/pair/1";

/// The ALPN protocol id of file transfers: the M2 engine, unchanged, on a
/// QUIC stream. Versioned like the pairing one.
pub const XFER_ALPN: &[u8] = b"beam/xfer/1";

/// Version 2 of file transfers: the same messages, but the sender may have
/// several chunks in flight and re-sends a rejected one after the others
/// (ADR-0040). A receiver that only knows version 1 needs each chunk answered
/// before the next. The name is agreed in the TLS handshake, before any
/// message, so the two can never be mixed up: a new sender offers both, and
/// a listener picks the newest it knows ([`transfer_alpns`]).
pub const XFER_ALPN_V2: &[u8] = b"beam/xfer/2";

/// The transfer protocols `listen` accepts, newest first. The TLS server picks
/// the first of its own list that the client offered.
pub fn transfer_alpns() -> [&'static [u8]; 2] {
    [XFER_ALPN_V2, XFER_ALPN]
}

/// How long a connection may go without hearing from the peer before it is
/// considered gone. iroh sends keep-alives every 5 s, so a live peer is never
/// idle this long; a crashed one is noticed within 15 s instead of noq's 30 s
/// default. That matters to `listen`, which holds its one transfer slot until
/// the sender is known to be gone (ADR-0030).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// How many bytes of one stream may be in flight, unacknowledged. A stream
/// can go no faster than this divided by the round-trip time; noq's default of
/// 1.25 MB allows only about 12.5 MB/s at 100 ms. 16 MiB holds a whole 4 MiB
/// chunk several times over, so a chunk goes out in one round trip instead of
/// four (performance plan, step 3).
pub const STREAM_WINDOW: u32 = 16 * 1024 * 1024;

/// How many bytes a peer may have in flight to us across *all* streams of one
/// connection. noq sets no limit, so without this a peer could open its 100
/// allowed streams and make us buffer 100 stream windows. 32 MiB bounds the
/// memory one connection can claim, while leaving a full stream window free
/// for the transfer stream.
pub const CONNECTION_WINDOW: u32 = 2 * STREAM_WINDOW;

/// How long to wait for the relay before announcing direct addresses only.
const RELAY_WAIT: Duration = Duration::from_secs(10);

/// With no relay, how long to wait for the router to report a port mapping
/// (UPnP, NAT-PMP, PCP). That mapping is the only public address a device
/// can learn without a relay (ADR-0038).
const PORTMAP_WAIT: Duration = Duration::from_secs(3);

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

/// The QUIC settings every beam endpoint uses: the idle timeout and the flow
/// control windows above. Public so the throughput benchmark can build an
/// endpoint that behaves like beam's.
pub fn transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .max_idle_timeout(Some(
            IdleTimeout::try_from(IDLE_TIMEOUT).expect("15 s is a valid idle timeout"),
        ))
        .stream_receive_window(VarInt::from_u32(STREAM_WINDOW))
        .receive_window(VarInt::from_u32(CONNECTION_WINDOW))
        .send_window(u64::from(CONNECTION_WINDOW))
        .build()
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

    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(relay_mode)
        .transport_config(transport_config())
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
    match (relay, bind) {
        (Relay::Url(_), _) => {
            let _ = tokio::time::timeout(RELAY_WAIT, endpoint.online()).await;
        }
        // No relay to learn a public address from: give the router's port
        // mapping a moment to appear, if the router offers one.
        (Relay::Disabled, Bind::Any) => {
            let mut watcher = endpoint.watch_addr();
            let _ = tokio::time::timeout(PORTMAP_WAIT, async {
                loop {
                    if watcher.get().ip_addrs().any(|a| is_public(a.ip())) {
                        break;
                    }
                    if watcher.updated().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }
        (Relay::Disabled, Bind::Loopback) => {}
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

/// Whether an address is reachable from the internet: not loopback,
/// private, link-local, carrier-grade NAT (100.64.0.0/10), unique-local or
/// unspecified.
pub fn is_public(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || (a == 100 && (64..128).contains(&b)))
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unicast_link_local()
                || v6.is_unique_local())
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

    #[test]
    fn only_internet_reachable_addresses_count_as_public() {
        for public in ["8.8.8.8", "202.28.63.1", "2001:4860::8888"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for local in [
            "10.0.0.5",
            "192.168.1.20",
            "172.16.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "0.0.0.0",
            "203.0.113.7",
            "::1",
            "fe80::1",
            "fd00::1",
        ] {
            assert!(!is_public(local.parse().unwrap()), "{local}");
        }
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
