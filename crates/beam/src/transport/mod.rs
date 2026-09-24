//! Byte-stream transports.
//!
//! The transfer engine is generic over `AsyncRead + AsyncWrite`, so this module
//! only has to hand it a stream. M2 provides TCP; M5 replaces it with a WebRTC
//! data channel without the engine noticing. See ADR-0016.

use std::net::SocketAddr;

/// How the two peers are connected, for the progress line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PathKind {
    /// A direct connection between the peers.
    #[default]
    Direct,
    /// Traffic is being relayed through a third party (M7).
    Relay,
}

impl PathKind {
    /// The tag shown in the progress line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Direct => "[Direct P2P]",
            Self::Relay => "[Relay]",
        }
    }
}

/// Whether an address keeps traffic on this machine.
///
/// M2 has neither encryption nor proven identity (ADR-0019), so binding
/// anywhere else deserves a warning.
pub fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_kind_labels() {
        assert_eq!(PathKind::Direct.label(), "[Direct P2P]");
        assert_eq!(PathKind::Relay.label(), "[Relay]");
        assert_eq!(PathKind::default(), PathKind::Direct);
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback(&"127.0.0.1:7777".parse().unwrap()));
        assert!(is_loopback(&"[::1]:7777".parse().unwrap()));
        assert!(!is_loopback(&"0.0.0.0:7777".parse().unwrap()));
        assert!(!is_loopback(&"192.168.1.10:7777".parse().unwrap()));
    }
}
