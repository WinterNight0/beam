//! Settings and history (GUI EX Patch #7): where received files go, router
//! port mapping, the background service, and clearing the history.
//!
//! Every change is the shared action the CLI uses (`actions::settings`,
//! `actions::service`, `history::clear`), so `beam receive-dir`, `beam
//! service` and this page can never disagree. Two changes are questions
//! that start on the safe answer: router port mapping on ("Keep it off":
//! it makes this device findable by scanning), and clearing the history
//! ("Keep").
//!
//! The background service is `beam agent`, from the `beam` program: the GUI
//! registers the `beam` next to it (they are shipped together). Without it,
//! the page says so rather than registering a program that cannot be the
//! agent.

use std::path::{Path, PathBuf};

use beam_core::actions::{service, settings};
use beam_core::agent::status::{self, Running};
use beam_core::untrusted;
use beam_model::{Ask, Choice};
use iced::Task;

use super::{Dialog, Gui, Message, Tone};

/// Turn router port mapping on? No keeps it off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortMappingOn;

/// Clear the whole history? No keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClearHistory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Sent,
    Received,
    Declined,
    Problems,
}

#[derive(Debug, Clone)]
pub enum SettingsMsg {
    ChooseFolder,
    FolderPicked(Option<PathBuf>),
    DefaultFolder,
    /// The port mapping switch was pressed.
    PortMapping(bool),
    AnswerPortMapping(Choice),
    /// "Start at sign-in" was pressed.
    AtSignIn(bool),
    StartAgent,
    StopAgent,
    /// A service action finished, off the window's thread: what to say.
    ServiceDone(Result<String, String>),
    Filter(Filter),
    Search(String),
    ClearHistory,
    AnswerClear(Choice),
}

impl Gui {
    pub(crate) fn on_settings(&mut self, message: SettingsMsg) -> Task<Message> {
        match message {
            SettingsMsg::ChooseFolder => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Where received files go")
                            .pick_folder()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    |dir| Message::Settings(SettingsMsg::FolderPicked(dir)),
                );
            }
            SettingsMsg::FolderPicked(Some(dir)) => self.save_folder(Some(&dir)),
            SettingsMsg::FolderPicked(None) => {}
            SettingsMsg::DefaultFolder => self.save_folder(None),
            SettingsMsg::PortMapping(true) => {
                self.dialog = Some(Dialog::PortMapping(Ask::new(PortMappingOn)));
            }
            SettingsMsg::PortMapping(false) => self.save_port_mapping(false),
            SettingsMsg::AnswerPortMapping(choice) => {
                if matches!(self.dialog, Some(Dialog::PortMapping(_))) {
                    self.dialog = None;
                    if choice == Choice::Yes {
                        self.save_port_mapping(true);
                    }
                }
            }
            SettingsMsg::AtSignIn(on) => {
                let store = self.store.clone();
                let (exe, here) = (self.beam_beside(), self.agent_folder());
                return off_thread(move || {
                    let store = store.ok_or_else(|| "no beam home".to_string())?;
                    if on {
                        let exe = exe.ok_or_else(|| NO_BEAM.to_string())?;
                        let here = here.ok_or_else(|| NO_FOLDER.to_string())?;
                        // As `beam service enable` warns: a build folder can
                        // vanish, and the sign-in entry with it.
                        let built = exe.components().any(|c| c.as_os_str() == "target");
                        service::enable(&store, &exe, &here)
                            .map(|started| {
                                format!(
                                    "The background agent will start at every sign-in. {}{}",
                                    started_words(&started),
                                    if built {
                                        " Note: it runs beam from a build folder; install beam so it does not depend on it."
                                    } else {
                                        ""
                                    }
                                )
                            })
                            .map_err(|e| e.to_string())
                    } else {
                        service::disable(&store)
                            .map(|_| "The background agent will no longer start at sign-in, and is stopped.".to_string())
                            .map_err(|e| e.to_string())
                    }
                });
            }
            SettingsMsg::StartAgent => {
                let store = self.store.clone();
                let (exe, here) = (self.beam_beside(), self.agent_folder());
                return off_thread(move || {
                    let store = store.ok_or_else(|| "no beam home".to_string())?;
                    let (exe, here) = (
                        exe.ok_or_else(|| NO_BEAM.to_string())?,
                        here.ok_or_else(|| NO_FOLDER.to_string())?,
                    );
                    service::start(&store, &exe, &here)
                        .map(|started| started_words(&started))
                        .map_err(|e| e.to_string())
                });
            }
            SettingsMsg::StopAgent => {
                let store = self.store.clone();
                return off_thread(move || {
                    let store = store.ok_or_else(|| "no beam home".to_string())?;
                    service::stop(&store)
                        .map(|stopped| match stopped {
                            service::Stopped::Now => "The background agent stopped.".to_string(),
                            service::Stopped::NotRunning => {
                                "The background agent is not running.".to_string()
                            }
                        })
                        .map_err(|e| e.to_string())
                });
            }
            SettingsMsg::ServiceDone(result) => {
                self.at_sign_in = service::enabled();
                self.notice = Some(match result {
                    Ok(words) => (words, Tone::Success),
                    Err(why) => (untrusted::lines(&why), Tone::Problem),
                });
                self.reload();
                self.connect_link();
            }
            SettingsMsg::Filter(filter) => self.history_filter = filter,
            SettingsMsg::Search(text) => self.history_search = text,
            SettingsMsg::ClearHistory => {
                self.dialog = Some(Dialog::ClearHistory(Ask::new(ClearHistory)));
            }
            SettingsMsg::AnswerClear(choice) => {
                if matches!(self.dialog, Some(Dialog::ClearHistory(_))) {
                    self.dialog = None;
                    if choice == Choice::Yes
                        && let Some(store) = &self.store
                    {
                        self.notice = Some(match beam_core::history::clear(store) {
                            Ok(_) => ("History cleared.".into(), Tone::Success),
                            Err(e) => (
                                format!(
                                    "Could not clear the history: {}",
                                    untrusted::text(&e.to_string())
                                ),
                                Tone::Problem,
                            ),
                        });
                        self.reload();
                    }
                }
            }
        }
        Task::none()
    }

    fn save_folder(&mut self, folder: Option<&Path>) {
        let Some(store) = &self.store else { return };
        self.notice = Some(match settings::set_receive_dir(store, folder) {
            Ok(Some(dir)) => (
                format!(
                    "Received files will be saved to {}.{}",
                    untrusted::name(&dir.display().to_string()),
                    self.restart_hint()
                ),
                Tone::Success,
            ),
            Ok(None) => (
                format!(
                    "Received files go to the default place again.{}",
                    self.restart_hint()
                ),
                Tone::Success,
            ),
            Err(e) => (format!("Nothing was changed: {e}"), Tone::Problem),
        });
        self.reload();
    }

    fn save_port_mapping(&mut self, on: bool) {
        let Some(store) = &self.store else { return };
        self.notice = Some(match settings::set_port_mapping(store, on) {
            Ok(()) => (
                format!(
                    "Router port mapping for the background agent is {}.{}",
                    if on { "on" } else { "off" },
                    self.restart_hint()
                ),
                if on { Tone::Info } else { Tone::Success },
            ),
            Err(e) => (format!("Nothing was changed: {e}"), Tone::Problem),
        });
        self.reload();
    }

    /// A running receiver reads its settings when it starts.
    fn restart_hint(&self) -> &'static str {
        match self.store.as_ref().map(status::read) {
            Some(Running::Yes(_)) => " The receiver running now uses it after a restart.",
            _ => "",
        }
    }

    /// The `beam` program next to this one: what the background service
    /// runs. `None` if it is not there. Looked up once, at start.
    pub(crate) fn beam_beside(&self) -> Option<PathBuf> {
        self.beam_program.clone()
    }

    /// The folder the agent is started in. On Linux that is where it saves
    /// files when no receive folder is set, so one must be chosen first; on
    /// Windows the default is the Downloads folder.
    pub(crate) fn agent_folder(&self) -> Option<PathBuf> {
        let chosen = self
            .config
            .as_ref()
            .is_some_and(|c| c.receive_dir.is_some());
        (cfg!(windows) || chosen)
            .then(std::env::current_dir)
            .and_then(Result::ok)
    }
}

/// `beam` (or `beam.exe`) in `dir`, if it is there.
pub(crate) fn beam_in(dir: &Path) -> Option<PathBuf> {
    let beam = dir.join(format!("beam{}", std::env::consts::EXE_SUFFIX));
    beam.is_file().then_some(beam)
}

const NO_BEAM: &str =
    "the background service is run by the beam program, which is not next to beam-gui";
const NO_FOLDER: &str = "choose where received files go first";

fn started_words(started: &service::Started) -> String {
    match started {
        service::Started::Now { receive_dir } => format!(
            "The background agent is running, saving to {receive_dir}. Requests show up in the Inbox."
        ),
        service::Started::Already { receive_dir } => {
            format!("The background agent is already running, saving to {receive_dir}.")
        }
    }
}

/// Runs a service action on its own thread (it waits for the agent), and
/// reports back without freezing the window.
fn off_thread(work: impl FnOnce() -> Result<String, String> + Send + 'static) -> Task<Message> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    Task::perform(
        async move {
            rx.await
                .unwrap_or_else(|_| Err("the action did not finish".into()))
        },
        |result| Message::Settings(SettingsMsg::ServiceDone(result)),
    )
}
