//! The beam rendezvous server.
//!
//! It maps a Short ID to an iroh endpoint address, for as long as a device is
//! waiting to pair, so that `beam pair <ID>` can find it. It never sees a file,
//! a pairing code, or a private key, and it keeps nothing on disk.
//!
//! The server is not trusted: registrations are signed by the device key, and
//! clients re-check every answer. See ADR-0027.

use std::net::SocketAddr;
use std::process::ExitCode;

use beam::rendezvous::{ServerConfig, serve};
use clap::Parser;
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(name = "beam-server", version, about = "The beam rendezvous server")]
struct Args {
    /// Address to listen on. Loopback by default; use 0.0.0.0:8787 to accept
    /// devices on other machines.
    #[arg(long, default_value = "127.0.0.1:8787", value_name = "HOST:PORT")]
    addr: SocketAddr,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let listener = match TcpListener::bind(args.addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("beam-server: cannot listen on {}: {e}", args.addr);
            return ExitCode::FAILURE;
        }
    };
    let bound = listener.local_addr().unwrap_or(args.addr);

    // What is printed here is all the server ever prints. It does not log
    // requests: a log line tying a Short ID to an IP address is exactly the
    // record a rendezvous server should not keep.
    println!(
        "beam-server listening on ws://{bound}{}",
        beam::rendezvous::proto::PATH
    );
    println!("Registrations live in memory only. Requests are not logged.");

    match serve(listener, ServerConfig::default()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("beam-server: {e}");
            ExitCode::FAILURE
        }
    }
}
