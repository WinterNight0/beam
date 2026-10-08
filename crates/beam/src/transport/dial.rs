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
use iroh::endpoint::{ConnectOptions, Connection, ConnectionError, VarInt};
use iroh::{Endpoint, EndpointAddr};

use super::endpoint::{XFER_ALPN, XFER_ALPN_V2, verifying_key};
use super::{PathKind, Route};
use crate::transfer::PIPELINE_WINDOW;

/// How long to give a peer to answer a connection attempt.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// The QUIC close code beam uses when the person on this side stopped it
/// (Ctrl+C) mid-transfer. 0 is a normal end and 1 an error. The peer learns
/// it from the CONNECTION_CLOSE frame at once, instead of noticing silence
/// after the 15 s idle timeout (ADR-0041).
pub const CLOSE_INTERRUPTED: u32 = 2;

/// Closes `connection` saying that this side was stopped by its user.
pub fn interrupt(connection: &Connection) {
    connection.close(CLOSE_INTERRUPTED.into(), b"interrupted");
}

/// Whether the *peer* closed `connection` because its user stopped beam.
///
/// Read from the connection's close, which says which side closed it: a close
/// we made ourselves is `LocallyClosed`, never this. So the other side cannot
/// be blamed for our own interrupt, and the code is the peer's only claim.
pub fn peer_interrupted(connection: &Connection) -> bool {
    matches!(
        connection.close_reason(),
        Some(ConnectionError::ApplicationClosed(close))
            if close.error_code == VarInt::from_u32(CLOSE_INTERRUPTED)
    )
}

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
    dial_offering(endpoint, addr, alpn, &[]).await
}

/// Connects to `addr` for a file transfer, offering `beam/xfer/2` and
/// `beam/xfer/1`. The receiver picks; [`send_on`] reads which it picked.
pub async fn dial_transfer(
    endpoint: &Endpoint,
    addr: EndpointAddr,
) -> Result<Connection, DialError> {
    dial_offering(endpoint, addr, XFER_ALPN_V2, &[XFER_ALPN]).await
}

async fn dial_offering(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    alpn: &[u8],
    fallbacks: &[&[u8]],
) -> Result<Connection, DialError> {
    if addr.is_empty() {
        return Err(DialError::NoAddress);
    }
    let key = verifying_key(&addr.id);
    let options =
        ConnectOptions::new().with_additional_alpns(fallbacks.iter().map(|a| a.to_vec()).collect());
    let connect = async {
        endpoint
            .connect_with_opts(addr, alpn, options)
            .await
            .map_err(|e| DialError::Unreachable(e.to_string()))?
            .await
            .map_err(|e| DialError::Unreachable(e.to_string()))
    };
    let connection = tokio::time::timeout(DIAL_TIMEOUT, connect)
        .await
        .map_err(|_| DialError::Unreachable("timed out".into()))??;

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

/// How many chunks may be in flight on a connection, from the transfer
/// protocol its receiver agreed to: several for `beam/xfer/2`, one for the
/// original `beam/xfer/1`.
pub fn window_for(alpn: &[u8]) -> u32 {
    if alpn == XFER_ALPN_V2 {
        PIPELINE_WINDOW
    } else {
        1
    }
}

/// Sends one file on an open connection: opens the stream, follows the path
/// for the progress line, and closes cleanly once the receiver has answered.
/// The chunk window follows the protocol the receiver agreed to.
pub async fn send_on<R: crate::transfer::Reporter>(
    connection: &Connection,
    options: &mut crate::transfer::SendOptions,
    reporter: &mut R,
) -> Result<crate::transfer::SendSummary, crate::transfer::TransferError> {
    let (send, recv) = connection.open_bi().await.map_err(|e| {
        crate::transfer::TransferError::io("open a stream to", "the peer", std::io::Error::other(e))
    })?;
    options.route = watch_route(connection);
    options.window = window_for(connection.alpn());
    let mut stream = tokio::io::join(recv, send);
    // Raced against the connection closing, so a receiver that stops is
    // noticed at once, even while this side is still hashing a large file
    // and not yet using the network (ADR-0041).
    let result = tokio::select! {
        result = crate::transfer::send_file(&mut stream, options, reporter) => result,
        reason = connection.closed() => Err(crate::transfer::TransferError::io(
            "keep talking to",
            "the peer",
            std::io::Error::other(reason),
        )),
    };
    // A failure caused by the receiver's user stopping beam says so.
    let result = match result {
        Err(_) if peer_interrupted(connection) => {
            Err(crate::transfer::TransferError::PeerInterrupted)
        }
        other => other,
    };

    // Make sure our last frame is delivered before the connection goes away.
    let (_recv, mut send) = stream.into_inner();
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(3), send.stopped()).await;
    connection.close(0u32.into(), b"done");
    result
}
