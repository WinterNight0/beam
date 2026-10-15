//! Byte-stream transports.
//!
//! The transfer engine is generic over `AsyncRead + AsyncWrite`, so this module
//! only has to hand it a stream. M2 provides TCP; M5 replaces it with an iroh
//! QUIC stream without the engine noticing. See ADR-0016 and ADR-0025.
//!
//! [`endpoint`] builds the iroh endpoint; [`dial`] reaches a paired peer by
//! its key and follows which path the connection is on.

pub mod dial;
pub mod endpoint;

use std::net::SocketAddr;

/// Where the progress line learns the current path. The transport holds the
/// sending half and updates it whenever the connection's selected path
/// changes; the engine only reads.
pub type Route = tokio::sync::watch::Receiver<PathKind>;

/// A route that never changes. The TCP stand-in and the tests use it.
pub fn fixed_route(kind: PathKind) -> Route {
    // The sender is dropped at once; a receiver keeps the last value.
    tokio::sync::watch::channel(kind).1
}

/// Follows a route and says when it has changed since it was last asked.
#[derive(Debug)]
pub struct RouteTracker {
    route: Route,
    last: PathKind,
}

impl RouteTracker {
    pub fn new(route: &Route) -> Self {
        let route = route.clone();
        let last = *route.borrow();
        Self { route, last }
    }

    /// The path now, and the path before it if it changed since the last call.
    pub fn poll(&mut self) -> (PathKind, Option<PathKind>) {
        let now = *self.route.borrow();
        if now == self.last {
            (now, None)
        } else {
            let before = std::mem::replace(&mut self.last, now);
            (now, Some(before))
        }
    }
}

/// How the two peers are connected, for the progress line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PathKind {
    /// A direct connection between the peers.
    #[default]
    Direct,
    /// Traffic is being relayed through a third party. Still encrypted end
    /// to end; the relay moves ciphertext.
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
    fn a_route_tracker_reports_each_change_once() {
        let (tx, rx) = tokio::sync::watch::channel(PathKind::Relay);
        let mut tracker = RouteTracker::new(&rx);
        assert_eq!(tracker.poll(), (PathKind::Relay, None));

        tx.send(PathKind::Direct).unwrap();
        assert_eq!(tracker.poll(), (PathKind::Direct, Some(PathKind::Relay)));
        assert_eq!(tracker.poll(), (PathKind::Direct, None));

        tx.send(PathKind::Relay).unwrap();
        assert_eq!(tracker.poll(), (PathKind::Relay, Some(PathKind::Direct)));
    }

    #[test]
    fn a_fixed_route_keeps_its_value() {
        let mut tracker = RouteTracker::new(&fixed_route(PathKind::Relay));
        assert_eq!(tracker.poll(), (PathKind::Relay, None));
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback(&"127.0.0.1:7777".parse().unwrap()));
        assert!(is_loopback(&"[::1]:7777".parse().unwrap()));
        assert!(!is_loopback(&"0.0.0.0:7777".parse().unwrap()));
        assert!(!is_loopback(&"192.168.1.10:7777".parse().unwrap()));
    }
}
