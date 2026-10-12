//! Sending one file to a friend: the whole of `beam send`, and of the
//! view's Send, written once.
//!
//! Nothing about the transfer is decided here: the engine still sends no
//! byte before the friend's Accept, and the transport still proves the
//! friend's key. This is the part around it: which friend, from which
//! address, how long to wait for their answer, what happens when the
//! person stops it, and the history line.
//!
//! The front end shows progress (a [`Reporter`]), says how far it got
//! ([`Step`]), and words the result. Every message that can quote the
//! network comes back already cleaned (see [`super::messages`]).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::config::Config;
use crate::history;
use crate::identity::{Fingerprint, PeerError, Store, StoreError, encode_public_key};
use crate::invite;
use crate::transfer::{Reporter, SendOptions, SendSummary};
use crate::transport::dial::{dial_transfer, interrupt, send_on};
use crate::transport::endpoint::{self, Bind};

use super::messages;

/// What to send, and to whom.
#[derive(Clone, Copy, Debug)]
pub struct SendRequest<'a> {
    /// The friend's nickname, as in `known_peers`.
    pub peer: &'a str,
    pub file: &'a Path,
    /// Bytes per chunk; `None` for the default (4 MiB).
    pub chunk_size: Option<u32>,
    /// `Loopback` only in tests.
    pub bind: Bind,
}

/// How far a send has got. `peer` is the friend's name as stored, which
/// may differ in case from what was typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// Looking for the friend.
    Looking { peer: &'a str },
    /// Connected; asking them, then sending.
    Sending { peer: &'a str },
}

/// A send that arrived.
#[derive(Clone, Debug)]
pub struct Sent {
    /// The friend's nickname, as stored.
    pub peer: String,
    /// The file's own name, as sent.
    pub file_name: String,
    pub summary: SendSummary,
}

/// Why a send did not arrive.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No friend has that name.
    #[error(transparent)]
    Peer(#[from] PeerError),
    #[error("{0}")]
    Config(String),
    #[error("{} is not a file", .0.display())]
    NotAFile(PathBuf),
    #[error("the chunk size must be at least 1")]
    ZeroChunkSize,
    #[error("{0}")]
    Endpoint(String),
    /// The friend could not be reached; worded and cleaned.
    #[error("{0}")]
    Unreachable(String),
    /// The friend refused, or the transfer failed; worded and cleaned.
    #[error("{0}")]
    Refused(String),
    /// The person stopped it. If the friend had been `reached`, they were
    /// told, and keep what arrived for a resume (ADR-0041).
    #[error("stopped")]
    Stopped { reached: bool },
}

/// How long a sender waits for the answer: past the longest a receiver
/// keeps its question open (the background agent's five minutes,
/// ADR-0042). Each receiver says "expired" itself; this only has to outlast
/// them.
pub fn accept_timeout() -> Duration {
    crate::agent::AGENT_ACCEPT_TIMEOUT + Duration::from_secs(15)
}

/// Sends `request.file` to `request.peer`, until it ends or `stop`
/// completes. Writes the history line for anything that got as far as
/// looking for the friend.
pub async fn send<R: Reporter>(
    store: &Store,
    request: SendRequest<'_>,
    reporter: &mut R,
    mut step: impl FnMut(Step<'_>),
    stop: impl Future<Output = ()>,
) -> Result<Sent, SendError> {
    let identity = store.load_identity()?;
    let known = store.load_known_peers()?;
    let peer = known
        .lookup(request.peer)
        .ok_or_else(|| PeerError::NotFound(request.peer.to_string()))?;
    let (peer_name, peer_key) = (peer.name.clone(), peer.public_key);
    let config =
        Config::load(&store.config_path()).map_err(|e| SendError::Config(e.to_string()))?;
    let peer_addr = invite::peer_addr(peer, &config.relay);
    if !request.file.is_file() {
        return Err(SendError::NotAFile(request.file.to_path_buf()));
    }
    let mut options = SendOptions::new(request.file, encode_public_key(&identity.verifying_key()));
    options.accept_timeout = accept_timeout();
    if let Some(chunk_size) = request.chunk_size {
        if chunk_size == 0 {
            return Err(SendError::ZeroChunkSize);
        }
        options.chunk_size = chunk_size;
    }
    let size = std::fs::metadata(request.file).map_or(0, |m| m.len());
    let file_name = request
        .file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    step(Step::Looking { peer: &peer_name });
    let endpoint = endpoint::bind(&identity, &config.relay, request.bind, 0, &[])
        .await
        .map_err(|e| SendError::Endpoint(e.to_string()))?;
    // How it ended, for `history.jsonl` (ADR-0043), and the connection once
    // there is one, so a stop can tell the friend.
    let ended = Mutex::new(None);
    let live = Mutex::new(None);
    let attempt = async {
        let connection = dial_transfer(&endpoint, peer_addr).await.map_err(|e| {
            *ended.lock().expect("not poisoned") =
                Some((history::Outcome::Failed, Some("not reachable".to_string())));
            SendError::Unreachable(messages::unreachable(&peer_name, &peer_key, e))
        })?;
        *live.lock().expect("not poisoned") = Some(connection.clone());
        step(Step::Sending { peer: &peer_name });
        let sent = send_on(&connection, &mut options, reporter).await;
        *ended.lock().expect("not poisoned") = Some(history::send_outcome(&sent));
        sent.map_err(|e| SendError::Refused(messages::refused(&peer_name, e)))
    };
    let outcome = tokio::select! {
        outcome = attempt => outcome,
        () = stop => {
            let connection = live.lock().expect("not poisoned").take();
            if let Some(connection) = &connection {
                interrupt(connection);
            }
            *ended.lock().expect("not poisoned") =
                Some(history::stopped_outcome(connection.is_some()));
            Err(SendError::Stopped { reached: connection.is_some() })
        }
    };
    endpoint.close().await;

    if let Some((outcome, note)) = ended.into_inner().expect("not poisoned") {
        history::record(
            store,
            &history::Entry::now(
                history::Direction::Sent,
                &peer_name,
                &Fingerprint::of(&peer_key).hex(),
                &file_name,
                size,
                outcome,
                note,
            ),
        );
    }
    outcome.map(|summary| Sent {
        peer: peer_name,
        file_name,
        summary,
    })
}
