//! The background behind the glass, drawn by hand on a canvas: a deep blue
//! gradient with soft light "swooshes", in the spirit of Windows 7's Harmony
//! wallpaper. JavaFX equivalent: a `Canvas` and its `GraphicsContext`.
//!
//! Real glass blurs whatever is behind the window. iced cannot blur the
//! desktop on Windows, so the glass blurs nothing and this picture is what
//! shows through it.

use iced::widget::canvas::{self, Cache, Geometry, Path, gradient};
use iced::{Color, Point, Rectangle, Renderer, Theme, mouse};

#[derive(Default)]
pub struct Wallpaper {
    cache: Cache,
}

impl<Message> canvas::Program<Message> for Wallpaper {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let picture = self.cache.draw(renderer, bounds.size(), |frame| {
            let (w, h) = (frame.width(), frame.height());

            // The sky: navy at the top left to teal at the bottom right.
            frame.fill_rectangle(
                Point::ORIGIN,
                frame.size(),
                gradient::Linear::new(Point::ORIGIN, Point::new(w, h))
                    .add_stop(0.0, rgb(0x06122b))
                    .add_stop(0.45, rgb(0x0b3a6e))
                    .add_stop(1.0, rgb(0x0f6e8c)),
            );

            // A wide glow low on the right, made of rings fading outwards.
            for i in 0..18 {
                let r = 40.0 + i as f32 * 28.0;
                let alpha = 0.035 * (1.0 - i as f32 / 18.0);
                frame.fill(
                    &Path::circle(Point::new(w * 0.78, h * 0.86), r),
                    Color {
                        a: alpha,
                        ..rgb(0x7fe3ff)
                    },
                );
            }

            // Three swooshes of light across the picture.
            swoosh(frame, w, h, 0.62, 0.10, 0.16, rgb(0x9fd8ff));
            swoosh(frame, w, h, 0.70, 0.06, 0.12, rgb(0x6fffe0));
            swoosh(frame, w, h, 0.50, 0.04, 0.10, Color::WHITE);
        });
        vec![picture]
    }
}

/// A curved band from the left edge to the right one; `y` is where it
/// crosses the middle (as a share of the height), `thick` its thickness.
fn swoosh(frame: &mut canvas::Frame, w: f32, h: f32, y: f32, thick: f32, alpha: f32, color: Color) {
    let band = Path::new(|p| {
        p.move_to(Point::new(0.0, h * (y + 0.25)));
        p.bezier_curve_to(
            Point::new(w * 0.35, h * (y - 0.05)),
            Point::new(w * 0.65, h * (y + 0.20)),
            Point::new(w, h * (y - 0.20)),
        );
        p.line_to(Point::new(w, h * (y - 0.20 + thick)));
        p.bezier_curve_to(
            Point::new(w * 0.65, h * (y + 0.20 + thick)),
            Point::new(w * 0.35, h * (y - 0.05 + thick)),
            Point::new(0.0, h * (y + 0.25 + thick)),
        );
        p.close();
    });
    frame.fill(
        &band,
        gradient::Linear::new(Point::new(0.0, 0.0), Point::new(w, 0.0))
            .add_stop(0.0, Color { a: 0.0, ..color })
            .add_stop(0.5, Color { a: alpha, ..color })
            .add_stop(1.0, Color { a: 0.0, ..color }),
    );
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
