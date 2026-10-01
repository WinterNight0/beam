//! Invites: what a joiner needs to reach a device that is waiting to pair, as
//! one line to copy and paste.
//!
//! An invite replaces the rendezvous server's Short ID lookup (ADR-0036). It
//! carries the waiting device's public key, the relay it can be reached
//! through, and its direct addresses:
//!
//! ```text
//! beam1 + base32( version | public key | relay | addresses | checksum )
//! ```
//!
//! **An invite is a routing hint, not a credential.** Like the Short ID before
//! it, it is safe to send over any chat. A connection to the key in it only
//! completes against the holder of that key, and pairing still needs the code
//! and both people saying yes (ADR-0026). A tampered invite gets the joiner a
//! failed pairing, never a wrong peer.
//!
//! After pairing, the relay and the addresses are saved next to the peer's key
//! in `known_peers` (`relay=` and `addrs=`), and that is how `beam send` finds
//! the peer again: by its key, through its relay, with no server of ours.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use ed25519_dalek::VerifyingKey;
use iroh::{EndpointAddr, RelayUrl};
use sha2::{Digest, Sha256};

use crate::config::{DEFAULT_RELAY, Relay};
use crate::identity::{Fingerprint, Peer, ShortId};
use crate::transport::endpoint::{endpoint_id, verifying_key};

/// Every invite starts with this, so it is recognisable and versioned.
pub const PREFIX: &str = "beam1";

/// The `known_peers` attribute holding the relay a peer's invite named, when
/// it differs from this device's own relay.
pub const RELAY_ATTR: &str = "relay";

/// The `known_peers` attribute holding the direct addresses a peer's invite
/// named, comma-separated.
pub const ADDRS_ATTR: &str = "addrs";

/// The encoding version, inside the base32 as well as in the prefix.
const VERSION: u8 = 1;

/// The relay is beam's built-in default, so its URL is left out.
const FLAG_DEFAULT_RELAY: u8 = 0b01;
/// Another relay; its URL follows.
const FLAG_OTHER_RELAY: u8 = 0b10;

/// The most direct addresses an invite carries. More only makes it longer: a
/// device is usually reachable on one or two.
const MAX_ADDRS: usize = 6;

/// The longest relay URL an invite carries.
const MAX_RELAY_LEN: usize = 200;

/// Truncated SHA-256 of everything before it. Not a security measure — the
/// connection is what proves the key — but it turns a typo into "copy it
/// again" instead of a pairing that fails for no visible reason.
const CHECKSUM_LEN: usize = 4;

const TAG_V4: u8 = 4;
const TAG_V6: u8 = 6;

/// RFC 4648 base32, lowercase: letters and digits only, so a double-click
/// selects the whole invite and case does not matter when it is retyped.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Where a device that is waiting to pair can be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The device's public key, which is also its iroh endpoint id.
    pub key: VerifyingKey,
    /// The relay it is connected to, if any.
    pub relay: Option<RelayUrl>,
    /// Direct addresses it may be reachable on.
    pub addrs: Vec<SocketAddr>,
}

/// Why a string is not a usable invite. The messages never quote the input:
/// it is pasted text, and it reaches the terminal (ADR-0034).
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    #[error(
        "that is not an invite: an invite starts with \"{PREFIX}\". Copy the whole \
         Invite line that `beam listen` shows on the other device"
    )]
    Prefix,
    #[error("the invite is incomplete or damaged; copy it again")]
    Malformed,
    #[error("the invite has a typo; copy it again rather than retyping it")]
    Checksum,
    #[error("the invite comes from a newer beam (format {0}); update beam on this device")]
    Version(u8),
}

impl Invite {
    /// The invite for an endpoint's advertised address.
    ///
    /// IPv4 addresses come first, and IPv6 link-local addresses are left out:
    /// they only work with an interface index, which an invite cannot carry
    /// usefully to another machine.
    pub fn new(addr: &EndpointAddr) -> Self {
        let mut addrs: Vec<SocketAddr> = addr
            .ip_addrs()
            .filter(|a| !a.ip().is_unspecified())
            .filter(|a| match a.ip() {
                IpAddr::V6(ip) => !ip.is_unicast_link_local(),
                IpAddr::V4(_) => true,
            })
            .copied()
            .collect();
        addrs.sort_by_key(|a| a.is_ipv6());
        addrs.dedup();
        addrs.truncate(MAX_ADDRS);
        Self {
            key: verifying_key(&addr.id),
            relay: addr.relay_urls().next().cloned(),
            addrs,
        }
    }

    /// The Short ID of the device. Pairing binds the code to it (ADR-0026),
    /// and both sides derive it from the key.
    pub fn short_id(&self) -> ShortId {
        self.fingerprint().short_id()
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.key)
    }

    /// The address to dial: the key, the relay and the direct addresses.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        let mut addr = EndpointAddr::new(endpoint_id(&self.key));
        if let Some(relay) = &self.relay {
            addr = addr.with_relay_url(relay.clone());
        }
        for socket in &self.addrs {
            addr = addr.with_ip_addr(*socket);
        }
        addr
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![VERSION];
        out.extend_from_slice(self.key.as_bytes());
        match &self.relay {
            None => out.push(0),
            Some(relay) if is_default_relay(relay) => out.push(FLAG_DEFAULT_RELAY),
            Some(relay) => {
                let url = relay.to_string();
                debug_assert!(url.len() <= MAX_RELAY_LEN, "checked when it was parsed");
                out.push(FLAG_OTHER_RELAY);
                out.push(url.len().min(MAX_RELAY_LEN) as u8);
                out.extend_from_slice(&url.as_bytes()[..url.len().min(MAX_RELAY_LEN)]);
            }
        }
        let addrs = &self.addrs[..self.addrs.len().min(MAX_ADDRS)];
        out.push(addrs.len() as u8);
        for socket in addrs {
            match socket.ip() {
                IpAddr::V4(ip) => {
                    out.push(TAG_V4);
                    out.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    out.push(TAG_V6);
                    out.extend_from_slice(&ip.octets());
                }
            }
            out.extend_from_slice(&socket.port().to_be_bytes());
        }
        let checksum = Sha256::digest(&out);
        out.extend_from_slice(&checksum[..CHECKSUM_LEN]);
        out
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, InviteError> {
        if bytes.len() < CHECKSUM_LEN {
            return Err(InviteError::Malformed);
        }
        let (body, checksum) = bytes.split_at(bytes.len() - CHECKSUM_LEN);
        if Sha256::digest(body)[..CHECKSUM_LEN] != *checksum {
            return Err(InviteError::Checksum);
        }

        let mut reader = Reader(body);
        let version = reader.byte()?;
        if version != VERSION {
            return Err(InviteError::Version(version));
        }
        let key_bytes: [u8; 32] = reader.take(32)?.try_into().expect("took 32 bytes");
        let key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| InviteError::Malformed)?;

        let relay = match reader.byte()? {
            0 => None,
            FLAG_DEFAULT_RELAY => Some(default_relay()),
            FLAG_OTHER_RELAY => {
                let len = reader.byte()? as usize;
                let text =
                    std::str::from_utf8(reader.take(len)?).map_err(|_| InviteError::Malformed)?;
                Some(parse_relay(text).ok_or(InviteError::Malformed)?)
            }
            _ => return Err(InviteError::Malformed),
        };

        let count = reader.byte()? as usize;
        if count > MAX_ADDRS {
            return Err(InviteError::Malformed);
        }
        let mut addrs = Vec::with_capacity(count);
        for _ in 0..count {
            let ip = match reader.byte()? {
                TAG_V4 => {
                    let octets: [u8; 4] = reader.take(4)?.try_into().expect("took 4 bytes");
                    IpAddr::V4(Ipv4Addr::from(octets))
                }
                TAG_V6 => {
                    let octets: [u8; 16] = reader.take(16)?.try_into().expect("took 16 bytes");
                    IpAddr::V6(Ipv6Addr::from(octets))
                }
                _ => return Err(InviteError::Malformed),
            };
            let port = u16::from_be_bytes(reader.take(2)?.try_into().expect("took 2 bytes"));
            addrs.push(SocketAddr::new(ip, port));
        }
        if !reader.0.is_empty() {
            return Err(InviteError::Malformed);
        }
        Ok(Self { key, relay, addrs })
    }
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{}", base32_encode(&self.to_bytes()))
    }
}

impl FromStr for Invite {
    type Err = InviteError;

    /// Reads a pasted invite. Surrounding whitespace, case, and any spaces or
    /// dashes a chat program or a person put inside it are ignored.
    fn from_str(text: &str) -> Result<Self, InviteError> {
        let cleaned: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect::<String>()
            .to_ascii_lowercase();
        let encoded = cleaned.strip_prefix(PREFIX).ok_or(InviteError::Prefix)?;
        let bytes = base32_decode(encoded).ok_or(InviteError::Malformed)?;
        Self::from_bytes(&bytes)
    }
}

/// Saves where `invite` says its device can be found on that device's entry.
///
/// Only *where* changes. The key is not touched here or anywhere else: a new
/// key means pairing again (rule 3). The relay is saved only when it differs
/// from this device's own, so a peer on the shared default follows the default
/// if it ever changes.
pub fn remember(peer: &mut Peer, invite: &Invite, own_relay: &Relay) {
    debug_assert_eq!(peer.public_key, invite.key, "an invite for another key");
    let relay = invite
        .relay
        .as_ref()
        .filter(|relay| !matches!(own_relay, Relay::Url(own) if own == *relay))
        .map(|relay| relay.to_string());
    peer.set_attr(RELAY_ATTR, relay);
    let addrs = (!invite.addrs.is_empty()).then(|| {
        invite
            .addrs
            .iter()
            .map(SocketAddr::to_string)
            .collect::<Vec<_>>()
            .join(",")
    });
    peer.set_attr(ADDRS_ATTR, addrs);
}

/// Where to look for a paired peer: its key, the relay its invite named (or
/// this device's own relay), and the direct addresses its invite named.
///
/// The attributes are in a file people may edit, so a value that does not
/// parse is skipped rather than fatal: the worst it can do is make the peer
/// unreachable, and the key is what the connection proves either way.
pub fn peer_addr(peer: &Peer, own_relay: &Relay) -> EndpointAddr {
    let mut addr = EndpointAddr::new(endpoint_id(&peer.public_key));
    let relay = peer
        .attr(RELAY_ATTR)
        .and_then(parse_relay)
        .or_else(|| match own_relay {
            Relay::Url(url) => Some(url.clone()),
            Relay::Disabled => None,
        });
    if let Some(relay) = relay {
        addr = addr.with_relay_url(relay);
    }
    for socket in peer
        .attr(ADDRS_ATTR)
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse::<SocketAddr>().ok())
    {
        addr = addr.with_ip_addr(socket);
    }
    addr
}

fn default_relay() -> RelayUrl {
    DEFAULT_RELAY.parse().expect("the default relay URL parses")
}

fn is_default_relay(relay: &RelayUrl) -> bool {
    *relay == default_relay()
}

/// An `http(s)` relay URL short enough for an invite.
fn parse_relay(text: &str) -> Option<RelayUrl> {
    if text.len() > MAX_RELAY_LEN || !(text.starts_with("https://") || text.starts_with("http://"))
    {
        return None;
    }
    text.parse().ok()
}

/// Reads an invite's bytes in order, failing on a short read.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], InviteError> {
        if self.0.len() < n {
            return Err(InviteError::Malformed);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, InviteError> {
        Ok(self.take(1)?[0])
    }
}

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Decodes unpadded lowercase base32. Leftover bits must be zero, so every
/// byte string has exactly one encoding.
fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let value = BASE32.iter().position(|&b| b == c)? as u32;
        buffer = ((buffer << 5) | value) & 0xFFFF;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    if bits >= 5 || buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors;

    fn invite(relay: Option<&str>, addrs: &[&str]) -> Invite {
        Invite {
            key: vectors::verifying_key("alpha"),
            relay: relay.map(|r| r.parse().unwrap()),
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn base32_round_trips_every_length() {
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = base32_encode(&bytes);
            assert!(text.bytes().all(|c| BASE32.contains(&c)), "{text}");
            assert_eq!(base32_decode(&text).unwrap(), bytes, "length {len}");
        }
        assert_eq!(base32_encode(b"foobar"), "mzxw6ytboi");
    }

    #[test]
    fn an_invite_round_trips() {
        for original in [
            invite(None, &[]),
            invite(Some(DEFAULT_RELAY), &["192.168.1.20:7820"]),
            invite(
                Some("https://relay.example.org/"),
                &["10.0.0.5:7820", "[2001:db8::7]:7820"],
            ),
        ] {
            let text = original.to_string();
            assert!(text.starts_with(PREFIX), "{text}");
            assert_eq!(text.parse::<Invite>().unwrap(), original, "{text}");
        }
    }

    #[test]
    fn the_usual_invite_is_short_enough_to_paste() {
        // The built-in relay costs one byte, not its URL.
        let text = invite(Some(DEFAULT_RELAY), &["192.168.1.20:7820", "10.0.0.5:7820"]).to_string();
        assert!(text.len() <= 100, "{} characters: {text}", text.len());
    }

    #[test]
    fn pasting_is_forgiving_about_case_spaces_and_dashes() {
        let original = invite(Some(DEFAULT_RELAY), &["192.168.1.20:7820"]);
        let text = original.to_string();
        let (a, b) = text.split_at(20);
        for pasted in [
            format!("  {text}\n"),
            text.to_ascii_uppercase(),
            format!("{a}-{b}"),
            format!("{a} {b}"),
        ] {
            assert_eq!(pasted.parse::<Invite>().unwrap(), original, "{pasted:?}");
        }
    }

    #[test]
    fn a_typo_is_caught_by_the_checksum() {
        let text = invite(Some(DEFAULT_RELAY), &["192.168.1.20:7820"]).to_string();
        // Change one character in the key part.
        let at = PREFIX.len() + 10;
        let mut chars: Vec<char> = text.chars().collect();
        chars[at] = if chars[at] == 'a' { 'b' } else { 'a' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(typo.parse::<Invite>(), Err(InviteError::Checksum));
    }

    #[test]
    fn what_is_not_an_invite_is_refused_without_quoting_it() {
        assert_eq!("123456789".parse::<Invite>(), Err(InviteError::Prefix));
        assert_eq!("".parse::<Invite>(), Err(InviteError::Prefix));
        assert_eq!("beam1".parse::<Invite>(), Err(InviteError::Malformed));
        assert_eq!("beam1!!!!".parse::<Invite>(), Err(InviteError::Malformed));
        let text = invite(None, &[]).to_string();
        // A cut-off invite is caught either by its length or by its checksum.
        assert!(matches!(
            text[..text.len() - 3].parse::<Invite>(),
            Err(InviteError::Malformed | InviteError::Checksum)
        ));
        let hostile = "\u{1b}]0;owned\u{07}beam1";
        let err = hostile.parse::<Invite>().unwrap_err().to_string();
        assert!(!err.contains('\u{1b}') && !err.contains("owned"), "{err}");
    }

    #[test]
    fn a_newer_format_says_to_update() {
        let mut bytes = invite(None, &[]).to_bytes();
        bytes.truncate(bytes.len() - CHECKSUM_LEN);
        bytes[0] = 2;
        let checksum = Sha256::digest(&bytes);
        bytes.extend_from_slice(&checksum[..CHECKSUM_LEN]);
        let text = format!("{PREFIX}{}", base32_encode(&bytes));
        assert_eq!(text.parse::<Invite>(), Err(InviteError::Version(2)));
    }

    #[test]
    fn link_local_and_unspecified_addresses_are_left_out_and_ipv4_comes_first() {
        let addr = EndpointAddr::new(endpoint_id(&vectors::verifying_key("alpha")))
            .with_ip_addr("[fe80::1]:7820".parse().unwrap())
            .with_ip_addr("[2001:db8::7]:7820".parse().unwrap())
            .with_ip_addr("0.0.0.0:7820".parse().unwrap())
            .with_ip_addr("192.168.1.20:7820".parse().unwrap());
        let invite = Invite::new(&addr);
        assert_eq!(
            invite.addrs,
            [
                "192.168.1.20:7820".parse::<SocketAddr>().unwrap(),
                "[2001:db8::7]:7820".parse().unwrap()
            ]
        );
        assert_eq!(invite.key, vectors::verifying_key("alpha"));
        assert_eq!(invite.endpoint_addr().id, addr.id);
    }

    #[test]
    fn the_short_id_is_the_one_the_device_shows_for_itself() {
        assert_eq!(
            invite(None, &[]).short_id(),
            vectors::identity("alpha").short_id()
        );
    }

    fn peer() -> Peer {
        Peer::new("alice", vectors::verifying_key("alpha"))
    }

    #[test]
    fn a_peer_on_the_shared_relay_follows_this_devices_relay() {
        let own = Relay::Url(DEFAULT_RELAY.parse().unwrap());
        let mut alice = peer();
        remember(
            &mut alice,
            &invite(Some(DEFAULT_RELAY), &["192.168.1.20:7820"]),
            &own,
        );
        assert_eq!(alice.attr(RELAY_ATTR), None);
        assert_eq!(alice.attr(ADDRS_ATTR), Some("192.168.1.20:7820"));

        let addr = peer_addr(&alice, &own);
        assert_eq!(addr.id, endpoint_id(&alice.public_key));
        assert_eq!(addr.relay_urls().next(), Some(&default_relay()));
        assert_eq!(
            addr.ip_addrs().collect::<Vec<_>>(),
            [&"192.168.1.20:7820".parse::<SocketAddr>().unwrap()]
        );
    }

    #[test]
    fn a_peer_on_another_relay_keeps_it() {
        let own = Relay::Url(DEFAULT_RELAY.parse().unwrap());
        let mut alice = peer();
        remember(
            &mut alice,
            &invite(Some("https://relay.example.org/"), &[]),
            &own,
        );
        assert_eq!(alice.attr(RELAY_ATTR), Some("https://relay.example.org/"));
        assert_eq!(alice.attr(ADDRS_ATTR), None);
        let relays: Vec<String> = peer_addr(&alice, &own)
            .relay_urls()
            .map(|r| r.to_string())
            .collect();
        assert_eq!(relays, ["https://relay.example.org/"]);
    }

    #[test]
    fn with_no_relay_and_no_addresses_there_is_nowhere_to_look() {
        let alice = peer();
        assert!(peer_addr(&alice, &Relay::Disabled).is_empty());
    }

    #[test]
    fn a_hand_edited_attribute_that_does_not_parse_is_skipped() {
        let mut alice = peer();
        alice.set_attr(RELAY_ATTR, Some("ftp://nope".into()));
        alice.set_attr(ADDRS_ATTR, Some("nonsense,10.0.0.5:7820".into()));
        let addr = peer_addr(&alice, &Relay::Disabled);
        assert!(addr.relay_urls().next().is_none());
        assert_eq!(addr.ip_addrs().count(), 1);
    }
}
