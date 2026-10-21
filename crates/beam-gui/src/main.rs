//! Opens beam's desktop window (ADR-0049). Everything else is in the
//! library: `beam_gui::app` (state), `view` (drawing), `theme`, `icons`.

// No console window behind the GUI on Windows (release builds only, so a
// debug build still shows panics).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use beam_core::gui_prefs::GuiPrefs;
use beam_core::identity::Store;
use beam_gui::app::{Gui, MIN_SIZE, SIZE};
use beam_gui::renderer::{self, Chosen};
use iced::{Size, window};

fn main() -> iced::Result {
    // "Software rendering" (Settings): iced reads the renderer only from
    // the environment, so start again with it set, before any window.
    // Should that fail, this process carries on with iced's default.
    let software = Store::default_dir()
        .ok()
        .and_then(|dir| GuiPrefs::load(&Store::new(dir).gui_prefs_path()).ok())
        .is_some_and(|prefs| prefs.software_rendering);
    if renderer::restart_in_software(software, &Chosen::now())
        && renderer::start_again(true).is_ok()
    {
        return Ok(());
    }
    let mut app = iced::application(Gui::boot, Gui::update, Gui::view)
        .title("beam")
        .theme(Gui::theme)
        .subscription(Gui::subscription)
        .scale_factor(Gui::scale_factor)
        .default_font(beam_gui::theme::FONT)
        .window(window::Settings {
            size: Size::new(SIZE.0, SIZE.1),
            min_size: Some(Size::new(MIN_SIZE.0, MIN_SIZE.1)),
            position: window::Position::Centered,
            // Closing stops the Receiving switch properly first (and asks if
            // a file is arriving): `app/receiving.rs`. Set here, not with
            // `.exit_on_close_request`, which these settings would replace.
            exit_on_close_request: false,
            ..window::Settings::default()
        });
    // Bundled, so nothing depends on the fonts a system happens to have.
    for font in beam_gui::fonts::ALL {
        app = app.font(font);
    }
    app.run()
}
