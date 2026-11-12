//! Framing: one length-prefixed frame per message.
//!
//! ```text
//! [1 byte type][4 bytes u32 big-endian payload length][payload]
//! ```
//!
//! The declared length is checked against [`MAX_FRAME_PAYLOAD`] *before* any
//! buffer is allocated, so a peer announcing a four-gigabyte frame costs five
//! bytes of work rather than four gigabytes of memory.
//!
//! A frame is not a chunk: chunks are 4 MiB and are the unit of hashing, frames
//! are at most 64 KiB and are the unit of transmission. See ADR-0015.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::message::{
    Accept, Cancel, ChunkAck, ChunkNak, ChunkStart, Complete, Message, Reject, TransferRequest,
    VerifyProgress,
};

/// The largest payload any frame may declare.
pub const MAX_FRAME_PAYLOAD: usize = 64 * 1024;

/// The header is a type byte plus a four-byte length.
pub const HEADER_SIZE: usize = 5;

/// A `CHUNK_DATA` payload begins with the chunk index, so slightly fewer file
/// bytes fit in one frame than the payload limit suggests.
pub const MAX_CHUNK_DATA: usize = MAX_FRAME_PAYLOAD - 4;

mod kind {
    pub const TRANSFER_REQUEST: u8 = 1;
    pub const ACCEPT: u8 = 2;
    pub const REJECT: u8 = 3;
    pub const CHUNK_START: u8 = 4;
    pub const CHUNK_DATA: u8 = 5;
    pub const CHUNK_ACK: u8 = 6;
    pub const CHUNK_NAK: u8 = 7;
    pub const COMPLETE: u8 = 8;
    pub const CANCEL: u8 = 9;
    pub const VERIFYING: u8 = 10;
}

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The peer closed the connection cleanly, between frames.
    #[error("the peer closed the connection")]
    Closed,
    /// The connection ended part-way through a frame.
    #[error("the connection ended in the middle of a frame")]
    Truncated,
    #[error("unknown frame type {0}")]
    UnknownType(u8),
    #[error("frame declares {declared} bytes, limit is {MAX_FRAME_PAYLOAD}")]
    TooLarge { declared: usize },
    #[error("a CHUNK_DATA frame is too short to contain a chunk index")]
    MalformedChunkData,
    #[error("malformed {kind} payload: {}", crate::untrusted::text(&source.to_string()))]
    Json {
        kind: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("transport error: {0}")]
    Io(#[from] std::io::Error),
}

/// Encodes a message as a complete frame, header included.
pub fn encode(message: &Message) -> Result<Vec<u8>, FrameError> {
    let (kind, payload) = match message {
        Message::TransferRequest(m) => (kind::TRANSFER_REQUEST, to_json(m, "TRANSFER_REQUEST")?),
        Message::Accept(m) => (kind::ACCEPT, to_json(m, "ACCEPT")?),
        Message::Reject(m) => (kind::REJECT, to_json(m, "REJECT")?),
        Message::ChunkStart(m) => (kind::CHUNK_START, to_json(m, "CHUNK_START")?),
        Message::ChunkAck(m) => (kind::CHUNK_ACK, to_json(m, "CHUNK_ACK")?),
        Message::ChunkNak(m) => (kind::CHUNK_NAK, to_json(m, "CHUNK_NAK")?),
        Message::Complete(m) => (kind::COMPLETE, to_json(m, "COMPLETE")?),
        Message::Cancel(m) => (kind::CANCEL, to_json(m, "CANCEL")?),
        Message::Verifying(m) => (kind::VERIFYING, to_json(m, "VERIFYING")?),
        Message::ChunkData { index, bytes } => {
            let mut payload = Vec::with_capacity(4 + bytes.len());
            payload.extend_from_slice(&index.to_be_bytes());
            payload.extend_from_slice(bytes);
            (kind::CHUNK_DATA, payload)
        }
    };

    if payload.len() > MAX_FRAME_PAYLOAD {
        return Err(FrameError::TooLarge {
            declared: payload.len(),
        });
    }

    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decodes a frame payload that has already been read off the wire.
pub fn decode(kind: u8, payload: &[u8]) -> Result<Message, FrameError> {
    Ok(match kind {
        kind::TRANSFER_REQUEST => {
            Message::TransferRequest(from_json::<TransferRequest>(payload, "TRANSFER_REQUEST")?)
        }
        kind::ACCEPT => Message::Accept(from_json::<Accept>(payload, "ACCEPT")?),
        kind::REJECT => Message::Reject(from_json::<Reject>(payload, "REJECT")?),
        kind::CHUNK_START => Message::ChunkStart(from_json::<ChunkStart>(payload, "CHUNK_START")?),
        kind::CHUNK_ACK => Message::ChunkAck(from_json::<ChunkAck>(payload, "CHUNK_ACK")?),
        kind::CHUNK_NAK => Message::ChunkNak(from_json::<ChunkNak>(payload, "CHUNK_NAK")?),
        kind::COMPLETE => Message::Complete(from_json::<Complete>(payload, "COMPLETE")?),
        kind::CANCEL => Message::Cancel(from_json::<Cancel>(payload, "CANCEL")?),
        kind::VERIFYING => Message::Verifying(from_json::<VerifyProgress>(payload, "VERIFYING")?),
        kind::CHUNK_DATA => {
            if payload.len() < 4 {
                return Err(FrameError::MalformedChunkData);
            }
            let index = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Message::ChunkData {
                index,
                bytes: payload[4..].to_vec(),
            }
        }
        other => return Err(FrameError::UnknownType(other)),
    })
}

/// Writes one message as a frame.
pub async fn write_message<W>(writer: &mut W, message: &Message) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    let frame = encode(message)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one message.
///
/// Returns [`FrameError::Closed`] when the peer hangs up tidily at a frame
/// boundary, and [`FrameError::Truncated`] when it hangs up part-way through.
pub async fn read_message<R>(reader: &mut R) -> Result<Message, FrameError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; HEADER_SIZE];

    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::Closed);
        }
        Err(e) => return Err(FrameError::Io(e)),
    }

    let kind = header[0];
    let declared = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;

    // Checked before allocating, deliberately.
    if declared > MAX_FRAME_PAYLOAD {
        return Err(FrameError::TooLarge { declared });
    }

    let mut payload = vec![0u8; declared];
    match reader.read_exact(&mut payload).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::Truncated);
        }
        Err(e) => return Err(FrameError::Io(e)),
    }

    decode(kind, &payload)
}

fn to_json<T: serde::Serialize>(value: &T, kind: &'static str) -> Result<Vec<u8>, FrameError> {
    serde_json::to_vec(value).map_err(|source| FrameError::Json { kind, source })
}

fn from_json<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    kind: &'static str,
) -> Result<T, FrameError> {
    serde_json::from_slice(payload).map_err(|source| FrameError::Json { kind, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::message::{RejectReason, TransferId};

    fn sample_id() -> TransferId {
        TransferId::from_bytes([7u8; 16])
    }

    fn samples() -> Vec<Message> {
        vec![
            Message::TransferRequest(TransferRequest {
                transfer_id: sample_id(),
                sender_public_key: "key".to_string(),
                file_name: "a.txt".to_string(),
                size: 10,
                chunk_size: 4,
                chunk_count: 3,
                file_sha256: "a".repeat(64),
            }),
            Message::Accept(Accept {
                transfer_id: sample_id(),
                have_bitmap: None,
            }),
            Message::Reject(Reject {
                transfer_id: sample_id(),
                reason: RejectReason::Declined,
            }),
            Message::ChunkStart(ChunkStart {
                index: 2,
                len: 9,
                sha256: "b".repeat(64),
            }),
            Message::ChunkData {
                index: 2,
                bytes: vec![1, 2, 3],
            },
            Message::ChunkAck(ChunkAck { index: 2 }),
            Message::ChunkNak(ChunkNak {
                index: 2,
                reason: crate::transfer::message::NakReason::HashMismatch,
            }),
            Message::Complete(Complete {
                transfer_id: sample_id(),
                final_name: Some("a (1).txt".to_string()),
            }),
            Message::Cancel(Cancel {
                transfer_id: sample_id(),
                reason: "user cancelled".to_string(),
            }),
        ]
    }

    #[tokio::test]
    async fn every_message_round_trips() {
        for message in samples() {
            let mut buffer = Vec::new();
            write_message(&mut buffer, &message).await.expect("write");

            let mut cursor = buffer.as_slice();
            let back = read_message(&mut cursor).await.expect("read");
            assert_eq!(back, message, "{}", message.kind_name());
        }
    }

    #[tokio::test]
    async fn messages_can_be_streamed_back_to_back() {
        let mut buffer = Vec::new();
        for message in samples() {
            write_message(&mut buffer, &message).await.expect("write");
        }
        let mut cursor = buffer.as_slice();
        for message in samples() {
            assert_eq!(read_message(&mut cursor).await.expect("read"), message);
        }
        assert!(matches!(
            read_message(&mut cursor).await,
            Err(FrameError::Closed)
        ));
    }

    #[tokio::test]
    async fn an_empty_stream_reads_as_a_clean_close() {
        let mut empty: &[u8] = &[];
        assert!(matches!(
            read_message(&mut empty).await,
            Err(FrameError::Closed)
        ));
    }

    #[tokio::test]
    async fn a_half_written_frame_reads_as_truncated() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &samples()[0])
            .await
            .expect("write");
        buffer.truncate(buffer.len() - 1);

        let mut cursor = buffer.as_slice();
        assert!(matches!(
            read_message(&mut cursor).await,
            Err(FrameError::Truncated)
        ));
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused_without_allocating() {
        // A header claiming 4 GiB, and nothing behind it. If the limit were
        // checked after allocation this test would try to reserve 4 GiB.
        let header = [kind::CHUNK_DATA, 0xff, 0xff, 0xff, 0xff];
        let mut cursor: &[u8] = &header;
        match read_message(&mut cursor).await {
            Err(FrameError::TooLarge { declared }) => {
                assert_eq!(declared, u32::MAX as usize);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_frame_exactly_at_the_limit_is_accepted() {
        let message = Message::ChunkData {
            index: 0,
            bytes: vec![0xab; MAX_CHUNK_DATA],
        };
        let frame = encode(&message).expect("encode");
        assert_eq!(frame.len(), HEADER_SIZE + MAX_FRAME_PAYLOAD);

        let mut cursor = frame.as_slice();
        assert_eq!(read_message(&mut cursor).await.expect("read"), message);
    }

    #[test]
    fn encoding_refuses_to_produce_an_oversized_frame() {
        let message = Message::ChunkData {
            index: 0,
            bytes: vec![0; MAX_CHUNK_DATA + 1],
        };
        assert!(matches!(encode(&message), Err(FrameError::TooLarge { .. })));
    }

    #[test]
    fn unknown_frame_types_are_refused() {
        assert!(matches!(
            decode(200, b"{}"),
            Err(FrameError::UnknownType(200))
        ));
    }

    #[test]
    fn a_short_chunk_data_payload_is_refused() {
        assert!(matches!(
            decode(kind::CHUNK_DATA, &[1, 2, 3]),
            Err(FrameError::MalformedChunkData)
        ));
    }

    #[test]
    fn malformed_json_is_refused() {
        assert!(matches!(
            decode(kind::ACCEPT, b"not json"),
            Err(FrameError::Json { .. })
        ));
    }
}
