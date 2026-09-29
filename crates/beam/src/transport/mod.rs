//! Beam's Layer 3/4 network foundation.
//!
//! Every Beam installation is a node. The node keeps a TCP listener available
//! for incoming transfers and can also open an outgoing TCP connection when it
//! is the sender. The transfer engine above this module only sees an
//! `AsyncRead + AsyncWrite` stream.

use std::net::SocketAddr;

use tokio::sync::watch;

pub type Route = watch::Receiver<PathKind>;

pub fn fixed_route(kind: PathKind) -> Route {
    watch::channel(kind).1
}

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

    pub fn poll(&mut self) -> (PathKind, Option<PathKind>) {
        let now = *self.route.borrow();
        if now == self.last { (now, None) }
        else {
            let before = std::mem::replace(&mut self.last, now);
            (now, Some(before))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PathKind {
    #[default]
    Direct,
}

impl PathKind {
    pub fn label(self) -> &'static str { "[Direct P2P]" }
}

pub fn is_loopback(addr: &SocketAddr) -> bool { addr.ip().is_loopback() }

pub mod client;
pub mod daemon;
