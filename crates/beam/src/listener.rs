//! Public entry point for the foreground Beam daemon.

use std::path::PathBuf;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use crate::identity::Store;
use crate::transfer::{DEFAULT_ACCEPT_TIMEOUT, DEFAULT_MAX_AGE, ReceiveOptions, Reporter};
use crate::transport::daemon::Daemon;

#[derive(Clone, Debug)]
pub struct ListenOptions {
    pub bind: SocketAddr,
    pub out_dir: PathBuf,
    pub accept_timeout: Duration,
}

impl Default for ListenOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), crate::config::DEFAULT_PORT),
            out_dir: PathBuf::from("."),
            accept_timeout: DEFAULT_ACCEPT_TIMEOUT,
        }
    }
}

pub async fn run<Q, R, F>(
    store: Store,
    options: ListenOptions,
    prompt: Q,
    reporter: F,
) -> anyhow::Result<()>
where
    Q: crate::transfer::Prompt + Clone + Send + Sync + 'static,
    R: Reporter + Send + 'static,
    F: Fn() -> R + Send + Sync + 'static,
{
    let receive = ReceiveOptions::new(&options.out_dir, store.tmp_path());
    let mut receive = receive;
    receive.accept_timeout = options.accept_timeout;
    receive.max_partial_age = DEFAULT_MAX_AGE;
    receive.accept_unpaired = true;
    let _ = receive.partials().sweep_expired(DEFAULT_MAX_AGE);
    Daemon::new(options.bind).run::<Q, R, F>(receive, prompt, reporter).await
}
