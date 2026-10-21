//! "Riced": a Linux-ricer look (Hyprland tiles, waybar pills, pastel
//! palettes) blended with a soft Aero gloss.
//!
//! Every style takes the current [`Pal`] (colour scheme), so switching from
//! Catppuccin to Gruvbox is just handing the view a different `&Pal`. In
//! JavaFX this would be swapping one stylesheet for another at runtime
//! (`scene.getStylesheets().setAll(...)`).

use iced::border::Radius;
use iced::gradient::Linear;
use iced::widget::{button, container, progress_bar, text_input, toggler};
use iced::{Background, Border, Color, Degrees, Shadow, Theme, Vector, color};

/// A colour scheme, named the way ricers name them.
pub struct Pal {
    pub name: &'static str,
    pub crust: Color,
    pub mantle: Color,
    pub base: Color,
    pub surface: Color,
    pub surface2: Color,
    pub overlay: Color,
    pub text: Color,
    pub subtext: Color,
    pub accent: Color,
    pub accent2: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub peach: Color,
    pub pink: Color,
    pub teal: Color,
}

impl Pal {
    /// The friend avatar colours, from the scheme itself.
    pub fn tints(&self) -> [Color; 6] {
        [
            self.pink,
            self.green,
            self.peach,
            self.accent2,
            self.red,
            self.accent,
        ]
    }
}

pub const PALETTES: [Pal; 5] = [
    Pal {
        name: "catppuccin",
        crust: color!(0x11111b),
        mantle: color!(0x181825),
        base: color!(0x1e1e2e),
        surface: color!(0x313244),
        surface2: color!(0x45475a),
        overlay: color!(0x6c7086),
        text: color!(0xcdd6f4),
        subtext: color!(0xa6adc8),
        accent: color!(0xcba6f7),
        accent2: color!(0x89b4fa),
        green: color!(0xa6e3a1),
        red: color!(0xf38ba8),
        yellow: color!(0xf9e2af),
        peach: color!(0xfab387),
        pink: color!(0xf5c2e7),
        teal: color!(0x94e2d5),
    },
    Pal {
        name: "tokyo-night",
        crust: color!(0x101014),
        mantle: color!(0x16161e),
        base: color!(0x1a1b26),
        surface: color!(0x292e42),
        surface2: color!(0x414868),
        overlay: color!(0x565f89),
        text: color!(0xc0caf5),
        subtext: color!(0xa9b1d6),
        accent: color!(0x7aa2f7),
        accent2: color!(0xbb9af7),
        green: color!(0x9ece6a),
        red: color!(0xf7768e),
        yellow: color!(0xe0af68),
        peach: color!(0xff9e64),
        pink: color!(0xbb9af7),
        teal: color!(0x7dcfff),
    },
    Pal {
        name: "gruvbox",
        crust: color!(0x141617),
        mantle: color!(0x1d2021),
        base: color!(0x282828),
        surface: color!(0x3c3836),
        surface2: color!(0x504945),
        overlay: color!(0x7c6f64),
        text: color!(0xebdbb2),
        subtext: color!(0xd5c4a1),
        accent: color!(0xfabd2f),
        accent2: color!(0xfe8019),
        green: color!(0xb8bb26),
        red: color!(0xfb4934),
        yellow: color!(0xfabd2f),
        peach: color!(0xfe8019),
        pink: color!(0xd3869b),
        teal: color!(0x8ec07c),
    },
    Pal {
        name: "nord",
        crust: color!(0x1f232b),
        mantle: color!(0x242933),
        base: color!(0x2e3440),
        surface: color!(0x3b4252),
        surface2: color!(0x434c5e),
        overlay: color!(0x616e88),
        text: color!(0xeceff4),
        subtext: color!(0xd8dee9),
        accent: color!(0x88c0d0),
        accent2: color!(0x81a1c1),
        green: color!(0xa3be8c),
        red: color!(0xbf616a),
        yellow: color!(0xebcb8b),
        peach: color!(0xd08770),
        pink: color!(0xb48ead),
        teal: color!(0x8fbcbb),
    },
    Pal {
        name: "rose-pine",
        crust: color!(0x110f18),
        mantle: color!(0x16141f),
        base: color!(0x191724),
        surface: color!(0x26233a),
        surface2: color!(0x403d52),
        overlay: color!(0x6e6a86),
        text: color!(0xe0def4),
        subtext: color!(0x908caa),
        accent: color!(0xc4a7e7),
        accent2: color!(0xebbcba),
        green: color!(0x9ccfd8),
        red: color!(0xeb6f92),
        yellow: color!(0xf6c177),
        peach: color!(0xebbcba),
        pink: color!(0xeb6f92),
        teal: color!(0x9ccfd8),
    },
];

/// The iced theme for a scheme (what the built-in widgets fall back on).
pub fn theme(p: &Pal) -> Theme {
    Theme::custom(
        p.name,
        iced::theme::Palette {
            background: p.base,
            text: p.text,
            primary: p.accent,
            success: p.green,
            warning: p.yellow,
            danger: p.red,
        },
    )
}

type ContainerStyle = Box<dyn Fn(&Theme) -> container::Style>;
type ButtonStyle = Box<dyn Fn(&Theme, button::Status) -> button::Style>;
type InputStyle = Box<dyn Fn(&Theme, text_input::Status) -> text_input::Style>;
type SwitchStyle = Box<dyn Fn(&Theme, toggler::Status) -> toggler::Style>;

// ---------------------------------------------------------------------------
// Tiles: Hyprland windows. The focused one gets a gradient border and glows.
//
// iced borders are one colour, so the gradient border is a container with a
// gradient *background* and 2 px of padding, holding the tile itself:
//
//     hyprland.conf:  col.active_border = rgba(cba6f7ee) rgba(89b4faee) 45deg
// ---------------------------------------------------------------------------

/// The 2 px frame around a tile.
pub fn frame(p: &'static Pal, focused: bool) -> ContainerStyle {
    Box::new(move |_| {
        let background: Background = if focused {
            Linear::new(Degrees(135.0))
                .add_stop(0.0, p.accent)
                .add_stop(1.0, p.accent2)
                .into()
        } else {
            alpha(p.surface2, 0.7).into()
        };
        container::Style {
            background: Some(background),
            border: Border {
                radius: 14.0.into(),
                ..Border::default()
            },
            shadow: Shadow {
                color: if focused {
                    alpha(p.accent, 0.35)
                } else {
                    alpha(Color::BLACK, 0.4)
                },
                offset: Vector::new(0.0, if focused { 0.0 } else { 4.0 }),
                blur_radius: if focused { 26.0 } else { 14.0 },
            },
            ..container::Style::default()
        }
    })
}

/// The tile inside the frame: the scheme's base, slightly see-through, with
/// a faint gloss across the top.
pub fn tile(p: &'static Pal) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(
            Linear::new(Degrees(180.0))
                .add_stop(
                    0.0,
                    mix(alpha(p.base, 0.94), alpha(Color::WHITE, 0.94), 0.05),
                )
                .add_stop(0.12, alpha(p.base, 0.94))
                .add_stop(1.0, alpha(p.mantle, 0.94))
                .into(),
        ),
        border: Border {
            radius: 12.0.into(),
            ..Border::default()
        },
        text_color: Some(p.text),
        ..container::Style::default()
    })
}

/// The waybar strip: a floating pill-shaped bar.
pub fn bar(p: &'static Pal) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(alpha(p.crust, 0.82).into()),
        border: Border {
            radius: 14.0.into(),
            width: 1.0,
            color: alpha(p.surface2, 0.8),
        },
        shadow: Shadow {
            color: alpha(Color::BLACK, 0.4),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        },
        text_color: Some(p.text),
        ..container::Style::default()
    })
}

/// A waybar module: a small glossy pill of colour.
pub fn module(p: &'static Pal, tint: Color) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(gloss(tint)),
        border: Border {
            radius: Radius::new(999.0),
            width: 1.0,
            color: alpha(Color::WHITE, 0.25),
        },
        text_color: Some(p.crust),
        ..container::Style::default()
    })
}

/// A card inside a tile.
pub fn card(p: &'static Pal) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(
            Linear::new(Degrees(180.0))
                .add_stop(0.0, mix(p.surface, Color::WHITE, 0.04))
                .add_stop(1.0, p.surface)
                .into(),
        ),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(p.surface2, 0.6),
        },
        ..container::Style::default()
    })
}

/// A rounded-square avatar with a soft gloss (a "squircle", as in modern
/// icon themes, glossy as in Aero).
pub fn avatar(tint: Color) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(gloss(tint)),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(Color::WHITE, 0.3),
        },
        shadow: Shadow {
            color: alpha(tint, 0.35),
            offset: Vector::ZERO,
            blur_radius: 10.0,
        },
        text_color: Some(color!(0x11111b)),
        ..container::Style::default()
    })
}

/// A coloured status pill (the Receiving banner, outcome tags).
pub fn tag(tint: Color) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(alpha(tint, 0.14).into()),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: alpha(tint, 0.55),
        },
        text_color: Some(tint),
        ..container::Style::default()
    })
}

pub fn veil(t: f32) -> ContainerStyle {
    Box::new(move |_| container::Style {
        background: Some(alpha(Color::BLACK, 0.5 * t).into()),
        ..container::Style::default()
    })
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// The signature button: a pastel pill with a soft gloss, dark text, and a
/// glow in its own colour on hover.
pub fn pill(p: &'static Pal, tint: Color) -> ButtonStyle {
    Box::new(move |_, status| {
        let (background, glow) = match status {
            button::Status::Active => (gloss(tint), 0.0),
            button::Status::Hovered => (gloss(mix(tint, Color::WHITE, 0.12)), 0.7),
            button::Status::Pressed => (gloss_flipped(tint), 0.0),
            button::Status::Disabled => (alpha(p.surface2, 0.6).into(), 0.0),
        };
        button::Style {
            background: Some(background),
            text_color: if status == button::Status::Disabled {
                p.overlay
            } else {
                p.crust
            },
            border: Border {
                radius: Radius::new(999.0),
                width: 1.0,
                color: alpha(Color::WHITE, 0.28),
            },
            shadow: Shadow {
                color: alpha(tint, glow),
                offset: Vector::ZERO,
                blur_radius: 16.0,
            },
            snap: true,
        }
    })
}

/// An outlined pill: the quieter choice next to a filled one.
pub fn ghost(p: &'static Pal) -> ButtonStyle {
    Box::new(move |_, status| {
        let fill = match status {
            button::Status::Hovered => Some(alpha(p.surface2, 0.6)),
            button::Status::Pressed => Some(p.surface),
            _ => None,
        };
        button::Style {
            background: fill.map(Background::Color),
            text_color: p.text,
            border: Border {
                radius: Radius::new(999.0),
                width: 1.0,
                color: p.overlay,
            },
            ..button::Style::default()
        }
    })
}

/// A workspace button in the bar: a dot of text, a glossy pill when active.
pub fn workspace(p: &'static Pal, active: bool) -> ButtonStyle {
    Box::new(move |theme, status| {
        if active {
            return pill(p, p.accent)(theme, status);
        }
        let fill = matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then(|| alpha(p.surface2, 0.8).into());
        button::Style {
            background: fill,
            text_color: p.subtext,
            border: Border {
                radius: Radius::new(999.0),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
}

/// A friend in the list: a left accent bar when selected (as in many riced
/// file managers), a faint fill on hover.
pub fn item(p: &'static Pal, selected: bool) -> ButtonStyle {
    Box::new(move |_, status| {
        let fill = match (selected, status) {
            (true, _) => Some(
                Linear::new(Degrees(90.0))
                    .add_stop(0.0, alpha(p.accent, 0.28))
                    .add_stop(1.0, alpha(p.accent, 0.04))
                    .into(),
            ),
            (false, button::Status::Hovered | button::Status::Pressed) => {
                Some(alpha(p.surface, 0.8).into())
            }
            _ => None,
        };
        button::Style {
            background: fill,
            text_color: p.text,
            border: Border {
                radius: 8.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
}

/// The traffic-light window buttons in the bar.
pub fn light(tint: Color) -> ButtonStyle {
    Box::new(move |_, status| button::Style {
        background: Some(gloss(match status {
            button::Status::Hovered => mix(tint, Color::WHITE, 0.2),
            _ => tint,
        })),
        border: Border {
            radius: Radius::new(999.0),
            width: 1.0,
            color: alpha(Color::BLACK, 0.3),
        },
        shadow: Shadow {
            color: alpha(
                tint,
                if status == button::Status::Hovered {
                    0.8
                } else {
                    0.0
                },
            ),
            offset: Vector::ZERO,
            blur_radius: 10.0,
        },
        ..button::Style::default()
    })
}

// ---------------------------------------------------------------------------
// Other widgets
// ---------------------------------------------------------------------------

pub fn input(p: &'static Pal) -> InputStyle {
    Box::new(move |_, status| text_input::Style {
        background: p.crust.into(),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: match status {
                text_input::Status::Focused { .. } => p.accent,
                text_input::Status::Hovered => p.overlay,
                _ => p.surface2,
            },
        },
        icon: p.overlay,
        placeholder: p.overlay,
        value: p.text,
        selection: alpha(p.accent, 0.4),
    })
}

pub fn switch(p: &'static Pal) -> SwitchStyle {
    Box::new(move |_, status| {
        let on = matches!(
            status,
            toggler::Status::Active { is_toggled: true }
                | toggler::Status::Hovered { is_toggled: true }
        );
        toggler::Style {
            background: if on {
                gloss(p.green)
            } else {
                p.surface2.into()
            },
            background_border_width: 1.0,
            background_border_color: alpha(Color::WHITE, 0.15),
            foreground: gloss(if on { p.crust } else { p.overlay }),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(p.text),
            border_radius: None,
            padding_ratio: 0.12,
        }
    })
}

pub fn progress(p: &'static Pal) -> Box<dyn Fn(&Theme) -> progress_bar::Style> {
    Box::new(move |_| progress_bar::Style {
        background: p.crust.into(),
        bar: Linear::new(Degrees(90.0))
            .add_stop(0.0, p.accent)
            .add_stop(1.0, p.accent2)
            .into(),
        border: Border {
            radius: Radius::new(999.0),
            ..Border::default()
        },
    })
}

// ---------------------------------------------------------------------------
// Gloss: Aero's half-and-half highlight, toned down for pastel colours
// ---------------------------------------------------------------------------

pub fn gloss(tint: Color) -> Background {
    Linear::new(Degrees(180.0))
        .add_stop(0.0, mix(tint, Color::WHITE, 0.38))
        .add_stop(0.48, mix(tint, Color::WHITE, 0.14))
        .add_stop(0.52, tint)
        .add_stop(1.0, mix(tint, Color::BLACK, 0.06))
        .into()
}

fn gloss_flipped(tint: Color) -> Background {
    Linear::new(Degrees(180.0))
        .add_stop(0.0, mix(tint, Color::BLACK, 0.12))
        .add_stop(1.0, mix(tint, Color::WHITE, 0.10))
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
