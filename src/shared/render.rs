//! Shared drawing helpers for the image cards: shapes, gradients, images and text.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Point, PremultipliedColorU8, Shader, SpreadMode, Stroke, Transform,
};

const REGULAR: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Bold.ttf");

pub fn colour(rgb: u32, alpha: u8) -> Color {
    Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, alpha)
}

pub fn solid(rgb: u32, alpha: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(colour(rgb, alpha));
    paint.anti_alias = true;
    paint
}

/// A gradient between two colours from `start` to `end`.
fn linear(start: Point, end: Point, from: u32, to: u32, alpha: u8) -> Paint<'static> {
    let stops = vec![GradientStop::new(0.0, colour(from, alpha)), GradientStop::new(1.0, colour(to, alpha))];
    let shader = LinearGradient::new(start, end, stops, SpreadMode::Pad, Transform::identity())
        .unwrap_or(Shader::SolidColor(colour(from, alpha)));
    Paint { shader, anti_alias: true, ..Default::default() }
}

/// A left-to-right gradient over `x0..x1`.
pub fn gradient(x0: f32, x1: f32, from: u32, to: u32, alpha: u8) -> Paint<'static> {
    linear(Point::from_xy(x0, 0.0), Point::from_xy(x1, 0.0), from, to, alpha)
}

/// A top-to-bottom gradient over `y0..y1`.
pub fn vgradient(y0: f32, y1: f32, from: u32, to: u32, alpha: u8) -> Paint<'static> {
    linear(Point::from_xy(0.0, y0), Point::from_xy(0.0, y1), from, to, alpha)
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn fill(pixmap: &mut Pixmap, path: &Path, paint: &Paint, mask: Option<&Mask>) {
    pixmap.fill_path(path, paint, FillRule::Winding, Transform::identity(), mask);
}

pub fn stroke(pixmap: &mut Pixmap, path: &Path, paint: &Paint, width: f32) {
    let stroke = Stroke { width, ..Default::default() };
    pixmap.stroke_path(path, paint, &stroke, Transform::identity(), None);
}

/// A soft glow: wide, faint strokes underneath a crisp one.
pub fn glow(pixmap: &mut Pixmap, path: &Path, paint_at: impl Fn(u8) -> Paint<'static>, width: f32) {
    for (extra, alpha) in [(18.0, 10), (12.0, 18), (7.0, 32), (3.0, 70)] {
        stroke(pixmap, path, &paint_at(alpha), width + extra);
    }
    if width > 0.0 {
        stroke(pixmap, path, &paint_at(255), width);
    }
}

/// A mask covering `path`, for clipping drawing to it.
pub fn mask_for(path: &Path, width: u32, height: u32) -> Option<Mask> {
    let mut mask = Mask::new(width, height)?;
    mask.fill_path(path, FillRule::Winding, true, Transform::identity());
    Some(mask)
}

/// Draws a PNG scaled into a square box, optionally clipped to a circle.
pub fn draw_png(pixmap: &mut Pixmap, png: &[u8], x: f32, y: f32, size: f32, circle: bool) {
    let Ok(image) = Pixmap::decode_png(png) else { return };
    let transform = Transform::from_row(size / image.width() as f32, 0.0, 0.0, size / image.height() as f32, x, y);
    let mask = if circle {
        PathBuilder::from_circle(x + size / 2.0, y + size / 2.0, size / 2.0)
            .and_then(|p| mask_for(&p, pixmap.width(), pixmap.height()))
    } else {
        None
    };
    let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
    pixmap.draw_pixmap(0, 0, image.as_ref(), &paint, transform, mask.as_ref());
}

/// Draws an SVG scaled to fit a square box.
pub fn draw_svg(pixmap: &mut Pixmap, svg: &[u8], x: f32, y: f32, size: f32) {
    let Ok(tree) = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()) else { return };
    let svg_size = tree.size();
    let scale = size / svg_size.width().max(svg_size.height());
    let transform = Transform::from_row(scale, 0.0, 0.0, scale, x, y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
}

#[derive(Clone, Copy)]
pub enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Noto Sans Regular and Bold, built into the binary.
pub struct Fonts {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

impl Fonts {
    pub fn load() -> Option<Self> {
        Some(Self { regular: FontRef::try_from_slice(REGULAR).ok()?, bold: FontRef::try_from_slice(BOLD).ok()? })
    }

    fn font(&self, weight: Weight) -> &FontRef<'static> {
        match weight {
            Weight::Regular => &self.regular,
            Weight::Bold => &self.bold,
        }
    }

    /// Converts a CSS-style font size (the em size) to ab_glyph's scale (the full line height).
    fn scale(&self, weight: Weight, size: f32) -> PxScale {
        let font = self.font(weight);
        PxScale::from(size * font.height_unscaled() / font.units_per_em().unwrap_or(1000.0))
    }

    /// Glyphs the font can draw; anything else (like emoji) is skipped.
    pub fn glyphs(&self, weight: Weight, text: &str) -> Vec<ab_glyph::GlyphId> {
        let font = self.font(weight);
        text.chars().map(|c| font.glyph_id(c)).filter(|id| id.0 != 0).collect()
    }

    pub fn width(&self, weight: Weight, text: &str, size: f32) -> f32 {
        let scaled = self.font(weight).as_scaled(self.scale(weight, size));
        let ids = self.glyphs(weight, text);
        let kerning: f32 = ids.windows(2).map(|w| scaled.kern(w[0], w[1])).sum();
        ids.iter().map(|id| scaled.h_advance(*id)).sum::<f32>() + kerning
    }

    /// Draws runs of differently styled text on one baseline at `y`.
    pub fn draw_spans(&self, pixmap: &mut Pixmap, spans: &[(&str, Weight, u32)], size: f32, x: f32, y: f32, align: Align) {
        let total: f32 = spans.iter().map(|(text, weight, _)| self.width(*weight, text, size)).sum();
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - total / 2.0,
            Align::Right => x - total,
        };
        for (text, weight, rgb) in spans {
            self.draw_run(pixmap, (text, *weight, *rgb), size, pen, y);
            pen += self.width(*weight, text, size);
        }
    }

    fn draw_run(&self, pixmap: &mut Pixmap, (text, weight, rgb): (&str, Weight, u32), size: f32, x: f32, y: f32) {
        let font = self.font(weight);
        let scale = self.scale(weight, size);
        let scaled = font.as_scaled(scale);
        let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
        let [r, g, b] = [(rgb >> 16) as u8 as f32, (rgb >> 8) as u8 as f32, rgb as u8 as f32];
        let pixels = pixmap.pixels_mut();

        let mut pen = x;
        let mut previous = None;
        for id in self.glyphs(weight, text) {
            if let Some(prev) = previous {
                pen += scaled.kern(prev, id);
            }
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen, y));
            pen += scaled.h_advance(id);
            previous = Some(id);

            let Some(outline) = font.outline_glyph(glyph) else { continue };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= width || py >= height {
                    return;
                }
                // Source-over blend into premultiplied pixels
                let a = coverage.clamp(0.0, 1.0);
                let dst = &mut pixels[(py * width + px) as usize];
                let blend = |src: f32, dst: u8| (src * a + dst as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
                let alpha = blend(255.0, dst.alpha());
                let channel = |src: f32, dst: u8| blend(src, dst).min(alpha);
                if let Some(c) = PremultipliedColorU8::from_rgba(channel(r, dst.red()), channel(g, dst.green()), channel(b, dst.blue()), alpha) {
                    *dst = c;
                }
            });
        }
    }

    /// Shrinks `text` (down to `min_size`, then by trimming with "…") until it fits `max_width`.
    pub fn fit(&self, weight: Weight, text: &str, size: f32, min_size: f32, max_width: f32) -> (String, f32) {
        let mut size = size;
        while size > min_size && self.width(weight, text, size) > max_width {
            size -= 1.0;
        }
        let mut text = text.to_string();
        while self.width(weight, &text, size) > max_width && text.chars().count() > 1 {
            text.pop();
            while text.ends_with('…') {
                text.pop();
            }
            text.push('…');
        }
        (text, size)
    }

    /// Splits text into lines no wider than `max_width`, breaking between words.
    pub fn wrap(&self, weight: Weight, text: &str, size: f32, max_width: f32) -> Vec<String> {
        let mut lines = vec![];
        let mut line = String::new();
        for word in text.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && self.width(weight, &candidate, size) > max_width {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_between_words() {
        let fonts = Fonts::load().unwrap();
        let width = fonts.width(Weight::Regular, "Maybe you", 30.0);
        let lines = fonts.wrap(Weight::Regular, "Maybe you should play with him.", 30.0, width + 1.0);
        assert_eq!(lines.first().map(String::as_str), Some("Maybe you"));
        assert_eq!(lines.join(" "), "Maybe you should play with him.");
        assert!(lines.iter().all(|l| fonts.width(Weight::Regular, l, 30.0) <= width + 1.0));
    }
}

/// Downloads an image, giving up quietly after a few seconds.
pub async fn download(url: &str) -> Option<Vec<u8>> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap());
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    Some(response.bytes().await.ok()?.to_vec())
}
