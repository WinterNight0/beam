//! How `beam inbox` talks to the background agent (ADR-0042).
//!
//! One JSON message per line over TCP on 127.0.0.1. Only the same user may
//! use it, because answering here *is* the Accept (rule 1):
//!
//! * the agent listens on loopback only, so nothing off this machine can
//!   connect;
//! * the first line must carry the token from `agent.json`, which only this
//!   user can read (0600 on Unix, the user profile's permissions on Windows).
//!   Until then the agent says nothing: no pending request, no file names;
//! * the token is 32 random bytes, compared in constant time;
//! * a client gets 5 s to present it, a line may be at most 64 KiB, and at
//!   most 8 clients may be connected at once.
//!
//! Loopback TCP rather than a Unix socket or a Windows named pipe: one code
//! path for every platform, no `unsafe` (a named pipe restricted to one user
//! needs a raw security descriptor), and the token gives the same "this user
//! only" property. Someone who can read `agent.json` is this user, and can
//! already read the device's private key (SECURITY.md).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::transfer::{PromptRequest, ResumeInfo};

/// How long a client has to present the token.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest line either side accepts.
pub const MAX_LINE: u64 = 64 * 1024;
/// How many clients may be connected at once.
pub const MAX_CLIENTS: usize = 8;

/// From `beam inbox` (or `beam service`) to the agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMsg {
    /// The first line: proves the client is this user.
    Hello { token: String },
    /// Accept or decline request `id`.
    Answer { id: u64, accept: bool },
    /// Stop the agent cleanly (`beam service stop`).
    Stop,
}

/// From the agent to its clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMsg {
    /// Sent once the token checks out.
    Welcome {
        receive_dir: String,
        port_mapping: bool,
    },
    /// A paired device wants to send a file; nobody has answered yet.
    Request {
        id: u64,
        request: RequestInfo,
        /// Seconds left before it counts as refused.
        expires_in: u64,
    },
    /// Request `id` is no longer waiting: answered, expired, or withdrawn.
    Closed { id: u64 },
    /// An accepted transfer is moving.
    Progress { done: u64, total: u64, relay: bool },
    /// A transfer ended. `ok` says whether the file was saved.
    Finished { ok: bool, text: String },
    /// The answer to request `id` came too late to count.
    TooLate { id: u64 },
    /// The agent is stopping.
    Stopping,
}

/// A transfer request as the inbox shows it: the same fields as the
/// terminal prompt (S-6). Strings are the peer's or the local nickname, and
/// the inbox shows them through `untrusted`, as `listen` does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestInfo {
    pub peer_name: String,
    pub fingerprint: String,
    pub file_name: String,
    pub size: u64,
    pub resume: Option<ResumeWire>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeWire {
    pub have_bytes: u64,
    pub have_chunks: u32,
    pub chunk_count: u32,
    pub age_secs: Option<u64>,
}

impl From<&PromptRequest> for RequestInfo {
    fn from(r: &PromptRequest) -> Self {
        Self {
            peer_name: r.peer_name.clone(),
            fingerprint: r.fingerprint.clone(),
            file_name: r.file_name.clone(),
            size: r.size,
            resume: r.resume.as_ref().map(|x| ResumeWire {
                have_bytes: x.have_bytes,
                have_chunks: x.have_chunks,
                chunk_count: x.chunk_count,
                age_secs: x.age.map(|a| a.as_secs()),
            }),
        }
    }
}

impl From<RequestInfo> for PromptRequest {
    fn from(r: RequestInfo) -> Self {
        Self {
            peer_name: r.peer_name,
            fingerprint: r.fingerprint,
            file_name: r.file_name,
            size: r.size,
            resume: r.resume.map(|x| ResumeInfo {
                have_bytes: x.have_bytes,
                have_chunks: x.have_chunks,
                chunk_count: x.chunk_count,
                age: x.age_secs.map(Duration::from_secs),
            }),
        }
    }
}

/// Why talking to the agent failed.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("the agent closed the connection")]
    Closed,
    #[error("a message was longer than {MAX_LINE} bytes")]
    TooLong,
    #[error("unreadable message: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A fresh token: 32 random bytes, hex.
pub fn new_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(crate::hex::encode(&bytes))
}

/// Compares two tokens in time that does not depend on where they differ.
pub fn same_token(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads one line of at most [`MAX_LINE`] bytes and parses it.
pub async fn read_msg<R, T>(reader: &mut BufReader<R>) -> Result<T, IpcError>
where
    R: tokio::io::AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut line = String::new();
    let n = (&mut *reader)
        .take(MAX_LINE + 1)
        .read_line(&mut line)
        .await?;
    if n == 0 {
        return Err(IpcError::Closed);
    }
    if n as u64 > MAX_LINE {
        return Err(IpcError::TooLong);
    }
    Ok(serde_json::from_str(line.trim_end())?)
}

/// Writes one message as one line.
pub async fn write_msg<W, T>(writer: &mut W, msg: &T) -> Result<(), IpcError>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: Serialize,
{
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

/// A client connection to the agent, past the token check.
pub struct Client {
    pub reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    pub writer: tokio::net::tcp::OwnedWriteHalf,
}

impl Client {
    /// Connects to the agent at `port` and presents `token`. Returns the
    /// agent's welcome.
    pub async fn connect(port: u16, token: &str) -> Result<(Self, AgentMsg), IpcError> {
        let stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let (read, mut writer) = stream.into_split();
        let mut reader = BufReader::new(read);
        write_msg(
            &mut writer,
            &ClientMsg::Hello {
                token: token.to_string(),
            },
        )
        .await?;
        let welcome = tokio::time::timeout(HELLO_TIMEOUT, read_msg(&mut reader))
            .await
            .map_err(|_| IpcError::Closed)??;
        Ok((Self { reader, writer }, welcome))
    }

    pub async fn send(&mut self, msg: &ClientMsg) -> Result<(), IpcError> {
        write_msg(&mut self.writer, msg).await
    }

    pub async fn next(&mut self) -> Result<AgentMsg, IpcError> {
        read_msg(&mut self.reader).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_compared_exactly() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(same_token(&a, &a.clone()));
        assert!(!same_token(&a, &b));
        assert!(!same_token(&a, &a[..63]));
        assert!(!same_token("", "x"));
    }

    #[test]
    fn messages_round_trip_and_unknown_fields_are_refused() {
        let msg = ClientMsg::Answer {
            id: 3,
            accept: true,
        };
        let text = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<ClientMsg>(&text).unwrap(), msg);
        assert!(
            serde_json::from_str::<ClientMsg>(r#"{"cmd":"answer","id":3,"accept":true,"x":1}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<ClientMsg>(r#"{"cmd":"auto_accept"}"#).is_err());
    }

    #[tokio::test]
    async fn an_overlong_line_is_refused_without_reading_it_all() {
        let line = format!("{}\n", "a".repeat(MAX_LINE as usize + 10));
        let mut reader = BufReader::new(line.as_bytes());
        let got: Result<ClientMsg, _> = read_msg(&mut reader).await;
        assert!(matches!(got, Err(IpcError::TooLong)), "{got:?}");
    }
}
