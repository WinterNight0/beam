//! Reaching a paired peer, and knowing which path the connection is on.
//!
//! * [`dial`] connects to a peer at the address beam saved for it: its **full
//!   public key**, the relay it is reachable through, and any direct
//!   addresses its invite named (ADR-0036). iroh dials by endpoint id, so the
//!   TLS handshake only completes against the holder of that key; the
//!   connection's `remote_id()` is checked against it anyway before anything
//!   is sent (ADR-0031).
//! * [`watch_route`] follows the connection's selected path and publishes
//!   `[Direct P2P]` or `[Relay]` for the progress line, updating it when iroh
//!   moves the connection (F-11, ADR-0032).

use std::time::Duration;

use futures_util::StreamExt;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr};

use super::endpoint::verifying_key;
use super::{PathKind, Route};

/// How long to give a peer to answer a connection attempt.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a peer could not be reached.
#[derive(Debug, thiserror::Error)]
pub enum DialError {
    /// No relay is configured and no direct address was saved: there is
    /// nowhere to send the first packet.
    #[error(
        "beam does not know where to find that device: no relay is set and no address was saved"
    )]
    NoAddress,
    /// Nobody holding the key answered. The peer is not running `beam
    /// listen`, has moved, or ran `beam init` again and now has a different
    /// key. The caller words the message; all of these look the same here.
    #[error("{0}")]
    Unreachable(String),
    /// Should be impossible: iroh only completes a handshake with the key it
    /// dialled. Checked because this is the key the transfer trusts.
    #[error("the device that answered does not hold the paired key")]
    WrongKey,
}

/// Connects to `addr` on `alpn`, and checks the connection proved `addr`'s key.
pub async fn dial(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    alpn: &[u8],
) -> Result<Connection, DialError> {
    if addr.is_empty() {
        return Err(DialError::NoAddress);
    }
    let key = verifying_key(&addr.id);
    let connection = tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(addr, alpn))
        .await
        .map_err(|_| DialError::Unreachable("timed out".into()))?
        .map_err(|e| DialError::Unreachable(e.to_string()))?;

    if verifying_key(&connection.remote_id()) != key {
        connection.close(1u32.into(), b"wrong key");
        return Err(DialError::WrongKey);
    }
    Ok(connection)
}

/// Publishes the connection's current path, and keeps it current until the
/// connection closes.
pub fn watch_route(connection: &Connection) -> Route {
    let initial = selected_kind(connection).unwrap_or_default();
    let (tx, rx) = tokio::sync::watch::channel(initial);
    let connection = connection.clone();
    tokio::spawn(async move {
        let mut paths = connection.paths_stream();
        while let Some(list) = paths.next().await {
            let kind = list.iter().find(|p| p.is_selected()).map(|p| {
                if p.is_relay() {
                    PathKind::Relay
                } else {
                    PathKind::Direct
                }
            });
            if let Some(kind) = kind {
                tx.send_if_modified(|current| std::mem::replace(current, kind) != kind);
            }
            if tx.is_closed() {
                break;
            }
        }
    });
    rx
}

fn selected_kind(connection: &Connection) -> Option<PathKind> {
    let paths = connection.paths();
    paths.iter().find(|p| p.is_selected()).map(|p| {
        if p.is_relay() {
            PathKind::Relay
        } else {
            PathKind::Direct
        }
    })
}

/// Sends one file on an open connection: opens the stream, follows the path
/// for the progress line, and closes cleanly once the receiver has answered.
pub async fn send_on<R: crate::transfer::Reporter>(
    connection: &Connection,
    options: &mut crate::transfer::SendOptions,
    reporter: &mut R,
) -> Result<crate::transfer::SendSummary, crate::transfer::TransferError> {
    let (send, recv) = connection.open_bi().await.map_err(|e| {
        crate::transfer::TransferError::io("open a stream to", "the peer", std::io::Error::other(e))
    })?;
    options.route = watch_route(connection);
    let mut stream = tokio::io::join(recv, send);
    let result = crate::transfer::send_file(&mut stream, options, reporter).await;

    // Make sure our last frame is delivered before the connection goes away.
    let (_recv, mut send) = stream.into_inner();
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(3), send.stopped()).await;
    connection.close(0u32.into(), b"done");
    result
}
