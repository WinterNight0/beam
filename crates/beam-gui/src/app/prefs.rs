//! The window's saved preferences (GUI EX Patch #9, ADR-0052): theme, zoom,
//! the highlighted answer and software rendering, in `gui.toml` beside
//! `config.toml` (`beam_core::gui_prefs`). Read when the window opens,
//! written on every change.
//!
//! Software rendering applies when the window starts (`renderer.rs`), so
//! the toggle offers Restart now, which closes the window the way its close
//! button does (asking first if a file is moving) and starts beam again.

use beam_core::gui_prefs::{GuiPrefs, Highlight, Theme};
use beam_core::untrusted;
use iced::{Task, window};

use super::{Emphasis, Gui, Message, RecvMsg, Tone};
use crate::renderer;

impl Gui {
    /// Applies `gui.toml`, if there is one. A damaged file is said once and
    /// the defaults are kept; the next change replaces it.
    pub(crate) fn load_prefs(&mut self) {
        let Some(store) = &self.store else { return };
        match GuiPrefs::load(&store.gui_prefs_path()) {
            Ok(prefs) => {
                self.dark = prefs.theme == Theme::Midnight;
                self.zoom = prefs.zoom;
                self.emphasis = match prefs.highlight {
                    Highlight::Safe => Emphasis::Safe,
                    Highlight::Action => Emphasis::Action,
                };
                self.software = prefs.software_rendering;
            }
            Err(e) => {
                self.notice = Some((
                    format!(
                        "{} The window's defaults are used.",
                        untrusted::text(&e.to_string())
                    ),
                    Tone::Problem,
                ));
            }
        }
    }

    /// Writes the preferences as they are now.
    pub(crate) fn save_prefs(&mut self) {
        let Some(store) = &self.store else { return };
        let prefs = GuiPrefs {
            theme: if self.dark {
                Theme::Midnight
            } else {
                Theme::Daylight
            },
            zoom: self.zoom,
            highlight: match self.emphasis {
                Emphasis::Safe => Highlight::Safe,
                Emphasis::Action => Highlight::Action,
            },
            software_rendering: self.software,
        };
        if let Err(e) = prefs.save(&store.gui_prefs_path()) {
            self.notice = Some((untrusted::text(&e.to_string()), Tone::Problem));
        }
    }

    /// Restart now: close as the close button does, then start again.
    pub(crate) fn restart(&mut self) -> Task<Message> {
        window::latest().map(Message::RestartIn)
    }

    pub(crate) fn restart_in(&mut self, id: Option<window::Id>) -> Task<Message> {
        let Some(id) = id else { return Task::none() };
        self.restarting = true;
        self.on_receive(RecvMsg::CloseRequested(id))
    }

    /// Just before the window closes: start the new beam if this close is a
    /// restart. If it cannot start, say so and keep this window open.
    pub(crate) fn start_again_if_restarting(&mut self) -> bool {
        if !std::mem::take(&mut self.restarting) {
            return true;
        }
        match renderer::start_again(self.software) {
            Ok(()) => true,
            Err(e) => {
                self.notice = Some((
                    format!(
                        "Could not start beam again: {}. Close and open it yourself.",
                        untrusted::text(&e.to_string())
                    ),
                    Tone::Problem,
                ));
                false
            }
        }
    }
}
