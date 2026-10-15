//! The "stylesheet". In JavaFX this would be `beam.css`; in iced a style is a
//! plain Rust function that returns a `Style` struct. A widget picks one with
//! `.style(style::card)`, much as a JavaFX node picks one with
//! `getStyleClass().add("card")`.
//!
//! Each function shows the JavaFX CSS it corresponds to.

use iced::border::Radius;
use iced::widget::{button, container, progress_bar, text_input, toggler};
use iced::{Background, Border, Color, Shadow, Theme, Vector, color};

// ---------------------------------------------------------------------------
// Colours: the equivalent of CSS "looked-up colours" in `.root { ... }`.
//
//     .root {
//         -beam-accent:  #5865f2;
//         -beam-success: #23a55a;
//         ...
//     }
// ---------------------------------------------------------------------------

pub struct Colors {
    pub rail: Color,
    pub sidebar: Color,
    pub page: Color,
    pub card: Color,
    pub hover: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub success: Color,
    pub danger: Color,
    pub warning: Color,
}

pub const DARK: Colors = Colors {
    rail: color!(0x1e1f22),
    sidebar: color!(0x2b2d31),
    page: color!(0x313338),
    card: color!(0x383a40),
    hover: color!(0x404249),
    text: color!(0xf2f3f5),
    muted: color!(0x949ba4),
    accent: color!(0x5865f2),
    success: color!(0x23a55a),
    danger: color!(0xda373c),
    warning: color!(0xf0b232),
};

pub const LIGHT: Colors = Colors {
    rail: color!(0xe3e5e8),
    sidebar: color!(0xf2f3f5),
    page: color!(0xffffff),
    card: color!(0xf6f6f7),
    hover: color!(0xe8e9ec),
    text: color!(0x060607),
    muted: color!(0x5c5e66),
    accent: color!(0x5865f2),
    success: color!(0x1f8b4c),
    danger: color!(0xd83c3e),
    warning: color!(0xc27c0e),
};

/// Which palette the current theme uses. The theme's own background tells
/// light and dark apart, so styles need no extra argument.
pub fn colors(theme: &Theme) -> &'static Colors {
    if theme.palette().background == DARK.page {
        &DARK
    } else {
        &LIGHT
    }
}

/// The app theme built from our colours: iced's built-in widgets (text
/// inputs, scrollbars) pick their colours from here.
pub fn theme(dark: bool) -> Theme {
    let c = if dark { &DARK } else { &LIGHT };
    Theme::custom(
        if dark { "beam dark" } else { "beam light" },
        iced::theme::Palette {
            background: c.page,
            text: c.text,
            primary: c.accent,
            success: c.success,
            warning: c.warning,
            danger: c.danger,
        },
    )
}

// ---------------------------------------------------------------------------
// Containers (JavaFX: Pane / VBox / HBox with a style class)
// ---------------------------------------------------------------------------

/// `.rail { -fx-background-color: -beam-rail; }`
pub fn rail(theme: &Theme) -> container::Style {
    filled(colors(theme).rail)
}

/// `.sidebar { -fx-background-color: -beam-sidebar; }`
pub fn sidebar(theme: &Theme) -> container::Style {
    filled(colors(theme).sidebar)
}

/// `.page { -fx-background-color: -beam-page; }`
pub fn page(theme: &Theme) -> container::Style {
    filled(colors(theme).page)
}

/// ```css
/// .card {
///     -fx-background-color: -beam-card;
///     -fx-background-radius: 10;
/// }
/// ```
pub fn card(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(colors(theme).card.into()),
        border: Border {
            radius: 10.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// ```css
/// .dialog {
///     -fx-background-color: -beam-page;
///     -fx-background-radius: 14;
///     -fx-effect: dropshadow(gaussian, rgba(0,0,0,0.5), 30, 0, 0, 8);
/// }
/// ```
pub fn dialog(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(colors(theme).page.into()),
        border: Border {
            radius: 14.0.into(),
            ..Border::default()
        },
        shadow: Shadow {
            color: Color {
                a: 0.5,
                ..Color::BLACK
            },
            offset: Vector::new(0.0, 8.0),
            blur_radius: 30.0,
        },
        ..container::Style::default()
    }
}

/// The dark veil behind a dialog: `.veil { -fx-background-color: rgba(0,0,0,0.6); }`
pub fn veil(_theme: &Theme) -> container::Style {
    filled(Color {
        a: 0.6,
        ..Color::BLACK
    })
}

/// A round avatar with the friend's colour:
/// `.avatar { -fx-background-color: <colour>; -fx-background-radius: 50%; }`
pub fn avatar(fill: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(fill.into()),
        text_color: Some(Color::WHITE),
        border: Border {
            radius: Radius::new(999.0),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// The small red number on the Pending tab.
pub fn badge(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(colors(theme).danger.into()),
        text_color: Some(Color::WHITE),
        border: Border {
            radius: Radius::new(999.0),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// The Receiving banner: green border when on, amber when off.
pub fn banner(on: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let c = colors(theme);
        let edge = if on { c.success } else { c.warning };
        container::Style {
            background: Some(Color { a: 0.12, ..edge }.into()),
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: edge,
            },
            ..container::Style::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Buttons. JavaFX needs a rule per pseudo-class:
//
//     .button-primary         { -fx-background-color: -beam-accent; }
//     .button-primary:hover   { -fx-background-color: derive(-beam-accent, -10%); }
//     .button-primary:pressed { -fx-background-color: derive(-beam-accent, -20%); }
//
// In iced the same function gets the `Status` and decides; `:hover`,
// `:pressed` and `:disabled` are just the arms of a `match`.
// ---------------------------------------------------------------------------

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    solid(colors(theme).accent, status)
}

pub fn success(theme: &Theme, status: button::Status) -> button::Style {
    solid(colors(theme).success, status)
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    solid(colors(theme).danger, status)
}

/// A grey button: `.button-secondary { -fx-background-color: -beam-hover; }`
pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let c = colors(theme);
    button::Style {
        text_color: c.text,
        ..solid(c.hover, status)
    }
}

/// A tab in the header; the selected one is filled.
pub fn tab(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let c = colors(theme);
        let fill = match (selected, status) {
            (true, _) => Some(c.hover),
            (false, button::Status::Hovered) => Some(Color { a: 0.5, ..c.hover }),
            _ => None,
        };
        button::Style {
            background: fill.map(Background::Color),
            text_color: if selected { c.text } else { c.muted },
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// A row in a list (a friend in the sidebar): transparent until hovered.
pub fn row_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let c = colors(theme);
        let fill = match (selected, status) {
            (true, _) => Some(c.hover),
            (false, button::Status::Hovered | button::Status::Pressed) => {
                Some(Color { a: 0.6, ..c.hover })
            }
            _ => None,
        };
        button::Style {
            background: fill.map(Background::Color),
            text_color: c.text,
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// A round icon button in the left rail.
pub fn rail_button(theme: &Theme, status: button::Status) -> button::Style {
    let c = colors(theme);
    let (fill, radius) = match status {
        button::Status::Hovered | button::Status::Pressed => (c.accent, 16.0),
        _ => (c.page, 24.0),
    };
    button::Style {
        background: Some(fill.into()),
        text_color: c.text,
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

// ---------------------------------------------------------------------------
// Other widgets
// ---------------------------------------------------------------------------

/// ```css
/// .text-field {
///     -fx-background-color: -beam-rail;
///     -fx-background-radius: 6;
///     -fx-prompt-text-fill: -beam-muted;
/// }
/// .text-field:focused { -fx-border-color: -beam-accent; }
/// ```
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let c = colors(theme);
    let edge = match status {
        text_input::Status::Focused { .. } => c.accent,
        _ => Color::TRANSPARENT,
    };
    text_input::Style {
        background: c.rail.into(),
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: edge,
        },
        icon: c.muted,
        placeholder: c.muted,
        value: c.text,
        selection: Color { a: 0.4, ..c.accent },
    }
}

/// The Receiving switch: green when on, grey when off.
pub fn switch(theme: &Theme, status: toggler::Status) -> toggler::Style {
    let c = colors(theme);
    let on = matches!(
        status,
        toggler::Status::Active { is_toggled: true }
            | toggler::Status::Hovered { is_toggled: true }
    );
    toggler::Style {
        background: (if on { c.success } else { c.muted }).into(),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Color::WHITE.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(c.text),
        border_radius: None,
        padding_ratio: 0.1,
    }
}

/// ```css
/// .progress-bar > .bar   { -fx-background-color: -beam-accent; }
/// .progress-bar > .track { -fx-background-color: -beam-rail; }
/// ```
pub fn progress(theme: &Theme) -> progress_bar::Style {
    let c = colors(theme);
    progress_bar::Style {
        background: c.rail.into(),
        bar: c.accent.into(),
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn filled(fill: Color) -> container::Style {
    container::Style {
        background: Some(fill.into()),
        ..container::Style::default()
    }
}

/// A filled button that darkens on hover and press, and fades when disabled.
fn solid(fill: Color, status: button::Status) -> button::Style {
    let fill = match status {
        button::Status::Active => fill,
        button::Status::Hovered => darken(fill, 0.1),
        button::Status::Pressed => darken(fill, 0.2),
        button::Status::Disabled => Color { a: 0.4, ..fill },
    };
    button::Style {
        background: Some(fill.into()),
        text_color: Color::WHITE,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// CSS `derive(color, -10%)`.
fn darken(c: Color, by: f32) -> Color {
    Color {
        r: c.r * (1.0 - by),
        g: c.g * (1.0 - by),
        b: c.b * (1.0 - by),
        a: c.a,
    }
}
