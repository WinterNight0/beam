//! Which renderer draws the window (ADR-0052): the GPU (wgpu) first, as
//! iced does, falling back to software by itself when the GPU cannot start;
//! or software (tiny-skia) when the person turns on "Software rendering" in
//! Settings, for older computers.
//!
//! iced picks the renderer once, from the `ICED_BACKEND` environment
//! variable, before the window opens. A program setting its own environment
//! is `unsafe` in Rust 2024 (forbidden here), so beam instead starts itself
//! again with the variable set for that new process only, and the first one
//! exits before opening a window. The new process has the variable, so it
//! never does this again.

use std::ffi::OsStr;
use std::process::Command;

/// iced's variable: a comma-separated list of renderers to try.
pub const ICED_BACKEND: &str = "ICED_BACKEND";
/// Set on a process beam started itself, so its `ICED_BACKEND` is known to
/// be beam's and not the person's.
const OURS: &str = "BEAM_GUI_RENDERER";
const SOFTWARE: &str = "tiny-skia";

/// Who chose the renderer of this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen {
    /// Nobody: iced's default, the GPU first.
    Default,
    /// beam, from the Settings toggle.
    Beam,
    /// The person, with `ICED_BACKEND`: the toggle cannot override it.
    Person(String),
}

impl Chosen {
    /// From this process's environment.
    pub fn now() -> Self {
        Self::from(
            std::env::var_os(ICED_BACKEND).as_deref(),
            std::env::var_os(OURS).as_deref(),
        )
    }

    fn from(backend: Option<&OsStr>, ours: Option<&OsStr>) -> Self {
        match (backend, ours) {
            (None, _) => Chosen::Default,
            (Some(_), Some(_)) => Chosen::Beam,
            (Some(b), None) => Chosen::Person(b.to_string_lossy().into_owned()),
        }
    }
}

/// Whether to start again in software before opening a window: the
/// toggle is on and nobody has chosen a renderer for this process yet.
pub fn restart_in_software(software: bool, chosen: &Chosen) -> bool {
    software && *chosen == Chosen::Default
}

/// Starts this program again, with the same arguments, drawing in software
/// if `software`. A renderer the person chose with `ICED_BACKEND` is kept.
pub fn start_again(software: bool) -> std::io::Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(std::env::args_os().skip(1));
    match Chosen::now() {
        Chosen::Person(_) => {}
        _ if software => {
            cmd.env(ICED_BACKEND, SOFTWARE).env(OURS, "1");
        }
        _ => {
            cmd.env_remove(ICED_BACKEND).env_remove(OURS);
        }
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unchosen_renderer_is_switched_to_software() {
        let none = Chosen::from(None, None);
        let ours = Chosen::from(Some(OsStr::new("tiny-skia")), Some(OsStr::new("1")));
        let theirs = Chosen::from(Some(OsStr::new("wgpu")), None);
        assert_eq!(none, Chosen::Default);
        assert_eq!(ours, Chosen::Beam);
        assert_eq!(theirs, Chosen::Person("wgpu".into()));

        assert!(restart_in_software(true, &none));
        assert!(!restart_in_software(false, &none), "the toggle is off");
        assert!(
            !restart_in_software(true, &ours),
            "already restarted: no loop"
        );
        assert!(
            !restart_in_software(true, &theirs),
            "the person's ICED_BACKEND wins"
        );
    }
}
