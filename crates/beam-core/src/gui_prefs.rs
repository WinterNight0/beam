//! The desktop window's own preferences: `gui.toml`, beside `config.toml` in
//! the beam folder (ADR-0052).
//!
//! Only the look of the window and how it draws, so nothing here touches a
//! rule: theme, zoom, which answer of a question is highlighted (Enter
//! starts on the safe answer either way), and whether to draw in software.
//! The command line never reads this file, which is why it is not
//! `config.toml`: that one is shared, and a GUI choice should not appear in
//! `beam`'s settings.
//!
//! A missing file is the defaults. A damaged one is reported and the
//! defaults are used; it is replaced on the next change.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The file's name in the beam folder.
pub const FILE_NAME: &str = "gui.toml";

/// Interface zoom limits, as the window offers them.
pub const ZOOM_MIN: f32 = 0.8;
pub const ZOOM_MAX: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Midnight,
    Daylight,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Theme::Midnight => "midnight",
            Theme::Daylight => "daylight",
        }
    }
}

/// Which answer of a question is purple and on the right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Highlight {
    /// The safe answer (Keep, Decline): beam's default.
    #[default]
    Safe,
    /// The action (Accept, Pair).
    Action,
}

impl Highlight {
    fn as_str(self) -> &'static str {
        match self {
            Highlight::Safe => "safe",
            Highlight::Action => "action",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuiPrefs {
    pub theme: Theme,
    pub zoom: f32,
    pub highlight: Highlight,
    /// Draw on the CPU (tiny-skia) instead of the GPU: for older computers,
    /// or graphics drivers that misbehave. Applies when the window starts.
    pub software_rendering: bool,
}

impl Default for GuiPrefs {
    fn default() -> Self {
        Self {
            theme: Theme::Midnight,
            zoom: 1.0,
            highlight: Highlight::Safe,
            software_rendering: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("could not save {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl GuiPrefs {
    /// Reads `path`: the defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Self, PrefsError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(PrefsError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut prefs: Self = toml::from_str(text).map_err(|e| PrefsError::Parse {
            path: path.to_path_buf(),
            message: e.message().to_string(),
        })?;
        prefs.zoom = clamp_zoom(prefs.zoom);
        Ok(prefs)
    }

    /// Writes the whole file: to a temporary file first, then moved into
    /// place, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<(), PrefsError> {
        let write = |source| PrefsError::Write {
            path: path.to_path_buf(),
            source,
        };
        // Written by hand: four plain keys, and the workspace builds `toml`
        // without its writer (it would be another crate).
        let text = format!(
            "# beam's desktop window (Settings > Appearance). The command line does not read this file.\n\
             theme = \"{}\"\nzoom = {:?}\nhighlight = \"{}\"\nsoftware_rendering = {}\n",
            self.theme.as_str(),
            clamp_zoom(self.zoom),
            self.highlight.as_str(),
            self.software_rendering,
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(write)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(write)?;
        std::fs::rename(&tmp, path).map_err(write)
    }
}

/// A zoom within the offered range; anything unreadable is 100%.
fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_MIN, ZOOM_MAX)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let prefs = GuiPrefs::load(&tmp.path().join(FILE_NAME)).unwrap();
        assert_eq!(prefs, GuiPrefs::default());
        assert!(!prefs.software_rendering, "the GPU first");
        assert_eq!(prefs.highlight, Highlight::Safe);
    }

    #[test]
    fn preferences_round_trip_and_keep_to_the_zoom_range() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("beam").join(FILE_NAME);
        let prefs = GuiPrefs {
            theme: Theme::Daylight,
            zoom: 1.25,
            highlight: Highlight::Action,
            software_rendering: true,
        };
        prefs.save(&path).unwrap();
        assert_eq!(GuiPrefs::load(&path).unwrap(), prefs);
        // 100% is written `1.0`: TOML reads a bare `1` as an integer.
        GuiPrefs::default().save(&path).unwrap();
        assert_eq!(GuiPrefs::load(&path).unwrap(), GuiPrefs::default());
        assert!(!path.with_extension("toml.tmp").exists());

        std::fs::write(&path, "zoom = 9.0\n").unwrap();
        let read = GuiPrefs::load(&path).unwrap();
        assert_eq!(read.zoom, ZOOM_MAX);
        assert_eq!(read.theme, Theme::Midnight, "missing keys are defaults");
    }

    #[test]
    fn a_damaged_file_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        std::fs::write(&path, "theme = \"purple\"\n").unwrap();
        assert!(matches!(
            GuiPrefs::load(&path),
            Err(PrefsError::Parse { .. })
        ));
    }
}
