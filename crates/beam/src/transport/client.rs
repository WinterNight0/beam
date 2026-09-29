//! Outgoing direct TCP connections.

use std::net::SocketAddr;

use anyhow::Context;
use tokio::net::TcpStream;

pub async fn connect(target: SocketAddr) -> anyhow::Result<TcpStream> {
    let stream = TcpStream::connect(target)
        .await
        .with_context(|| format!("failed to connect to Beam peer at {target}"))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}
