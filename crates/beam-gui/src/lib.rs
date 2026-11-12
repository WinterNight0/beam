//! beam's desktop window (ADR-0049): a front end like the CLI and the
//! full-screen view. It draws what `beam-model` holds and runs the flows in
//! `beam-core`'s `actions`, keeping only its own layout, clicks and words.
//! Every rule that keeps beam safe is enforced below it, so this crate
//! cannot weaken one.
//!
//! Not released yet. A library and a binary, like `beam`: the binary only opens
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

/// beam's logos, bundled like the fonts (ADR-0054; `assets/README.md`).
pub mod images {
    use std::sync::LazyLock;

    use iced::widget::image::Handle;
    use iced::window::icon::{self, Icon};

    /// The bolt in its circle: the mark at the top of the rail and on the
    /// welcome card.
    pub const MARK_PNG: &[u8] = include_bytes!("../assets/images/logo-b.png");
    /// The full logo, the bolt on a folder: the window and taskbar icon.
    pub const LOGO_PNG: &[u8] = include_bytes!("../assets/images/beam.png");

    /// One handle for the whole run: iced keeps a decoded image per handle,
    /// and a new handle each frame would decode it again each frame.
    pub static MARK: LazyLock<Handle> = LazyLock::new(|| Handle::from_bytes(MARK_PNG));

    /// The window icon, or `None` (the system's default) should the bundled
    /// file not decode, which `tests::the_bundled_logos_decode` rules out.
    pub fn window_icon() -> Option<Icon> {
        let pixels = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png)
            .ok()?
            .into_rgba8();
        let (width, height) = pixels.dimensions();
        icon::from_rgba(pixels.into_raw(), width, height).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_bundled_logos_decode() {
            for png in [MARK_PNG, LOGO_PNG] {
                let decoded =
                    image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap();
                assert_eq!(decoded.width(), decoded.height(), "square, for an icon");
            }
            assert!(window_icon().is_some());
        }
    }
}
