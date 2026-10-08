//! The background agent: `beam listen` without a terminal (ADR-0042).
//!
//! It runs the same listener as `beam listen`, with three differences:
//!
//! * **Pairing is off.** Only devices already in `known_peers` can even ask,
//!   and the pairing protocol is not offered at all, so a device left
//!   listening all day cannot be used to try pairing codes (R-8).
//! * **Nobody is at a keyboard,** so a request waits — up to
//!   [`AGENT_ACCEPT_TIMEOUT`] — while the agent shows a desktop notification
//!   and `beam inbox`, in a terminal, shows the usual Accept prompt. The
//!   answer given there is the Accept (rule 1). Unanswered is refused.
//! * **Router port mapping is off** unless the person turned it on, after a
//!   warning (`beam service port-mapping on`).
//!
//! It runs as the user, never as a system service: a system service could
//! not show a notification, would write received files with system rights,
//! and, started from a folder the user can write to, would be a privilege
//! escalation waiting to happen.

pub mod ipc;
pub mod notify;
pub mod service;
pub mod status;

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::sync::{Notify, Semaphore, broadcast};

use crate::identity::{Fingerprint, Identity, Store};
use crate::listener::{ListenEvent, ListenOptions};
use crate::pairing::{Confirm, ConfirmRequest, Network, Policy, Timeouts};
use crate::transfer::{Progress, Prompt, PromptRequest, Reporter};
use crate::transport::PathKind;
use crate::{ui, untrusted};

use ipc::{AgentMsg, ClientMsg, HELLO_TIMEOUT, MAX_CLIENTS};
use status::{AgentLock, AgentStatus};

/// How long a request to the agent waits for an answer in `beam inbox`.
/// Longer than `listen`'s 60 s, because the person first has to notice the
/// notification and open a terminal.
pub const AGENT_ACCEPT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The fewest seconds between two notifications, so a paired device that
/// sends request after request cannot flood the desktop.
const NOTIFY_GAP: Duration = Duration::from_secs(10);

/// How often a transfer's progress is passed on to `beam inbox`.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

/// Where received files go when the person has not chosen (ADR-0042):
///
/// * Windows: the Downloads folder, asked of Windows itself (the Known Folder
///   API, through `dirs`), so a Downloads folder the person moved elsewhere
///   is found where it really is;
/// * elsewhere: the directory the terminal was in when the agent was started
///   (`beam service start` or `enable` starts it there).
pub fn default_receive_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(dir) = dirs::download_dir() {
            return dir;
        }
        if let Some(home) = dirs::home_dir() {
            return home.join("Downloads");
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Where the agent saves files: the setting, else the default. Also says
/// which it was.
pub fn receive_dir(setting: Option<&Path>) -> (PathBuf, bool) {
    match setting {
        Some(dir) => (dir.to_path_buf(), true),
        None => (default_receive_dir(), false),
    }
}

/// How the agent runs.
#[derive(Clone, Debug)]
pub struct AgentOptions {
    pub network: Network,
    pub receive_dir: PathBuf,
    pub accept_timeout: Duration,
    pub port_mapping: bool,
    /// Show desktop notifications. Off in tests.
    pub notify: bool,
    /// Also print what happens to stdout, for `beam agent` run in a terminal.
    pub echo: bool,
    /// Run by the full-screen view's Receiving switch rather than as the
    /// background agent: same rules, but it lives only while the view is
    /// open (ADR-0044).
    pub in_view: bool,
}

/// Why the agent could not run.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(
        "the background agent is already running (`beam service status` shows it; \
         `beam service stop` stops it)"
    )]
    AlreadyRunning,
    #[error(
        "`beam listen` is running in this beam home. Stop it (Ctrl+C) before starting the \
         background agent: both would receive on the same identity"
    )]
    ListenRunning,
    #[error("could not draw a token: {0}")]
    Random(getrandom::Error),
    #[error(transparent)]
    Store(#[from] crate::identity::StoreError),
    #[error(transparent)]
    Listen(#[from] crate::listener::ListenError),
    #[error("could not open the local port for `beam inbox`: {0}")]
    Ipc(std::io::Error),
}

/// A request waiting for an answer from `beam inbox`.
struct Pending {
    id: u64,
    info: ipc::RequestInfo,
    deadline: Instant,
    answer: std::sync::mpsc::Sender<bool>,
}

/// What the agent's parts share.
struct State {
    store: Store,
    token: String,
    receive_dir: String,
    port_mapping: bool,
    notify: bool,
    echo: bool,
    pending: Mutex<Option<Pending>>,
    next_id: AtomicU64,
    events: broadcast::Sender<AgentMsg>,
    stop: Notify,
    last_notice: Mutex<Option<Instant>>,
    /// What `agent.json` says, to update once the invite is known.
    status: Mutex<AgentStatus>,
}

impl State {
    fn broadcast(&self, msg: AgentMsg) {
        let _ = self.events.send(msg);
    }

    /// Writes a line to `agent.log`, and to stdout when run in a terminal.
    fn log(&self, line: &str) {
        let stamp = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let line = untrusted::text(line);
        if self.echo {
            println!("{line}");
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.store.agent_log_path())
        {
            let _ = writeln!(file, "{stamp} {line}");
        }
    }

    /// Ends whatever request is waiting: its asker is told "no" (if it is
    /// still asking) and every inbox stops showing it.
    fn close_pending(&self) {
        if let Some(pending) = self.pending.lock().expect("not poisoned").take() {
            self.broadcast(AgentMsg::Closed { id: pending.id });
        }
    }

    fn name_of(&self, fingerprint: &Fingerprint) -> String {
        self.store
            .load_known_peers()
            .ok()
            .and_then(|known| {
                known
                    .peers()
                    .into_iter()
                    .find(|p| p.fingerprint() == *fingerprint)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_else(|| fingerprint.short())
    }

    fn notify_request(&self, info: &ipc::RequestInfo) {
        if !self.notify {
            return;
        }
        {
            let mut last = self.last_notice.lock().expect("not poisoned");
            if last.is_some_and(|t| t.elapsed() < NOTIFY_GAP) {
                return;
            }
            *last = Some(Instant::now());
        }
        let body = format!(
            "{} wants to send {} ({}). Open a terminal and run: beam inbox (expires in {} min)",
            untrusted::name(&info.peer_name),
            untrusted::name(&info.file_name),
            ui::format_bytes(info.size),
            AGENT_ACCEPT_TIMEOUT.as_secs() / 60
        );
        notify::show("beam: incoming file", &body);
    }
}

/// The agent's prompt: hands the question to `beam inbox` and waits.
#[derive(Clone)]
struct InboxPrompt {
    state: Arc<State>,
    timeout: Duration,
}

impl Prompt for InboxPrompt {
    fn confirm(&mut self, request: &PromptRequest) -> std::io::Result<bool> {
        let id = self.state.next_id.fetch_add(1, Ordering::SeqCst);
        let info = ipc::RequestInfo::from(request);
        let (tx, rx) = std::sync::mpsc::channel();
        *self.state.pending.lock().expect("not poisoned") = Some(Pending {
            id,
            info: info.clone(),
            deadline: Instant::now() + self.timeout,
            answer: tx,
        });
        self.state.log(&format!(
            "request {id}: {} ({}) wants to send {} ({}); waiting for `beam inbox`",
            info.peer_name,
            info.fingerprint,
            info.file_name,
            ui::format_bytes(info.size)
        ));
        self.state.broadcast(AgentMsg::Request {
            id,
            request: info.clone(),
            expires_in: self.timeout.as_secs(),
        });
        self.state.notify_request(&info);

        // Silence is a no (S-5). A little longer than the listener's own
        // deadline, which answers "expired" first.
        let answer = rx
            .recv_timeout(self.timeout + Duration::from_secs(1))
            .unwrap_or(false);
        {
            let mut pending = self.state.pending.lock().expect("not poisoned");
            if pending.as_ref().is_some_and(|p| p.id == id) {
                *pending = None;
                self.state.broadcast(AgentMsg::Closed { id });
            }
        }
        self.state.log(&format!(
            "request {id}: {}",
            if answer { "accepted" } else { "not accepted" }
        ));
        Ok(answer)
    }
}

impl Confirm for InboxPrompt {
    /// Never reached: the agent does not offer pairing. Refuse regardless.
    fn confirm(&mut self, _request: &ConfirmRequest) -> std::io::Result<bool> {
        Ok(false)
    }
}

/// Passes a transfer's progress on to `beam inbox`, now and then.
struct InboxReporter {
    state: Arc<State>,
    last: Option<Instant>,
}

impl Reporter for InboxReporter {
    fn report(&mut self, progress: Progress) {
        if let Progress::Transferring { done, total, path } = progress {
            let now = Instant::now();
            if done < total && self.last.is_some_and(|t| now - t < PROGRESS_EVERY) {
                return;
            }
            self.last = Some(now);
            self.state.broadcast(AgentMsg::Progress {
                done,
                total,
                relay: path == PathKind::Relay,
            });
        }
    }
}

/// Runs the agent until `stop` completes or `beam service stop` asks.
pub async fn run(
    identity: Identity,
    store: Store,
    options: AgentOptions,
    stop: impl Future<Output = ()>,
) -> Result<(), AgentError> {
    let lock = match AgentLock::claim(&store) {
        Ok(lock) => lock,
        Err(status::ClaimError::Taken) => return Err(AgentError::AlreadyRunning),
        Err(status::ClaimError::Store(e)) => return Err(e.into()),
    };
    if matches!(
        crate::listen_status::read(&store),
        crate::listen_status::Listening::Yes(_) | crate::listen_status::Listening::Unreadable
    ) {
        return Err(AgentError::ListenRunning);
    }
    std::fs::create_dir_all(&options.receive_dir)
        .map_err(|e| crate::identity::StoreError::io("create", &options.receive_dir, e))?;

    let token = ipc::new_token().map_err(AgentError::Random)?;
    let ipc_listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(AgentError::Ipc)?;
    let port = ipc_listener.local_addr().map_err(AgentError::Ipc)?.port();

    let (events, _) = broadcast::channel(64);
    let published = AgentStatus {
        pid: std::process::id(),
        port,
        token: token.clone(),
        receive_dir: options.receive_dir.display().to_string(),
        port_mapping: options.port_mapping,
        started: crate::listen_status::unix(SystemTime::now()),
        invite: None,
        in_view: options.in_view,
    };
    let state = Arc::new(State {
        store: store.clone(),
        token: token.clone(),
        receive_dir: options.receive_dir.display().to_string(),
        port_mapping: options.port_mapping,
        notify: options.notify,
        echo: options.echo,
        pending: Mutex::new(None),
        next_id: AtomicU64::new(1),
        events,
        stop: Notify::new(),
        last_notice: Mutex::new(None),
        status: Mutex::new(published.clone()),
    });

    lock.publish(&published)?;
    state.log(&format!(
        "agent started: saving to {}; pairing off; router port mapping {}",
        state.receive_dir,
        if options.port_mapping { "on" } else { "off" }
    ));

    let server = tokio::spawn(serve(ipc_listener, Arc::clone(&state)));

    let listen_options = ListenOptions {
        out_dir: options.receive_dir.clone(),
        accept_timeout: options.accept_timeout,
        stall_timeout: crate::transfer::engine::DEFAULT_STALL_TIMEOUT,
        pairing: Policy::default(),
        timeouts: Timeouts::default(),
        allow_pairing: false,
        port_mapping: options.port_mapping,
    };
    let prompt = InboxPrompt {
        state: Arc::clone(&state),
        timeout: options.accept_timeout,
    };
    let reporting = Arc::clone(&state);
    let watching = Arc::clone(&state);
    let stopping = Arc::clone(&state);
    let result = crate::listener::run_until(
        identity,
        store,
        options.network,
        listen_options,
        prompt,
        move || InboxReporter {
            state: Arc::clone(&reporting),
            last: None,
        },
        move |event| on_event(&watching, event),
        async move {
            tokio::select! {
                () = stop => {}
                () = stopping.stop.notified() => {}
            }
        },
    )
    .await;

    state.broadcast(AgentMsg::Stopping);
    state.log("agent stopped");
    server.abort();
    drop(lock);
    result.map_err(Into::into)
}

/// Turns what the listener reports into log lines and inbox messages.
fn on_event(state: &State, event: ListenEvent) {
    match event {
        ListenEvent::Ready { invite, .. } => {
            state.log(&format!("listening as {}", Fingerprint::of(&invite.key)));
            let mut status = state.status.lock().expect("not poisoned");
            status.invite = Some(invite.to_string());
            let json = serde_json::to_string_pretty(&*status).expect("agent status serialises");
            if let Err(e) = state.store.save_agent_status(&json) {
                state.log(&format!("could not update agent.json: {e}"));
            }
        }
        ListenEvent::PortTaken { wanted, got } => state.log(&format!(
            "port {wanted} was taken; listening on {got}. Peers that saved the old \
             address reach this device through the relay"
        )),
        ListenEvent::TransferTurnedAway { peer } => state.log(&format!(
            "{} tried to send while another transfer was running; told to try later",
            state.name_of(&peer)
        )),
        ListenEvent::Received(summary) => {
            state.close_pending();
            let text = format!(
                "Received {} from {}, saved as {} in {}",
                ui::format_bytes(summary.bytes),
                untrusted::name(&summary.peer_name),
                untrusted::name(&summary.final_name),
                state.receive_dir
            );
            state.log(&text);
            state.broadcast(AgentMsg::Finished { ok: true, text });
        }
        ListenEvent::TransferFailed { peer, error } => {
            state.close_pending();
            let text = format!(
                "transfer from {} did not complete: {}",
                state.name_of(&peer),
                untrusted::text(&error)
            );
            state.log(&text);
            state.broadcast(AgentMsg::Finished { ok: false, text });
        }
        ListenEvent::TransferInterrupted { peer } => {
            state.close_pending();
            let who = state.name_of(&peer);
            let text = format!(
                "{who} stopped beam on their side (Ctrl+C), so the transfer was cancelled. \
                 What arrived is kept; it resumes if {who} sends the file again."
            );
            state.log(&text);
            state.broadcast(AgentMsg::Finished { ok: false, text });
        }
        ListenEvent::Stopped { cancelled } => {
            state.close_pending();
            if !cancelled.is_empty() {
                let names: Vec<String> = cancelled.iter().map(|p| state.name_of(p)).collect();
                state.log(&format!(
                    "stopping: cancelled the transfer from {}, and told them",
                    names.join(", ")
                ));
            }
        }
        // Pairing is off in the agent; code rotation still ticks inside the
        // listener, and is of no interest here.
        _ => {}
    }
}

/// Accepts `beam inbox` connections.
async fn serve(listener: TcpListener, state: Arc<State>) {
    let slots = Arc::new(Semaphore::new(MAX_CLIENTS));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            // Too many clients: refuse this one by closing it.
            continue;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = client(stream, &state).await;
            drop(permit);
        });
    }
}

/// Serves one client: the token first, then messages both ways.
async fn client(stream: tokio::net::TcpStream, state: &State) -> Result<(), ipc::IpcError> {
    let (read, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read);

    // Nothing is said until the token checks out.
    let hello = tokio::time::timeout(HELLO_TIMEOUT, ipc::read_msg::<_, ClientMsg>(&mut reader))
        .await
        .map_err(|_| ipc::IpcError::Closed)??;
    match hello {
        ClientMsg::Hello { token } if ipc::same_token(&token, &state.token) => {}
        _ => return Ok(()),
    }

    let mut events = state.events.subscribe();
    ipc::write_msg(
        &mut writer,
        &AgentMsg::Welcome {
            receive_dir: state.receive_dir.clone(),
            port_mapping: state.port_mapping,
        },
    )
    .await?;
    let waiting = state
        .pending
        .lock()
        .expect("not poisoned")
        .as_ref()
        .map(|p| AgentMsg::Request {
            id: p.id,
            request: p.info.clone(),
            expires_in: p
                .deadline
                .saturating_duration_since(Instant::now())
                .as_secs(),
        });
    if let Some(waiting) = waiting {
        ipc::write_msg(&mut writer, &waiting).await?;
    }

    loop {
        tokio::select! {
            msg = ipc::read_msg::<_, ClientMsg>(&mut reader) => match msg? {
                ClientMsg::Hello { .. } => {}
                ClientMsg::Answer { id, accept } => {
                    let taken = {
                        let mut pending = state.pending.lock().expect("not poisoned");
                        match pending.as_ref() {
                            Some(p) if p.id == id => pending.take(),
                            _ => None,
                        }
                    };
                    match taken {
                        // Whoever answers first decides; the rest are told it closed.
                        Some(p) => {
                            let _ = p.answer.send(accept);
                            state.broadcast(AgentMsg::Closed { id });
                        }
                        None => ipc::write_msg(&mut writer, &AgentMsg::TooLate { id }).await?,
                    }
                }
                ClientMsg::Stop => {
                    state.log("asked to stop by `beam service stop`");
                    state.stop.notify_one();
                }
            },
            event = events.recv() => match event {
                Ok(event) => ipc::write_msg(&mut writer, &event).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
        }
    }
}
