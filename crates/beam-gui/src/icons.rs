//! Icons from **Fluent UI System Icons** (Regular, 20 px designs; MIT,
//! Microsoft), bundled as `assets/fonts/FluentSystemIcons-beam.ttf` and
//! loaded at start-up, so they look the same on Windows and Linux (ADR-0051).
//! Codepoints are from the font's own index, `FluentSystemIcons-Regular.json`
//! (`ic_fluent_<name>_20_regular`).
//!
//! The bundled font holds only these glyphs. Adding an icon means adding it
//! here **and** rebuilding the font (`assets/README.md`); the test below
//! catches one that was forgotten.

use iced::Font;

pub const FONT: Font = Font::with_name("FluentSystemIcons-Regular");

pub const PEOPLE: char = '\u{F5A8}'; // people
pub const TRANSFERS: char = '\u{F18D}'; // arrow_swap
pub const INBOX: char = '\u{F514}'; // mail_inbox
pub const HISTORY: char = '\u{F47E}'; // history
pub const SETTINGS: char = '\u{F6A9}'; // settings
pub const SEARCH: char = '\u{F68F}'; // search
pub const ADD: char = '\u{F109}'; // add
pub const SEND: char = '\u{F699}'; // send
pub const UPLOAD: char = '\u{F1A4}'; // arrow_upload
pub const DOWNLOAD: char = '\u{F150}'; // arrow_download
pub const FOLDER: char = '\u{F418}'; // folder
pub const DOCUMENT: char = '\u{F378}'; // document
pub const SHIELD: char = '\u{EAC8}'; // shield_checkmark
pub const LOCK: char = '\u{E78F}'; // lock_closed
pub const CLOSE: char = '\u{F369}'; // dismiss
pub const CHECK: char = '\u{F294}'; // checkmark
pub const WARNING: char = '\u{F869}'; // warning
pub const INFO: char = '\u{F4A3}'; // info
pub const COPY: char = '\u{F32B}'; // copy
pub const EDIT: char = '\u{F3DD}'; // edit
pub const DELETE: char = '\u{F34C}'; // delete
pub const GLOBE: char = '\u{F45A}'; // globe
pub const LINK: char = '\u{F4E4}'; // link
pub const FILTER: char = '\u{F406}'; // filter
pub const REFRESH: char = '\u{F190}'; // arrow_sync
pub const CLOCK: char = '\u{F2DD}'; // clock
pub const SUN: char = '\u{F8A1}'; // weather_sunny
pub const MOON: char = '\u{F87D}'; // weather_moon
pub const CHEVRON_RIGHT: char = '\u{F2B0}'; // chevron_right
pub const KEY: char = '\u{F4B6}'; // key
pub const PLUG: char = '\u{E999}'; // plug_connected
pub const ROUTER: char = '\u{EA3B}'; // router

/// Every icon above, for the test.
#[cfg(test)]
const ALL: [char; 32] = [
    PEOPLE,
    TRANSFERS,
    INBOX,
    HISTORY,
    SETTINGS,
    SEARCH,
    ADD,
    SEND,
    UPLOAD,
    DOWNLOAD,
    FOLDER,
    DOCUMENT,
    SHIELD,
    LOCK,
    CLOSE,
    CHECK,
    WARNING,
    INFO,
    COPY,
    EDIT,
    DELETE,
    GLOBE,
    LINK,
    FILTER,
    REFRESH,
    CLOCK,
    SUN,
    MOON,
    CHEVRON_RIGHT,
    KEY,
    PLUG,
    ROUTER,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The codepoints a font's `cmap` table maps (format 4: the BMP, which
    /// is where every icon here lives).
    fn mapped(font: &[u8]) -> Vec<u32> {
        let u16_at = |at: usize| u32::from(u16::from_be_bytes([font[at], font[at + 1]]));
        let u32_at = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
        let tables = u16_at(4) as usize;
        let cmap = (0..tables)
            .map(|i| 12 + 16 * i)
            .find(|&rec| &font[rec..rec + 4] == b"cmap")
            .map(|rec| u32_at(rec + 8))
            .expect("a cmap table");
        let subtables = u16_at(cmap + 2) as usize;
        let mut out = Vec::new();
        for i in 0..subtables {
            let at = cmap + u32_at(cmap + 4 + 8 * i + 4);
            if u16_at(at) != 4 {
                continue;
            }
            let segments = u16_at(at + 6) as usize / 2;
            let ends = at + 14;
            let starts = ends + 2 * segments + 2;
            for s in 0..segments {
                let (start, end) = (u16_at(starts + 2 * s), u16_at(ends + 2 * s));
                if start != 0xFFFF {
                    out.extend(start..=end);
                }
            }
        }
        out
    }

    #[test]
    fn every_icon_is_in_the_bundled_font() {
        let mapped = mapped(crate::fonts::ICONS);
        let missing: Vec<String> = ALL
            .iter()
            .filter(|c| !mapped.contains(&u32::from(**c)))
            .map(|c| format!("U+{:04X}", u32::from(*c)))
            .collect();
        assert!(
            missing.is_empty(),
            "not in FluentSystemIcons-beam.ttf (rebuild it, assets/README.md): {missing:?}"
        );
    }
}
