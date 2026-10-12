//! "Aero Modern": Windows 7 glass and gloss on a modern layout.
//!
//! The counterpart of MediGuard's `classic-dark.css`. The two techniques it
//! leans on map like this:
//!
//! * **Layered backgrounds.** JavaFX stacks two gradients, a white gloss over
//!   a coloured base:
//!   ```css
//!   -fx-background-color:
//!       linear-gradient(to bottom, rgba(255,255,255,0.28) 0%, rgba(255,255,255,0.02) 48%, transparent 52%),
//!       linear-gradient(to bottom, #333c46 0%, #242b32 100%);
//!   ```
//!   An iced widget has one background, but a gradient has up to 8 stops, so
//!   both layers fold into one gradient with a hard step at the middle: the
//!   classic Aero "glass edge" (see [`gloss`]).
//! * **`dropshadow(...)`** is `Shadow`; a coloured shadow with a large blur is
//!   the Aero hover **glow**. **`innershadow(...)`** has no iced equivalent;
//!   a pressed button flips its gradient instead.

use iced::border::Radius;
use iced::gradient::Linear;
use iced::widget::{button, container, progress_bar, text_input, toggler};
use iced::{Background, Border, Color, Degrees, Shadow, Theme, Vector, color};

// ---------------------------------------------------------------------------
// Colours (CSS: the looked-up colours in `* { ... }`)
// ---------------------------------------------------------------------------

pub const TEXT: Color = Color::WHITE;
pub const DIM: Color = color!(0xb9d3ee);
pub const SKY: Color = color!(0x2f95e6);
pub const SKY_DEEP: Color = color!(0x1d74c9);
pub const SKY_LIGHT: Color = color!(0x8fd0ff);
pub const LEAF: Color = color!(0x3fae2a);
pub const LEAF_LIGHT: Color = color!(0xb4f59a);
pub const EMBER: Color = color!(0xc8371c);
pub const EMBER_LIGHT: Color = color!(0xf6a48f);
pub const AMBER: Color = color!(0xf0b232);

pub fn theme() -> Theme {
    Theme::custom(
        "aero",
        iced::theme::Palette {
            background: color!(0x0b2547),
            text: TEXT,
            primary: SKY,
            success: LEAF,
            warning: AMBER,
            danger: EMBER,
        },
    )
}

// ---------------------------------------------------------------------------
// Gradients
// ---------------------------------------------------------------------------

/// The Aero gloss: bright at the top, a hard step halfway down, then the
/// colour deepening again towards the bottom.
pub fn gloss(light: Color, base: Color, deep: Color) -> Background {
    Linear::new(Degrees(180.0))
        .add_stop(0.0, light)
        .add_stop(0.48, mix(light, base, 0.55))
        .add_stop(0.5, deep)
        .add_stop(1.0, base)
        .into()
}

/// Frosted glass: a translucent white sheen, brighter at the top.
fn glass(top: f32, bottom: f32) -> Background {
    Linear::new(Degrees(180.0))
        .add_stop(0.0, white(top))
        .add_stop(0.5, white(top * 0.55))
        .add_stop(0.501, white(bottom))
        .add_stop(1.0, white(bottom * 0.6))
        .into()
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

/// The custom title bar: the strongest glass, like Win7's window frame.
pub fn title_bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(
            Linear::new(Degrees(180.0))
                .add_stop(
                    0.0,
                    Color {
                        a: 0.55,
                        ..color!(0x6fb8f0)
                    },
                )
                .add_stop(
                    0.5,
                    Color {
                        a: 0.35,
                        ..color!(0x2c6fb5)
                    },
                )
                .add_stop(
                    0.51,
                    Color {
                        a: 0.45,
                        ..color!(0x173e70)
                    },
                )
                .add_stop(
                    1.0,
                    Color {
                        a: 0.55,
                        ..color!(0x10305a)
                    },
                )
                .into(),
        ),
        border: Border {
            color: white(0.25),
            width: 1.0,
            radius: Radius::default(),
        },
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

/// ```css
/// .panel {
///     -fx-background-color: <glass>;
///     -fx-background-radius: 12;
///     -fx-border-color: rgba(255,255,255,0.22);
///     -fx-effect: dropshadow(gaussian, rgba(0,0,0,0.45), 18, 0, 0, 6);
/// }
/// ```
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(glass(0.14, 0.07)),
        border: Border {
            color: white(0.22),
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: black(0.45),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

/// A card inside a panel: lighter glass, smaller corners.
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(glass(0.12, 0.05)),
        border: Border {
            color: white(0.18),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: black(0.30),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 6.0,
        },
        ..container::Style::default()
    }
}

/// A dialog: solid (iced cannot blur what is behind it, so see-through
/// glass would show the text underneath), glossy caption, blue glow.
pub fn dialog(_: &Theme) -> container::Style {
    container::Style {
        background: Some(
            Linear::new(Degrees(180.0))
                .add_stop(
                    0.0,
                    Color {
                        a: 1.0,
                        ..color!(0x2a5f99)
                    },
                )
                .add_stop(
                    0.18,
                    Color {
                        a: 1.0,
                        ..color!(0x1c4677)
                    },
                )
                .add_stop(
                    0.181,
                    Color {
                        a: 1.0,
                        ..color!(0x112f55)
                    },
                )
                .add_stop(
                    1.0,
                    Color {
                        a: 1.0,
                        ..color!(0x0c2442)
                    },
                )
                .into(),
        ),
        border: Border {
            color: Color {
                a: 0.8,
                ..SKY_LIGHT
            },
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: Color { a: 0.55, ..SKY },
            offset: Vector::ZERO,
            blur_radius: 40.0,
        },
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

/// The dim veil behind a dialog; `alpha` follows the fade-in.
pub fn veil(alpha: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(
            Color {
                a: 0.55 * alpha,
                ..color!(0x020a18)
            }
            .into(),
        ),
        ..container::Style::default()
    }
}

/// A glossy round avatar in the friend's colour.
pub fn orb(tint: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(gloss(
            mix(Color::WHITE, tint, 0.45),
            tint,
            darken(tint, 0.25),
        )),
        border: Border {
            color: black(0.45),
            width: 1.0,
            radius: Radius::new(999.0),
        },
        shadow: Shadow {
            color: Color { a: 0.6, ..tint },
            offset: Vector::ZERO,
            blur_radius: 8.0,
        },
        text_color: Some(Color::WHITE),
        ..container::Style::default()
    }
}

/// The red count on the Pending tab: a small glossy pill.
pub fn badge(_: &Theme) -> container::Style {
    container::Style {
        background: Some(gloss(EMBER_LIGHT, color!(0xe0562f), EMBER)),
        border: Border {
            color: black(0.4),
            width: 1.0,
            radius: Radius::new(999.0),
        },
        text_color: Some(Color::WHITE),
        ..container::Style::default()
    }
}

/// The Receiving banner: a green or amber glass strip that glows.
pub fn banner(on: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let edge = if on { LEAF_LIGHT } else { AMBER };
        container::Style {
            background: Some(
                Linear::new(Degrees(180.0))
                    .add_stop(0.0, Color { a: 0.30, ..edge })
                    .add_stop(0.5, Color { a: 0.14, ..edge })
                    .add_stop(0.51, Color { a: 0.08, ..edge })
                    .add_stop(1.0, Color { a: 0.12, ..edge })
                    .into(),
            ),
            border: Border {
                color: Color { a: 0.8, ..edge },
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: Shadow {
                color: Color { a: 0.35, ..edge },
                offset: Vector::ZERO,
                blur_radius: 16.0,
            },
            ..container::Style::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Buttons: the signature glossy look, with a glow on hover
//
//     .button:hover { ... -fx-effect: dropshadow(gaussian, #8fd0ff, 14, 0.3, 0, 0); }
//     .button:armed { ... innershadow(...); }
// ---------------------------------------------------------------------------

pub fn sky(_: &Theme, status: button::Status) -> button::Style {
    glossy(SKY_LIGHT, SKY, SKY_DEEP, status)
}

pub fn leaf(_: &Theme, status: button::Status) -> button::Style {
    glossy(LEAF_LIGHT, color!(0x48c234), color!(0x2e8a1f), status)
}

pub fn ember(_: &Theme, status: button::Status) -> button::Style {
    glossy(EMBER_LIGHT, color!(0xe0562f), EMBER, status)
}

/// A clear glass button: the "neutral" choice next to a coloured one.
pub fn clear(_: &Theme, status: button::Status) -> button::Style {
    let (top, bottom, glow) = match status {
        button::Status::Hovered => (0.34, 0.14, 0.5),
        button::Status::Pressed => (0.08, 0.20, 0.0),
        button::Status::Disabled => (0.06, 0.03, 0.0),
        button::Status::Active => (0.22, 0.08, 0.0),
    };
    button::Style {
        background: Some(glass(top, bottom)),
        text_color: TEXT,
        border: Border {
            color: white(0.35),
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow {
            color: Color {
                a: glow,
                ..SKY_LIGHT
            },
            offset: Vector::ZERO,
            blur_radius: 12.0,
        },
        ..button::Style::default()
    }
}

/// A tab: invisible until hovered, glossy blue glass when chosen.
pub fn tab(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        if selected {
            return glossy(
                Color {
                    a: 0.75,
                    ..SKY_LIGHT
                },
                Color { a: 0.55, ..SKY },
                Color {
                    a: 0.65,
                    ..SKY_DEEP
                },
                status,
            );
        }
        match status {
            button::Status::Hovered | button::Status::Pressed => clear(theme, status),
            _ => button::Style {
                text_color: DIM,
                ..button::Style::default()
            },
        }
    }
}

/// A friend in the sidebar: Windows Explorer's light-blue selection.
pub fn item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let strength = match (selected, status) {
            (true, _) => 1.0,
            (false, button::Status::Hovered | button::Status::Pressed) => 0.55,
            _ => 0.0,
        };
        if strength == 0.0 {
            return button::Style {
                text_color: TEXT,
                ..button::Style::default()
            };
        }
        button::Style {
            background: Some(
                Linear::new(Degrees(180.0))
                    .add_stop(
                        0.0,
                        Color {
                            a: 0.45 * strength,
                            ..SKY_LIGHT
                        },
                    )
                    .add_stop(
                        1.0,
                        Color {
                            a: 0.25 * strength,
                            ..SKY
                        },
                    )
                    .into(),
            ),
            text_color: TEXT,
            border: Border {
                color: Color {
                    a: 0.8 * strength,
                    ..SKY_LIGHT
                },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// The title bar's minimise and maximise buttons.
pub fn caption(_: &Theme, status: button::Status) -> button::Style {
    let a = match status {
        button::Status::Hovered => 0.35,
        button::Status::Pressed => 0.15,
        _ => 0.0,
    };
    button::Style {
        background: Some(glass(a, a * 0.5)),
        text_color: TEXT,
        border: Border {
            color: white(a),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}

/// Win7's red close button: glossy red, glowing on hover.
pub fn close(theme: &Theme, status: button::Status) -> button::Style {
    let style = ember(theme, status);
    button::Style {
        border: Border {
            radius: 4.0.into(),
            ..style.border
        },
        ..style
    }
}

// ---------------------------------------------------------------------------
// Other widgets
// ---------------------------------------------------------------------------

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let edge = match status {
        text_input::Status::Focused { .. } => SKY_LIGHT,
        text_input::Status::Hovered => white(0.45),
        _ => white(0.25),
    };
    text_input::Style {
        background: Linear::new(Degrees(180.0))
            .add_stop(0.0, black(0.45))
            .add_stop(1.0, black(0.25))
            .into(),
        border: Border {
            color: edge,
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: DIM,
        placeholder: Color { a: 0.7, ..DIM },
        value: TEXT,
        selection: Color { a: 0.45, ..SKY },
    }
}

/// The Receiving switch: a glossy green track when on.
pub fn switch(_: &Theme, status: toggler::Status) -> toggler::Style {
    let on = matches!(
        status,
        toggler::Status::Active { is_toggled: true }
            | toggler::Status::Hovered { is_toggled: true }
    );
    toggler::Style {
        background: if on {
            gloss(LEAF_LIGHT, color!(0x48c234), color!(0x2e8a1f))
        } else {
            gloss(color!(0x9aa7b5), color!(0x4d5866), color!(0x3a4350))
        },
        background_border_width: 1.0,
        background_border_color: black(0.5),
        foreground: gloss(Color::WHITE, color!(0xe4ecf5), color!(0xc9d5e2)),
        foreground_border_width: 1.0,
        foreground_border_color: black(0.35),
        text_color: Some(TEXT),
        border_radius: None,
        padding_ratio: 0.1,
    }
}

/// Win7's green progress bar, gloss and all.
pub fn progress(_: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Linear::new(Degrees(180.0))
            .add_stop(0.0, black(0.5))
            .add_stop(1.0, black(0.25))
            .into(),
        bar: gloss(LEAF_LIGHT, color!(0x48c234), color!(0x2e8a1f)),
        border: Border {
            color: black(0.5),
            width: 1.0,
            radius: 5.0.into(),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers (CSS: derive(), rgba())
// ---------------------------------------------------------------------------

/// A filled glossy button that brightens and glows on hover, and turns its
/// gloss upside down when pressed.
fn glossy(light: Color, base: Color, deep: Color, status: button::Status) -> button::Style {
    let (background, glow) = match status {
        button::Status::Active => (gloss(light, base, deep), 0.0),
        button::Status::Hovered => (gloss(lighten(light, 0.2), lighten(base, 0.15), deep), 0.85),
        button::Status::Pressed => (gloss(deep, darken(base, 0.1), darken(deep, 0.2)), 0.0),
        button::Status::Disabled => (
            gloss(
                Color { a: 0.35, ..light },
                Color { a: 0.35, ..base },
                Color { a: 0.35, ..deep },
            ),
            0.0,
        ),
    };
    button::Style {
        background: Some(background),
        text_color: Color::WHITE,
        border: Border {
            color: black(0.55),
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow {
            color: Color { a: glow, ..light },
            offset: Vector::ZERO,
            blur_radius: 14.0,
        },
        ..button::Style::default()
    }
}

pub fn white(a: f32) -> Color {
    Color { a, ..Color::WHITE }
}

pub fn black(a: f32) -> Color {
    Color { a, ..Color::BLACK }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn lighten(c: Color, by: f32) -> Color {
    mix(
        c,
        Color {
            a: c.a,
            ..Color::WHITE
        },
        by,
    )
}

fn darken(c: Color, by: f32) -> Color {
    mix(
        c,
        Color {
            a: c.a,
            ..Color::BLACK
        },
        by,
    )
}
