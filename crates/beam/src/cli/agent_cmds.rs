//! `beam agent`, `beam inbox`, `beam service …` and `beam receive-dir`: the
//! background agent and its controls (ADR-0042).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::desk::{PromptDesk, Question};
use super::terminal::{Keyboard, TerminalReporter};
use super::{App, CommandError, Io};
use crate::agent::ipc::{AgentMsg, Client, ClientMsg};
use crate::agent::status::{self, Running};
use crate::agent::{self, AGENT_ACCEPT_TIMEOUT, AgentOptions, service};
use crate::config::{self, Config};
use crate::pairing::Network;
use crate::transfer::{Progress, Reporter};
use crate::transport::PathKind;
use crate::transport::endpoint::Bind;
use crate::{ui, untrusted};

/// The warning shown before router port mapping is turned on for the agent.
const PORT_MAPPING_WARNING: &str = "\
WARNING: router port mapping for the background agent

With this on, the agent asks your router (UPnP, NAT-PMP or PCP) to forward
UDP port 7820 to this computer, for as long as the agent runs, which may be
all day. Then:

  * this computer can be found from the internet by anyone scanning for it;
  * they can see a beam key there, and connect to it;
  * they still cannot send you anything: only paired devices may ask, every
    file needs your Accept in `beam inbox`, and the agent does not pair.

What you gain: more transfers go direct instead of through the relay, which
is faster and keeps the relay out of it.

Leave it off unless transfers to this computer keep going through the relay.";

/// What `beam service` can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ServiceAction {
    /// Start the agent at every login, and start it now
    Enable,
    /// Stop starting it at login, and stop it now
    Disable,
    /// Start the agent now, in the background
    Start,
    /// Stop the running agent cleanly
    Stop,
    /// Show whether the agent runs, where it saves, and its settings
    Status,
}

/// On or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Switch {
    On,
    Off,
}

impl App {
    fn config(&self) -> Result<Config, CommandError> {
        Config::load(&self.store.config_path()).map_err(|e| CommandError::Message(e.to_string()))
    }

    /// `beam agent`: runs the agent in this process until stopped.
    pub(super) fn agent(&self, loopback: bool, io: &mut Io<'_>) -> Result<(), CommandError> {
        let identity = self.store.load_identity()?;
        self.store.load_known_peers()?;
        let config = self.config()?;
        let (receive_dir, _) = agent::receive_dir(config.receive_dir.as_deref());
        let options = AgentOptions {
            network: Network {
                relay: config.relay,
                bind: if loopback { Bind::Loopback } else { Bind::Any },
                port: config.port,
                advertise: config.advertise,
            },
            receive_dir,
            accept_timeout: AGENT_ACCEPT_TIMEOUT,
            port_mapping: config.agent_port_mapping,
            notify: true,
            echo: true,
            in_view: false,
        };
        let _ = io;
        let runtime = self.runtime()?;
        let result = runtime.block_on(agent::run(
            identity,
            self.store.clone(),
            options,
            stop_signal(),
        ));
        runtime.shutdown_timeout(Duration::from_secs(1));
        result.map_err(|e| CommandError::Message(e.to_string()))
    }

    /// `beam inbox`: answers the agent's requests, at the usual prompt.
    pub(super) fn inbox(&self, io: &mut Io<'_>) -> Result<(), CommandError> {
        let status = match status::read(&self.store) {
            Running::Yes(status) => status,
            Running::No => {
                return Err(CommandError::Message(
                    "the background agent is not running. Start it with `beam service start` \
                     (or `beam service enable` to start it at every login), or use \
                     `beam listen`"
                        .into(),
                ));
            }
            Running::Unreadable => {
                return Err(CommandError::Message(
                    "the background agent is running, but its status file cannot be read; \
                     restart it with `beam service stop` and `beam service start`"
                        .into(),
                ));
            }
        };
        let runtime = self.runtime()?;
        let result = runtime.block_on(async {
            let (mut client, welcome) = Client::connect(status.port, &status.token)
                .await
                .map_err(|e| CommandError::Message(format!("could not reach the agent: {e}")))?;
            let AgentMsg::Welcome { receive_dir, .. } = welcome else {
                return Err(CommandError::Message(
                    "the agent did not answer as expected".into(),
                ));
            };
            writeln!(
                io.out,
                "Waiting for requests from paired devices. The agent saves files to {}.\n\
                 Ctrl+C leaves the inbox; the agent keeps running.",
                untrusted::name(&receive_dir)
            )?;
            io.out.flush()?;

            let desk = PromptDesk::terminal(Keyboard::start());
            let mut reporter = TerminalReporter::with_desk(desk.clone());
            let (answers_tx, mut answers) = tokio::sync::mpsc::unbounded_channel::<(u64, bool)>();
            loop {
                tokio::select! {
                    msg = client.next() => {
                        let Ok(msg) = msg else {
                            reporter.finish();
                            writeln!(io.out, "The agent stopped.")?;
                            return Ok(());
                        };
                        match msg {
                            AgentMsg::Request { id, request, expires_in } => {
                                // The same prompt, and the same rules, as `beam listen`.
                                let desk = desk.clone();
                                let tx = answers_tx.clone();
                                let question = Question::Transfer(request.into());
                                let left = Duration::from_secs(expires_in.max(1));
                                tokio::task::spawn_blocking(move || {
                                    let accept = desk.ask(question, left);
                                    let _ = tx.send((id, accept));
                                });
                            }
                            AgentMsg::Closed { .. } | AgentMsg::Welcome { .. } => {}
                            AgentMsg::TooLate { .. } => desk.notice(
                                "That request had already closed (answered elsewhere, expired, \
                                 or withdrawn by the sender); the answer did not count.",
                            ),
                            AgentMsg::Progress { done, total, relay } => {
                                reporter.report(Progress::Transferring {
                                    done,
                                    total,
                                    path: if relay { PathKind::Relay } else { PathKind::Direct },
                                });
                            }
                            AgentMsg::Finished { text, .. } => {
                                reporter.finish();
                                desk.notice(untrusted::text(&text));
                            }
                            AgentMsg::Stopping => {
                                reporter.finish();
                                writeln!(io.out, "The agent is stopping.")?;
                                return Ok(());
                            }
                        }
                    }
                    Some((id, accept)) = answers.recv() => {
                        if client.send(&ClientMsg::Answer { id, accept }).await.is_err() {
                            writeln!(io.out, "The agent stopped.")?;
                            return Ok(());
                        }
                    }
                    () = super::net_cmds::interrupted() => {
                        reporter.finish();
                        writeln!(io.out, "\nLeft the inbox; the agent keeps running.")?;
                        return Ok(());
                    }
                }
            }
        });
        runtime.shutdown_timeout(Duration::from_millis(200));
        // The keyboard thread may still be waiting for a line.
        result
    }

    /// `beam service <action>`.
    pub(super) fn service(
        &self,
        action: ServiceAction,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let exe = std::env::current_exe().map_err(CommandError::Io)?;
        let here = std::env::current_dir().map_err(CommandError::Io)?;
        let fail = |e: service::ServiceError| CommandError::Message(e.to_string());
        match action {
            ServiceAction::Enable => {
                self.store.load_identity()?;
                service::enable(&exe, self.store.dir(), &here).map_err(fail)?;
                writeln!(io.out, "The background agent will start at every login.")?;
                if exe.components().any(|c| c.as_os_str() == "target") {
                    writeln!(
                        io.out,
                        "Note: it will run {} — a build folder. Install beam first \
                         (scripts\\install.bat on Windows) so it does not depend on it.",
                        exe.display()
                    )?;
                }
                self.start_agent(&exe, &here, io)
            }
            ServiceAction::Disable => {
                service::disable().map_err(fail)?;
                writeln!(
                    io.out,
                    "The background agent will no longer start at login."
                )?;
                self.stop_agent(io)
            }
            ServiceAction::Start => self.start_agent(&exe, &here, io),
            ServiceAction::Stop => self.stop_agent(io),
            ServiceAction::Status => self.service_status(io),
        }
    }

    fn start_agent(&self, exe: &Path, here: &Path, io: &mut Io<'_>) -> Result<(), CommandError> {
        if let Running::Yes(status) = status::read(&self.store) {
            writeln!(
                io.out,
                "The background agent is already running, saving to {}.",
                untrusted::name(&status.receive_dir)
            )?;
            return Ok(());
        }
        self.store.load_identity()?;
        let config = self.config()?;
        if config.receive_dir.is_none() && !cfg!(windows) {
            writeln!(
                io.out,
                "Received files will be saved here, in {} (change it with `beam receive-dir`).",
                here.display()
            )?;
        }
        service::start(exe, here, self.store.dir())
            .map_err(|e| CommandError::Message(e.to_string()))?;
        // Wait for it to say it is up.
        for _ in 0..50 {
            if let Running::Yes(status) = status::read(&self.store) {
                writeln!(
                    io.out,
                    "The background agent is running. It saves files to {}.\n\
                     When a paired device sends something you get a notification; answer in \
                     a terminal with `beam inbox`.",
                    untrusted::name(&status.receive_dir)
                )?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(CommandError::Message(format!(
            "the background agent did not start; its log is {}",
            self.store.agent_log_path().display()
        )))
    }

    fn stop_agent(&self, io: &mut Io<'_>) -> Result<(), CommandError> {
        let Running::Yes(status) = status::read(&self.store) else {
            writeln!(io.out, "The background agent is not running.")?;
            return Ok(());
        };
        let runtime = self.runtime()?;
        runtime
            .block_on(async {
                let (mut client, _) = Client::connect(status.port, &status.token).await?;
                client.send(&ClientMsg::Stop).await
            })
            .map_err(|e| CommandError::Message(format!("could not reach the agent: {e}")))?;
        for _ in 0..80 {
            if !matches!(status::read(&self.store), Running::Yes(_)) {
                writeln!(io.out, "The background agent stopped.")?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(CommandError::Message(
            "the background agent was asked to stop but is still running".into(),
        ))
    }

    fn service_status(&self, io: &mut Io<'_>) -> Result<(), CommandError> {
        let config = self.config()?;
        match status::read(&self.store) {
            Running::Yes(status) => {
                let what = if status.in_view {
                    format!(
                        "receiving in beam's full-screen view (process {}); it stops when that                          view closes or its Receiving switch is turned off",
                        status.pid
                    )
                } else {
                    format!("running (process {})", status.pid)
                };
                ui::field(io.out, "Agent", &what)?;
                ui::field(io.out, "Saving to", &untrusted::name(&status.receive_dir))?;
                ui::field(
                    io.out,
                    "Port mapping",
                    if status.port_mapping { "on" } else { "off" },
                )?;
            }
            Running::No => {
                ui::field(io.out, "Agent", "not running")?;
                let (dir, chosen) = agent::receive_dir(config.receive_dir.as_deref());
                let dir = dir.display().to_string();
                let note = if chosen {
                    dir
                } else if cfg!(windows) {
                    format!("{dir} (your Downloads folder)")
                } else {
                    "the folder `beam service start` is run in".to_string()
                };
                ui::field(io.out, "Would save to", &note)?;
                ui::field(
                    io.out,
                    "Port mapping",
                    if config.agent_port_mapping {
                        "on"
                    } else {
                        "off"
                    },
                )?;
            }
            Running::Unreadable => ui::field(io.out, "Agent", "running (status unreadable)")?,
        }
        ui::field(
            io.out,
            "At login",
            if service::is_enabled() {
                "starts automatically"
            } else {
                "does not start"
            },
        )?;
        ui::field(
            io.out,
            "Pairing",
            "off in the agent (use `beam listen` or `beam pair --wait`)",
        )?;
        ui::field(
            io.out,
            "Log",
            &self.store.agent_log_path().display().to_string(),
        )?;
        Ok(())
    }

    /// `beam service port-mapping on|off`.
    pub(super) fn port_mapping(&self, switch: Switch, io: &mut Io<'_>) -> Result<(), CommandError> {
        let path = self.store.config_path();
        let fail = |e: config::ConfigError| CommandError::Message(e.to_string());
        match switch {
            Switch::Off => {
                config::set_value(&path, "agent_port_mapping", None).map_err(fail)?;
                writeln!(io.out, "Router port mapping for the agent is off.")?;
            }
            Switch::On => {
                writeln!(io.out, "{PORT_MAPPING_WARNING}\n")?;
                if !ui::confirm(
                    io.input,
                    io.out,
                    "Turn router port mapping on for the agent?",
                )? {
                    writeln!(io.out, "Nothing was changed; it stays off.")?;
                    return Ok(());
                }
                config::set_value(&path, "agent_port_mapping", Some("true")).map_err(fail)?;
                writeln!(io.out, "Router port mapping for the agent is on.")?;
            }
        }
        if matches!(status::read(&self.store), Running::Yes(_)) {
            writeln!(
                io.out,
                "The running agent uses it after a restart: `beam service stop`, then \
                 `beam service start`."
            )?;
        }
        Ok(())
    }

    /// `beam receive-dir [PATH] [--default]`.
    pub(super) fn receive_dir(
        &self,
        path: Option<PathBuf>,
        default: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let config_path = self.store.config_path();
        let fail = |e: config::ConfigError| CommandError::Message(e.to_string());
        if default {
            config::set_value(&config_path, "receive_dir", None).map_err(fail)?;
            writeln!(io.out, "Received files go to the default place again:")?;
        } else if let Some(path) = path {
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map_err(CommandError::Io)?
                    .join(path)
            };
            std::fs::create_dir_all(&path).map_err(CommandError::Io)?;
            let path = std::fs::canonicalize(&path).map_err(CommandError::Io)?;
            let path = strip_verbatim(path);
            let text = path.display().to_string();
            config::set_value(
                &config_path,
                "receive_dir",
                Some(&config::toml_string(&text)),
            )
            .map_err(fail)?;
            writeln!(io.out, "Received files will be saved to {text}")?;
            writeln!(
                io.out,
                "(by the background agent, and by `beam listen` when it is not given --out)"
            )?;
            if matches!(status::read(&self.store), Running::Yes(_)) {
                writeln!(
                    io.out,
                    "The running agent uses it after a restart: `beam service stop`, then \
                     `beam service start`."
                )?;
            }
            return Ok(());
        }

        let config = self.config()?;
        match config.receive_dir {
            Some(dir) => writeln!(io.out, "{}", dir.display())?,
            None => {
                let (dir, _) = agent::receive_dir(None);
                if cfg!(windows) {
                    writeln!(
                        io.out,
                        "{} (your Downloads folder; the default)",
                        dir.display()
                    )?;
                } else {
                    writeln!(
                        io.out,
                        "not set: `beam listen` saves where it is run, and the agent where \
                         `beam service start` was run"
                    )?;
                }
            }
        }
        writeln!(
            io.out,
            "Change it with `beam receive-dir <folder>`; `beam receive-dir --default` resets it."
        )?;
        Ok(())
    }
}

/// `C:\x` rather than `\\?\C:\x`, which canonicalize returns on Windows and
/// people do not recognise.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// What stops `beam agent`: Ctrl+C, or SIGTERM on Unix (what `systemctl
/// stop` sends).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    () = super::net_cmds::interrupted() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => super::net_cmds::interrupted().await,
        }
    }
    #[cfg(not(unix))]
    super::net_cmds::interrupted().await;
}
