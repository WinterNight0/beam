//! `beam ui`: what plain `beam` opens (ADR-0043).

use super::{App, CommandError, Io};
use crate::config::{self, Config, UiMode};

/// The values `beam ui` takes.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum UiChoice {
    /// The full-screen view
    Tui,
    /// The help; every command is typed out
    Cli,
}

impl App {
    pub(super) fn ui(&self, choice: Option<UiChoice>, io: &mut Io<'_>) -> Result<(), CommandError> {
        let path = self.store.config_path();
        let fail = |e: config::ConfigError| CommandError::Message(e.to_string());
        let mode = match choice {
            None => Config::load(&path).map_err(fail)?.ui,
            // The default is written as no line at all, like the other
            // settings beam edits, so the file only records a choice.
            Some(UiChoice::Tui) => {
                config::set_value(&path, "ui", None).map_err(fail)?;
                UiMode::Tui
            }
            Some(UiChoice::Cli) => {
                config::set_value(&path, "ui", Some(&config::toml_string("cli"))).map_err(fail)?;
                UiMode::Cli
            }
        };

        if self.json {
            return super::identity_cmds::write_json(
                io,
                &serde_json::json!({ "ui": mode.as_str() }),
            );
        }
        match (choice, mode) {
            (None, UiMode::Tui) => writeln!(
                io.out,
                "Plain `beam` opens the full-screen view. `beam ui cli` makes it print the help instead."
            )?,
            (None, UiMode::Cli) => writeln!(
                io.out,
                "Plain `beam` prints the help. `beam ui tui` makes it open the full-screen view."
            )?,
            (Some(_), UiMode::Tui) => writeln!(
                io.out,
                "Plain `beam` now opens the full-screen view. Every command still works as before."
            )?,
            (Some(_), UiMode::Cli) => writeln!(
                io.out,
                "Plain `beam` now prints the help. Every command works as before; \
                 `beam ui tui` brings the full-screen view back."
            )?,
        }
        Ok(())
    }
}
