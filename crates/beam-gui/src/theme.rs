//! beam's design system (ADR-0049, ADR-0051): Aero's structure (navigation
//! rail, list pane, content pane, framed dialogs, status bar) with the rice
//! palette direction (calm dark slate, one periwinkle accent), and a light
//! variant. Approved as the enterprise mock-up in `spike/gui-iced`.
//!
//! Every style takes the palette, so switching Midnight/Daylight is handing
//! the view a different `&'static Pal`.
//!
//! Alignment rules learned in the mock-up, kept by the code that uses this:
//! a button does not centre its content, so centre it explicitly; draw
//! markers as overlays rather than giving them layout room; icons use line
//! height 1.0; a fixed-size tile in a table column is wrapped, not resized.

use iced::border::Radius;
use iced::gradient::Linear;
use iced::widget::{button, container, progress_bar, text_input, toggler};
use iced::{Background, Border, Color, Degrees, Font, Shadow, Theme, Vector, color};

// ---------------------------------------------------------------------------
// Type and spacing
// ---------------------------------------------------------------------------

/// Inter (OFL 1.1) and JetBrains Mono (OFL 1.1), bundled in `assets/fonts`
/// and loaded in `main`, so the text looks the same on Windows and Linux.
pub const FONT: Font = Font::with_name("Inter");
pub const SEMIBOLD: Font = Font {
    weight: iced::font::Weight::Semibold,
    ..FONT
};
pub const BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..FONT
};
pub const MONO: Font = Font::with_name("JetBrains Mono");

/// Text sizes: caption, body, label, title, heading, display.
pub const XS: u32 = 11;
pub const SM: u32 = 12;
pub const MD: u32 = 13;
pub const LG: u32 = 15;
pub const XL: u32 = 18;
pub const XXL: u32 = 24;

/// Corner radii.
pub const R_SM: f32 = 4.0;
pub const R_MD: f32 = 6.0;
pub const R_LG: f32 = 10.0;

// ---------------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------------

pub struct Pal {
    pub name: &'static str,
    pub dark: bool,
    /// The navigation rail, darkest.
    pub rail: Color,
    /// The list pane next to the rail.
    pub pane: Color,
    /// The content area.
    pub surface: Color,
    /// Cards, tables and dialogs on the surface.
    pub raised: Color,
    pub hover: Color,
    pub pressed: Color,
    pub border: Color,
    pub divider: Color,
    pub text: Color,
    pub subtext: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent_hi: Color,
    pub accent_lo: Color,
    pub on_accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    pub veil: Color,
    pub shadow: Color,
}

pub static DARK: Pal = Pal {
    name: "Midnight",
    dark: true,
    rail: color!(0x0f1116),
    pane: color!(0x161920),
    surface: color!(0x1b1e26),
    raised: color!(0x222631),
    hover: color!(0x2a2f3c),
    pressed: color!(0x323848),
    border: color!(0x2f3442),
    divider: color!(0x272b36),
    text: color!(0xe6e9f0),
    subtext: color!(0xa6adbf),
    faint: color!(0x6f778c),
    accent: color!(0x7c8cff),
    accent_hi: color!(0x9aa6ff),
    accent_lo: color!(0x5f6fe8),
    on_accent: color!(0x0d1020),
    success: color!(0x5ccf8a),
    warning: color!(0xe8b44f),
    danger: color!(0xef6b73),
    info: color!(0x5fb4f0),
    veil: Color {
        r: 0.02,
        g: 0.03,
        b: 0.06,
        a: 0.62,
    },
    shadow: Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.5,
    },
};

pub static LIGHT: Pal = Pal {
    name: "Daylight",
    dark: false,
    rail: color!(0xe7e9ef),
    pane: color!(0xf3f4f7),
    surface: color!(0xfafbfc),
    raised: color!(0xffffff),
    hover: color!(0xeceef4),
    pressed: color!(0xe1e4ec),
    border: color!(0xd9dde6),
    divider: color!(0xe6e9ef),
    text: color!(0x1a1e29),
    subtext: color!(0x545c70),
    faint: color!(0x8890a4),
    accent: color!(0x4f5fe6),
    accent_hi: color!(0x6372f0),
    accent_lo: color!(0x3c4bd0),
    on_accent: color!(0xffffff),
    success: color!(0x1e9a57),
    warning: color!(0xb1761a),
    danger: color!(0xd23a45),
    info: color!(0x2a7fd4),
    veil: Color {
        r: 0.08,
        g: 0.09,
        b: 0.14,
        a: 0.38,
    },
    shadow: Color {
        r: 0.06,
        g: 0.08,
        b: 0.16,
        a: 0.22,
    },
};

/// Avatar colours: saturated enough for white initials in both palettes.
pub const TINTS: [Color; 8] = [
    color!(0xd9539b),
    color!(0x3fa34d),
    color!(0xe08a1e),
    color!(0x2f8fdc),
    color!(0xc8452f),
    color!(0x8a5bd2),
    color!(0x169f9a),
    color!(0x5f6fe8),
];

/// A friend's avatar colour, from their fingerprint: the same friend has the
/// same colour on every device and after a rename, and it needs no setting.
pub fn tint_for(fingerprint_hex: &str) -> Color {
    let byte = fingerprint_hex
        .get(..2)
        .and_then(|h| u8::from_str_radix(h, 16).ok())
        .unwrap_or(0);
    TINTS[usize::from(byte) % TINTS.len()]
}

/// The iced theme behind the custom styles (text inputs' caret, pick
/// lists, scrollbars).
pub fn theme(p: &Pal) -> Theme {
    Theme::custom(
        format!("beam {}", p.name),
        iced::theme::Palette {
            background: p.surface,
            text: p.text,
            primary: p.accent,
            success: p.success,
            warning: p.warning,
            danger: p.danger,
        },
    )
}

type C = Box<dyn Fn(&Theme) -> container::Style>;
pub type B = Box<dyn Fn(&Theme, button::Status) -> button::Style>;
type I = Box<dyn Fn(&Theme, text_input::Status) -> text_input::Style>;
type S = Box<dyn Fn(&Theme, toggler::Status) -> toggler::Style>;

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

fn fill(bg: Color, text: Color) -> container::Style {
    container::Style {
        background: Some(bg.into()),
        text_color: Some(text),
        ..container::Style::default()
    }
}

pub fn rail(p: &'static Pal) -> C {
    Box::new(move |_| fill(p.rail, p.text))
}

pub fn pane(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        border: Border {
            color: p.divider,
            width: 1.0,
            radius: Radius::default(),
        },
        ..fill(p.pane, p.text)
    })
}

pub fn surface(p: &'static Pal) -> C {
    Box::new(move |_| fill(p.surface, p.text))
}

/// A card: raised, hairline border, soft shadow.
pub fn card(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        border: Border {
            color: p.border,
            width: 1.0,
            radius: R_LG.into(),
        },
        shadow: Shadow {
            color: Color {
                a: p.shadow.a * 0.35,
                ..p.shadow
            },
            offset: Vector::new(0.0, 1.0),
            blur_radius: 3.0,
        },
        ..fill(p.raised, p.text)
    })
}

/// The header of a card or a table: the Aero glass edge, very faint.
pub fn card_header(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(sheen(p.raised, p.dark)),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: Radius {
                top_left: R_LG,
                top_right: R_LG,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
        },
        text_color: Some(p.subtext),
        ..container::Style::default()
    })
}

/// The bar across the top of the content: title, breadcrumb, actions.
pub fn toolbar(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(sheen(p.surface, p.dark)),
        border: Border {
            color: p.divider,
            width: 1.0,
            radius: Radius::default(),
        },
        text_color: Some(p.text),
        ..container::Style::default()
    })
}

/// The status bar along the bottom of the window.
pub fn status_bar(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        border: Border {
            color: p.divider,
            width: 1.0,
            radius: Radius::default(),
        },
        ..fill(p.rail, p.subtext)
    })
}

/// A modal dialog.
pub fn dialog(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        border: Border {
            color: p.border,
            width: 1.0,
            radius: R_LG.into(),
        },
        shadow: Shadow {
            color: p.shadow,
            offset: Vector::new(0.0, 12.0),
            blur_radius: 40.0,
        },
        ..fill(p.raised, p.text)
    })
}

/// The ring around the answer Enter gives on a question. Drawn (clear)
/// round the other answer too, so moving it shifts nothing.
pub fn focus_ring(p: &'static Pal, on: bool) -> C {
    Box::new(move |_| container::Style {
        border: Border {
            color: if on { p.accent } else { Color::TRANSPARENT },
            width: 2.0,
            radius: (R_MD + 2.0).into(),
        },
        ..container::Style::default()
    })
}

pub fn dialog_footer(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(p.pane.into()),
        border: Border {
            color: p.divider,
            width: 1.0,
            radius: Radius {
                top_left: 0.0,
                top_right: 0.0,
                bottom_right: R_LG,
                bottom_left: R_LG,
            },
        },
        ..container::Style::default()
    })
}

pub fn veil(p: &'static Pal, t: f32) -> C {
    Box::new(move |_| {
        fill(
            Color {
                a: p.veil.a * t,
                ..p.veil
            },
            p.text,
        )
    })
}

/// A notice strip: info, success, warning or danger.
pub fn notice(p: &'static Pal, tint: Color) -> C {
    Box::new(move |_| container::Style {
        background: Some(alpha(tint, if p.dark { 0.12 } else { 0.08 }).into()),
        border: Border {
            color: alpha(tint, 0.45),
            width: 1.0,
            radius: R_MD.into(),
        },
        text_color: Some(p.text),
        ..container::Style::default()
    })
}

/// A small coloured label: "Verified", "Direct P2P", "Declined".
pub fn chip(p: &'static Pal, tint: Color) -> C {
    Box::new(move |_| container::Style {
        background: Some(alpha(tint, if p.dark { 0.16 } else { 0.12 }).into()),
        border: Border {
            color: alpha(tint, 0.35),
            width: 1.0,
            radius: Radius::new(999.0),
        },
        text_color: Some(tint),
        ..container::Style::default()
    })
}

/// The count on a rail icon.
pub fn badge(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(p.danger.into()),
        border: Border {
            color: p.rail,
            width: 2.0,
            radius: Radius::new(999.0),
        },
        text_color: Some(Color::WHITE),
        ..container::Style::default()
    })
}

/// A round avatar with a soft Aero gloss in the friend's colour.
pub fn avatar(tint: Color) -> C {
    Box::new(move |_| container::Style {
        background: Some(
            Linear::new(Degrees(180.0))
                .add_stop(0.0, mix(tint, Color::WHITE, 0.22))
                .add_stop(0.5, tint)
                .add_stop(1.0, mix(tint, Color::BLACK, 0.18))
                .into(),
        ),
        border: Border {
            color: alpha(Color::BLACK, 0.18),
            width: 1.0,
            radius: Radius::new(999.0),
        },
        text_color: Some(Color::WHITE),
        ..container::Style::default()
    })
}

/// The dot on an avatar: green when receiving.
pub fn dot(p: &'static Pal, tint: Color) -> C {
    Box::new(move |_| container::Style {
        background: Some(tint.into()),
        border: Border {
            color: p.pane,
            width: 2.0,
            radius: Radius::new(999.0),
        },
        ..container::Style::default()
    })
}

/// A square tile behind an icon (file type, direction).
pub fn icon_tile(p: &'static Pal, tint: Color) -> C {
    Box::new(move |_| container::Style {
        background: Some(alpha(tint, if p.dark { 0.16 } else { 0.10 }).into()),
        border: Border {
            color: alpha(tint, 0.25),
            width: 1.0,
            radius: R_MD.into(),
        },
        text_color: Some(tint),
        ..container::Style::default()
    })
}

/// A block of monospace text: fingerprints, invites, paths.
pub fn code_block(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(if p.dark { p.rail } else { p.pane }.into()),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: R_MD.into(),
        },
        text_color: Some(p.text),
        ..container::Style::default()
    })
}

/// The marker left of the selected rail item.
pub fn rail_marker(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(p.accent.into()),
        border: Border {
            radius: Radius::new(2.0),
            ..Border::default()
        },
        ..container::Style::default()
    })
}

pub fn step(p: &'static Pal, state: Step) -> C {
    Box::new(move |_| {
        let (bg, fg, edge) = match state {
            Step::Done => (p.success, Color::WHITE, p.success),
            Step::Current => (p.accent, p.on_accent, p.accent),
            Step::Todo => (p.raised, p.faint, p.border),
        };
        container::Style {
            background: Some(bg.into()),
            border: Border {
                color: edge,
                width: 1.5,
                radius: Radius::new(999.0),
            },
            text_color: Some(fg),
            ..container::Style::default()
        }
    })
}

#[derive(Clone, Copy, PartialEq)]
pub enum Step {
    Done,
    Current,
    Todo,
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// The one primary action of a screen or dialog: accent, with the Aero glass
/// edge, and a soft glow on hover.
pub fn primary(p: &'static Pal) -> B {
    Box::new(move |_, status| {
        let base = match status {
            button::Status::Hovered => p.accent_hi,
            button::Status::Pressed => p.accent_lo,
            _ => p.accent,
        };
        let disabled = status == button::Status::Disabled;
        button::Style {
            background: Some(if disabled {
                alpha(p.accent, 0.35).into()
            } else {
                gloss(base)
            }),
            text_color: if disabled {
                alpha(p.on_accent, 0.6)
            } else {
                p.on_accent
            },
            border: Border {
                color: if disabled {
                    Color::TRANSPARENT
                } else {
                    mix(base, Color::BLACK, 0.25)
                },
                width: 1.0,
                radius: R_MD.into(),
            },
            shadow: Shadow {
                color: alpha(
                    p.accent,
                    if status == button::Status::Hovered {
                        0.45
                    } else {
                        0.0
                    },
                ),
                offset: Vector::ZERO,
                blur_radius: 10.0,
            },
            ..button::Style::default()
        }
    })
}

/// A secondary action: raised, bordered.
pub fn secondary(p: &'static Pal) -> B {
    Box::new(move |_, status| {
        let bg = match status {
            button::Status::Hovered => p.hover,
            button::Status::Pressed => p.pressed,
            _ => p.raised,
        };
        button::Style {
            background: Some(bg.into()),
            text_color: if status == button::Status::Disabled {
                p.faint
            } else {
                p.text
            },
            border: Border {
                color: p.border,
                width: 1.0,
                radius: R_MD.into(),
            },
            ..button::Style::default()
        }
    })
}

/// A destructive action: red text until hovered, then red.
pub fn danger(p: &'static Pal) -> B {
    Box::new(move |_, status| {
        let (bg, fg) = match status {
            button::Status::Hovered => (p.danger, Color::WHITE),
            button::Status::Pressed => (mix(p.danger, Color::BLACK, 0.2), Color::WHITE),
            _ => (p.raised, p.danger),
        };
        button::Style {
            background: Some(bg.into()),
            text_color: fg,
            border: Border {
                color: alpha(p.danger, 0.6),
                width: 1.0,
                radius: R_MD.into(),
            },
            ..button::Style::default()
        }
    })
}

/// No frame until hovered: toolbar icons, "Copy", the dialog's close.
pub fn ghost(p: &'static Pal) -> B {
    Box::new(move |_, status| {
        let bg = match status {
            button::Status::Hovered => Some(p.hover.into()),
            button::Status::Pressed => Some(p.pressed.into()),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: p.subtext,
            border: Border {
                radius: R_MD.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
}

/// A navigation rail item.
pub fn rail_item(p: &'static Pal, selected: bool) -> B {
    Box::new(move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(alpha(p.accent, if p.dark { 0.16 } else { 0.12 }).into()),
            (false, button::Status::Hovered) => Some(p.hover.into()),
            (false, button::Status::Pressed) => Some(p.pressed.into()),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: if selected { p.accent } else { p.subtext },
            border: Border {
                radius: R_MD.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
}

/// A row in a list or table: highlights on hover, accent tint when selected.
pub fn list_item(p: &'static Pal, selected: bool) -> B {
    Box::new(move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(alpha(p.accent, if p.dark { 0.14 } else { 0.10 }).into()),
            (false, button::Status::Hovered) => Some(p.hover.into()),
            (false, button::Status::Pressed) => Some(p.pressed.into()),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: p.text,
            border: Border {
                color: if selected {
                    alpha(p.accent, 0.45)
                } else {
                    Color::TRANSPARENT
                },
                width: 1.0,
                radius: R_MD.into(),
            },
            ..button::Style::default()
        }
    })
}

/// A table row: flat, hover only.
pub fn table_row(p: &'static Pal) -> B {
    Box::new(move |_, status| button::Style {
        background: match status {
            button::Status::Hovered | button::Status::Pressed => Some(p.hover.into()),
            _ => None,
        },
        text_color: p.text,
        ..button::Style::default()
    })
}

/// One option of a segmented control.
pub fn segment(p: &'static Pal, selected: bool) -> B {
    Box::new(move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(p.raised.into()),
            (false, button::Status::Hovered) => Some(p.hover.into()),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: if selected { p.text } else { p.subtext },
            border: Border {
                color: if selected {
                    p.border
                } else {
                    Color::TRANSPARENT
                },
                width: 1.0,
                radius: R_SM.into(),
            },
            shadow: Shadow {
                color: alpha(p.shadow, if selected { 0.25 } else { 0.0 }),
                offset: Vector::new(0.0, 1.0),
                blur_radius: 2.0,
            },
            ..button::Style::default()
        }
    })
}

/// The track a segmented control sits in.
pub fn segments(p: &'static Pal) -> C {
    Box::new(move |_| container::Style {
        background: Some(if p.dark { p.rail } else { p.pressed }.into()),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: R_MD.into(),
        },
        ..container::Style::default()
    })
}

/// A settings section in the settings sub-navigation.
pub fn subnav(p: &'static Pal, selected: bool) -> B {
    Box::new(move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(p.hover.into()),
            (false, button::Status::Hovered) => Some(alpha(p.hover, 0.6).into()),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: if selected { p.text } else { p.subtext },
            border: Border {
                radius: R_MD.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

pub fn input(p: &'static Pal) -> I {
    Box::new(move |_, status| {
        let edge = match status {
            text_input::Status::Focused { .. } => p.accent,
            text_input::Status::Hovered => p.faint,
            _ => p.border,
        };
        text_input::Style {
            background: if p.dark { p.rail } else { p.raised }.into(),
            border: Border {
                color: edge,
                width: if matches!(status, text_input::Status::Focused { .. }) {
                    1.5
                } else {
                    1.0
                },
                radius: R_MD.into(),
            },
            icon: p.faint,
            placeholder: p.faint,
            value: p.text,
            selection: alpha(p.accent, 0.35),
        }
    })
}

pub fn switch(p: &'static Pal) -> S {
    Box::new(move |_, status| {
        let on = matches!(
            status,
            toggler::Status::Active { is_toggled: true }
                | toggler::Status::Hovered { is_toggled: true }
        );
        toggler::Style {
            background: if on {
                gloss(p.success)
            } else {
                Background::Color(p.pressed)
            },
            background_border_width: 1.0,
            background_border_color: if on {
                mix(p.success, Color::BLACK, 0.2)
            } else {
                p.border
            },
            foreground: Background::Color(Color::WHITE),
            foreground_border_width: 1.0,
            foreground_border_color: alpha(Color::BLACK, 0.15),
            text_color: Some(p.text),
            border_radius: None,
            padding_ratio: 0.12,
        }
    })
}

pub fn progress(p: &'static Pal, tint: Color) -> Box<dyn Fn(&Theme) -> progress_bar::Style> {
    Box::new(move |_| progress_bar::Style {
        background: Background::Color(if p.dark { p.rail } else { p.pressed }),
        bar: gloss(tint),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::new(999.0),
        },
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The Aero glass edge, toned down for an enterprise look: a slightly
/// brighter top half with a soft step at the middle.
pub fn gloss(base: Color) -> Background {
    Linear::new(Degrees(180.0))
        .add_stop(0.0, mix(base, Color::WHITE, 0.16))
        .add_stop(0.49, mix(base, Color::WHITE, 0.06))
        .add_stop(0.51, base)
        .add_stop(1.0, mix(base, Color::BLACK, 0.06))
        .into()
}

/// An almost flat surface with a hint of the glass edge at the top.
fn sheen(base: Color, dark: bool) -> Background {
    let lift = if dark { 0.035 } else { 0.5 };
    Linear::new(Degrees(180.0))
        .add_stop(0.0, mix(base, Color::WHITE, lift))
        .add_stop(1.0, base)
        .into()
}

pub fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}
