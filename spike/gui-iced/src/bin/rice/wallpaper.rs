//! A ricer-style wallpaper drawn in code: soft blurred colour blobs from the
//! current scheme over its darkest shade, and a faint dot grid. Redrawn when
//! the scheme changes.

use iced::widget::canvas::{self, Cache, Geometry, Path, gradient};
use iced::{Color, Point, Rectangle, Renderer, Theme, mouse};

use crate::style::{Pal, alpha};

pub struct Wallpaper {
    pal: &'static Pal,
    cache: Cache,
}

impl Wallpaper {
    pub fn new(pal: &'static Pal) -> Self {
        Self {
            pal,
            cache: Cache::new(),
        }
    }
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
        let p = self.pal;
        let picture = self.cache.draw(renderer, bounds.size(), |frame| {
            let (w, h) = (frame.width(), frame.height());
            frame.fill_rectangle(
                Point::ORIGIN,
                frame.size(),
                gradient::Linear::new(Point::ORIGIN, Point::new(0.0, h))
                    .add_stop(0.0, p.crust)
                    .add_stop(1.0, p.mantle),
            );

            // Blobs: many faint rings, which read as one blurred shape.
            for (x, y, r, tint) in [
                (0.15, 0.20, 0.45, p.accent),
                (0.85, 0.25, 0.40, p.accent2),
                (0.70, 0.95, 0.50, p.pink),
                (0.25, 0.90, 0.30, p.teal),
            ] {
                let radius = r * w.max(h);
                for i in 0..24 {
                    let k = i as f32 / 24.0;
                    frame.fill(
                        &Path::circle(Point::new(w * x, h * y), radius * (1.0 - k)),
                        alpha(tint, 0.012 + 0.006 * k),
                    );
                }
            }

            // The dot grid many tiling setups show behind their windows.
            let step = 28.0;
            let mut y = step / 2.0;
            while y < h {
                let mut x = step / 2.0;
                while x < w {
                    frame.fill(
                        &Path::circle(Point::new(x, y), 1.0),
                        Color { a: 0.06, ..p.text },
                    );
                    x += step;
                }
                y += step;
            }
        });
        vec![picture]
    }
}
