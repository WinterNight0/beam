//! beam's desktop window (ADR-0049).
//!
//! A front end like the CLI and the full-screen view: it draws what
//! `beam-model` holds and runs the flows in `beam-core`'s `actions`, and
//! keeps only its own layout, clicks and words. Every rule that keeps beam
//! safe is enforced below it, so this crate cannot weaken one.
//!
//! Being built in steps on `GUI-Experimental`; not released yet.

// No console window behind the GUI on Windows (release builds only, so a
// debug build still shows panics).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> iced::Result {
    iced::application(app::Gui::boot, app::Gui::update, app::Gui::view)
        .title("beam")
        .window_size((900.0, 560.0))
        .run()
}
