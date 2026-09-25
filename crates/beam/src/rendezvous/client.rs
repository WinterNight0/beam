//! Talking to the rendezvous server.

use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use futures_util::{SinkExt, StreamExt};
use iroh::EndpointAddr;
use tokio::net::TcpStream;
use tokio_websockets::{ClientBuilder, Limits, MaybeTlsStream, Message, WebSocketStream};

use super::proto::{
    ClientMessage, MAX_MESSAGE, ServerMessage, sign_registration, unix_now, verify_record,
};
use crate::identity::{Identity, ShortId};

/// How long to wait for the server to connect or answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a waiting device refreshes its registration. A third of the
/// server's 90-second lifetime, so two refreshes can be lost before the entry
/// lapses.
pub const REFRESH_EVERY: Duration = Duration::from_secs(30);

/// Why talking to the rendezvous server failed.
#[derive(Debug, thiserror::Error)]
pub enum RendezvousError {
    #[error(
        "cannot reach the rendezvous server at {url}: {message}\n       \
         Is beam-server running? The address is `rendezvous` in config.toml."
    )]
    Unreachable { url: String, message: String },
    #[error("the rendezvous server refused the request ({code}): {message}")]
    Refused { code: String, message: String },
    #[error("the rendezvous server did not answer in time")]
    Timeout,
    #[error("the rendezvous server closed the connection")]
    Closed,
    #[error("the rendezvous server sent something unexpected: {0}")]
    Protocol(String),
}

/// A device found by a lookup, already checked against the Short ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub public_key: VerifyingKey,
    pub addr: EndpointAddr,
}

/// An open connection to the rendezvous server.
pub struct RendezvousClient {
    url: String,
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl RendezvousClient {
    /// Connects to `url` (`ws://` or `wss://`).
    pub async fn connect(url: &str) -> Result<Self, RendezvousError> {
        let unreachable = |message: String| RendezvousError::Unreachable {
            url: url.to_string(),
            message,
        };
        let builder = ClientBuilder::new()
            .uri(url)
            .map_err(|e| unreachable(e.to_string()))?
            .limits(Limits::default().max_payload_len(Some(MAX_MESSAGE)));
        let (ws, _response) = tokio::time::timeout(REQUEST_TIMEOUT, builder.connect())
            .await
            .map_err(|_| unreachable("timed out".to_string()))?
            .map_err(|e| unreachable(e.to_string()))?;
        Ok(Self {
            url: url.to_string(),
            ws,
        })
    }

    /// The server's URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Announces that this device is reachable at `addr`. Signed with the
    /// device key; see [`super::proto::verify_registration`].
    ///
    /// Returns how long the registration lasts without a refresh.
    pub async fn register(
        &mut self,
        identity: &Identity,
        addr: &EndpointAddr,
    ) -> Result<Duration, RendezvousError> {
        let request = sign_registration(identity, addr, unix_now());
        match self.request(&request).await? {
            ServerMessage::Registered { ttl_secs } => Ok(Duration::from_secs(ttl_secs)),
            other => Err(unexpected(&other)),
        }
    }

    /// Every device currently registered under `short_id` whose entry checks
    /// out. Entries whose key does not derive `short_id`, or whose address is
    /// for another endpoint, are dropped here whatever the server says.
    pub async fn lookup(&mut self, short_id: ShortId) -> Result<Vec<Found>, RendezvousError> {
        let request = ClientMessage::Lookup {
            short_id: short_id.to_string(),
        };
        match self.request(&request).await? {
            ServerMessage::Found { peers, .. } => Ok(peers
                .iter()
                .filter_map(|record| verify_record(short_id, record))
                .map(|(public_key, addr)| Found { public_key, addr })
                .collect()),
            other => Err(unexpected(&other)),
        }
    }

    /// Closes the connection, which also removes this device's registration.
    pub async fn close(mut self) {
        let _ = self.ws.close().await;
    }

    async fn request(&mut self, request: &ClientMessage) -> Result<ServerMessage, RendezvousError> {
        let text = serde_json::to_string(request).expect("requests always serialise");
        self.ws
            .send(Message::text(text))
            .await
            .map_err(|_| RendezvousError::Closed)?;

        let reply = tokio::time::timeout(REQUEST_TIMEOUT, async {
            loop {
                match self.ws.next().await {
                    Some(Ok(message)) if message.is_close() => return Err(RendezvousError::Closed),
                    Some(Ok(message)) => {
                        if let Some(text) = message.as_text() {
                            return serde_json::from_str::<ServerMessage>(text)
                                .map_err(|e| RendezvousError::Protocol(e.to_string()));
                        }
                    }
                    Some(Err(_)) | None => return Err(RendezvousError::Closed),
                }
            }
        })
        .await
        .map_err(|_| RendezvousError::Timeout)??;

        match reply {
            ServerMessage::Error { code, message } => {
                Err(RendezvousError::Refused { code, message })
            }
            reply => Ok(reply),
        }
    }
}

fn unexpected(reply: &ServerMessage) -> RendezvousError {
    RendezvousError::Protocol(format!("unexpected reply {reply:?}"))
}
