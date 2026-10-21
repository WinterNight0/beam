//! beam's desktop window (ADR-0049): a front end like the CLI and the
//! full-screen view. It draws what `beam-model` holds and runs the flows in
//! `beam-core`'s `actions`, keeping only its own layout, clicks and words.
//! Every rule that keeps beam safe is enforced below it, so this crate
//! cannot weaken one.
//!
//! Being built in steps on `GUI-Experimental` ("GUI EX Patch #N"); not
//! released yet. A library and a binary, like `beam`: the binary only opens
//! the window, and the design system can hold styles later patches use.

pub mod app;
pub mod icons;
pub mod renderer;
pub mod theme;
mod view;

/// The fonts the window draws with, bundled so it looks the same on Windows
/// and Linux and nothing depends on what a system has installed (ADR-0051).
/// Sources, versions and licences: `assets/README.md`, `assets/LICENSES/`.
pub mod fonts {
    pub const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
    pub const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
    pub const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
    pub const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
    pub const ICONS: &[u8] = include_bytes!("../assets/fonts/FluentSystemIcons-beam.ttf");

    pub const ALL: [&[u8]; 5] = [INTER_REGULAR, INTER_SEMIBOLD, INTER_BOLD, MONO, ICONS];
}
