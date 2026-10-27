//! Draws the /rank card as a PNG: a dark card with a glowing cyan-to-orange border,
//! a glowing avatar ring, and a segmented gradient XP bar.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Point, PremultipliedColorU8, Shader, SpreadMode, Stroke, Transform,
};

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Bold.ttf");

const WIDTH: u32 = 1080;
const HEIGHT: u32 = 400;

const PAGE: u32 = 0x10141c;
const CARD_LEFT: u32 = 0x1b2230;
const CARD_RIGHT: u32 = 0x222a39;
const PANEL: u32 = 0x161c27;
const BOX: u32 = 0x232c3c;
const TRACK: u32 = 0x0f141d;
const TRACK_BORDER: u32 = 0x2d3748;
const WHITE: u32 = 0xf1f4f8;
const GREY: u32 = 0xa0aec0;
const CYAN: u32 = 0x00e5ff;
const ORANGE: u32 = 0xff9c59;
const BAR_START: u32 = 0x8b1a2b;
const BAR_END: u32 = 0xd8925e;
const BAR_GLOW: u32 = 0xe0303f;

// Card layout
const CARD_X: f32 = 22.0;
const CARD_Y: f32 = 22.0;
const CARD_W: f32 = WIDTH as f32 - 44.0;
const CARD_H: f32 = HEIGHT as f32 - 44.0;
const CARD_RADIUS: f32 = 20.0;
const PANEL_RIGHT: f32 = 300.0;
const CONTENT_X: f32 = 330.0;
const CONTENT_RIGHT: f32 = 1028.0;

pub struct CardData {
    pub username: String,
    /// PNG bytes of the avatar
    pub avatar: Option<Vec<u8>>,
    pub level: i64,
    pub rep: i64,
    /// Leaderboard position, if ranked
    pub position: Option<i64>,
    pub role_name: Option<String>,
    pub role_colour: u32,
    /// PNG bytes of the highest role's icon
    pub role_icon: Option<Vec<u8>>,
    pub xp: i64,
    /// Total XP needed for the next level
    pub next_level_xp: i64,
}

fn colour(rgb: u32, alpha: u8) -> Color {
    Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, alpha)
}

fn solid(rgb: u32, alpha: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(colour(rgb, alpha));
    paint.anti_alias = true;
    paint
}

/// A left-to-right gradient between two colours over `x0..x1`.
fn gradient(x0: f32, x1: f32, from: u32, to: u32, alpha: u8) -> Paint<'static> {
    let stops = vec![GradientStop::new(0.0, colour(from, alpha)), GradientStop::new(1.0, colour(to, alpha))];
    let shader = LinearGradient::new(Point::from_xy(x0, 0.0), Point::from_xy(x1, 0.0), stops, SpreadMode::Pad, Transform::identity())
        .unwrap_or(Shader::SolidColor(colour(from, alpha)));
    Paint { shader, anti_alias: true, ..Default::default() }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
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

fn fill(pixmap: &mut Pixmap, path: &Path, paint: &Paint, mask: Option<&Mask>) {
    pixmap.fill_path(path, paint, FillRule::Winding, Transform::identity(), mask);
}

fn stroke(pixmap: &mut Pixmap, path: &Path, paint: &Paint, width: f32) {
    let stroke = Stroke { width, ..Default::default() };
    pixmap.stroke_path(path, paint, &stroke, Transform::identity(), None);
}

/// A soft glow: wide, faint strokes underneath a crisp one.
fn glow(pixmap: &mut Pixmap, path: &Path, paint_at: impl Fn(u8) -> Paint<'static>, width: f32) {
    for (extra, alpha) in [(18.0, 10), (12.0, 18), (7.0, 32), (3.0, 70)] {
        stroke(pixmap, path, &paint_at(alpha), width + extra);
    }
    if width > 0.0 {
        stroke(pixmap, path, &paint_at(255), width);
    }
}

fn mask_for(path: &Path) -> Option<Mask> {
    let mut mask = Mask::new(WIDTH, HEIGHT)?;
    mask.fill_path(path, FillRule::Winding, true, Transform::identity());
    Some(mask)
}

/// Draws a PNG scaled into a square box, optionally clipped to a circle.
fn draw_image(pixmap: &mut Pixmap, png: &[u8], x: f32, y: f32, size: f32, circle: bool) {
    let Ok(image) = Pixmap::decode_png(png) else { return };
    let transform = Transform::from_row(size / image.width() as f32, 0.0, 0.0, size / image.height() as f32, x, y);
    let mask = if circle {
        PathBuilder::from_circle(x + size / 2.0, y + size / 2.0, size / 2.0).and_then(|p| mask_for(&p))
    } else {
        None
    };
    let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
    pixmap.draw_pixmap(0, 0, image.as_ref(), &paint, transform, mask.as_ref());
}

#[derive(Clone, Copy)]
enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

struct Fonts<'a> {
    regular: FontRef<'a>,
    bold: FontRef<'a>,
}

impl Fonts<'_> {
    fn font(&self, weight: Weight) -> &FontRef<'_> {
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
    fn glyphs(&self, weight: Weight, text: &str) -> Vec<ab_glyph::GlyphId> {
        let font = self.font(weight);
        text.chars().map(|c| font.glyph_id(c)).filter(|id| id.0 != 0).collect()
    }

    fn width(&self, weight: Weight, text: &str, size: f32) -> f32 {
        let scaled = self.font(weight).as_scaled(self.scale(weight, size));
        let ids = self.glyphs(weight, text);
        let kerning: f32 = ids.windows(2).map(|w| scaled.kern(w[0], w[1])).sum();
        ids.iter().map(|id| scaled.h_advance(*id)).sum::<f32>() + kerning
    }

    /// Draws runs of differently styled text on one baseline at `y`.
    fn draw_spans(&self, pixmap: &mut Pixmap, spans: &[(&str, Weight, u32)], size: f32, x: f32, y: f32, align: Align) {
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
    fn fit(&self, weight: Weight, text: &str, size: f32, min_size: f32, max_width: f32) -> (String, f32) {
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
}

/// Renders the rank card. Returns PNG bytes.
pub fn render(data: &CardData) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
    let fonts = Fonts { regular: FontRef::try_from_slice(REGULAR).ok()?, bold: FontRef::try_from_slice(BOLD).ok()? };
    pixmap.fill(colour(PAGE, 255));

    // Card body
    let card = rounded_rect(CARD_X, CARD_Y, CARD_W, CARD_H, CARD_RADIUS)?;
    fill(&mut pixmap, &card, &gradient(CARD_X, CARD_X + CARD_W, CARD_LEFT, CARD_RIGHT, 255), None);

    // Left panel, clipped to the card's rounded corners
    let card_mask = mask_for(&card)?;
    let panel = rounded_rect(CARD_X, CARD_Y, PANEL_RIGHT - CARD_X, CARD_H, 0.0)?;
    fill(&mut pixmap, &panel, &solid(PANEL, 200), Some(&card_mask));
    let divider = rounded_rect(PANEL_RIGHT - 1.0, CARD_Y, 2.0, CARD_H, 0.0)?;
    fill(&mut pixmap, &divider, &solid(TRACK_BORDER, 180), None);

    // Glowing cyan-to-orange border
    glow(&mut pixmap, &card, |alpha| gradient(CARD_X, CARD_X + CARD_W, CYAN, ORANGE, alpha), 2.0);

    // Avatar with a glowing cyan ring
    let (avatar_cx, avatar_cy, avatar_r) = (161.0, 132.0, 78.0);
    let ring = PathBuilder::from_circle(avatar_cx, avatar_cy, avatar_r + 5.0)?;
    fill(&mut pixmap, &ring, &solid(TRACK, 255), None);
    if let Some(avatar) = &data.avatar {
        draw_image(&mut pixmap, avatar, avatar_cx - avatar_r, avatar_cy - avatar_r, avatar_r * 2.0, true);
    }
    glow(&mut pixmap, &ring, |alpha| solid(CYAN, alpha), 3.0);

    // Level and reputation boxes
    for (y, label, value) in [(240.0, "LEVEL ", data.level), (305.0, "REP ", data.rep)] {
        let rect = rounded_rect(48.0, y, 226.0, 52.0, 12.0)?;
        fill(&mut pixmap, &rect, &solid(BOX, 230), None);
        stroke(&mut pixmap, &rect, &solid(TRACK_BORDER, 255), 1.5);
        let value = value.to_string();
        let spans = [(label, Weight::Regular, WHITE), (value.as_str(), Weight::Bold, WHITE)];
        fonts.draw_spans(&mut pixmap, &spans, 28.0, 161.0, y + 36.0, Align::Center);
    }

    // Rank position (top right), then the username in the space left of it
    let mut name_right = CONTENT_RIGHT;
    if let Some(position) = data.position {
        let rank = format!("#{position}");
        fonts.draw_spans(&mut pixmap, &[(&rank, Weight::Bold, WHITE)], 46.0, CONTENT_RIGHT, 94.0, Align::Right);
        name_right -= fonts.width(Weight::Bold, &rank, 46.0) + 30.0;
    }
    let (name, name_size) = fonts.fit(Weight::Regular, &data.username, 30.0, 20.0, name_right - CONTENT_X);
    fonts.draw_spans(&mut pixmap, &[(&name, Weight::Regular, WHITE)], name_size, CONTENT_X, 88.0, Align::Left);

    // Highest role in its colour, with its icon if it has one
    if let Some(role) = &data.role_name {
        let mut x = CONTENT_X;
        if let Some(icon) = &data.role_icon {
            draw_image(&mut pixmap, icon, CONTENT_X, 118.0, 44.0, false);
            x += 56.0;
        }
        let (role, size) = fonts.fit(Weight::Bold, role, 46.0, 26.0, CONTENT_RIGHT - x);
        fonts.draw_spans(&mut pixmap, &[(&role, Weight::Bold, data.role_colour)], size, x, 158.0, Align::Left);
    }

    // XP bar: a pill-shaped track with a glowing, segmented gradient fill
    let (bar_x, bar_y, bar_w, bar_h) = (CONTENT_X, 196.0, CONTENT_RIGHT - CONTENT_X, 48.0);
    let track = rounded_rect(bar_x, bar_y, bar_w, bar_h, bar_h / 2.0)?;
    fill(&mut pixmap, &track, &solid(TRACK, 255), None);

    let progress = if data.next_level_xp > 0 { (data.xp as f32 / data.next_level_xp as f32).clamp(0.0, 1.0) } else { 0.0 };
    let inset = 7.0;
    let (fill_x, fill_y, fill_h) = (bar_x + inset, bar_y + inset, bar_h - inset * 2.0);
    let inner_w = bar_w - inset * 2.0;
    let fill_w = inner_w * progress;
    if fill_w >= 1.0 {
        let filled = rounded_rect(fill_x, fill_y, fill_w.max(fill_h), fill_h, fill_h / 2.0)?;
        glow(&mut pixmap, &filled, |alpha| solid(BAR_GLOW, alpha / 2), 0.0);
        fill(&mut pixmap, &filled, &gradient(fill_x, fill_x + inner_w, BAR_START, BAR_END, 255), None);

        // Slanted gaps split the fill into four segments
        for i in 1..4 {
            let gap_x = fill_x + inner_w * i as f32 / 4.0;
            if gap_x > fill_x + fill_w - 8.0 {
                break;
            }
            let mut pb = PathBuilder::new();
            pb.move_to(gap_x + 3.0, fill_y);
            pb.line_to(gap_x + 8.0, fill_y);
            pb.line_to(gap_x + 3.0, fill_y + fill_h);
            pb.line_to(gap_x - 2.0, fill_y + fill_h);
            pb.close();
            if let Some(gap) = pb.finish() {
                fill(&mut pixmap, &gap, &solid(TRACK, 255), None);
            }
        }
    }
    stroke(&mut pixmap, &track, &gradient(bar_x, bar_x + bar_w, 0x5a6478, 0xb08060, 200), 2.0);

    // "18755 / 21600 for next rank"
    let xp_text = format!("{} / {}", data.xp, data.next_level_xp);
    let spans = [(xp_text.as_str(), Weight::Regular, WHITE), (" for ", Weight::Regular, GREY), ("next rank", Weight::Regular, ORANGE)];
    fonts.draw_spans(&mut pixmap, &spans, 26.0, bar_x + bar_w / 2.0, 300.0, Align::Center);

    pixmap.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set RANK_CARD_PREVIEW to a path to save the rendered card, RANK_CARD_AVATAR to
    /// a PNG avatar, and RANK_CARD_NAME to try different usernames.
    #[test]
    fn renders_a_png() {
        let data = CardData {
            username: std::env::var("RANK_CARD_NAME").unwrap_or_else(|_| "a_really_long_username_that_needs_to_shrink_to_fit".to_string()),
            avatar: std::env::var("RANK_CARD_AVATAR").ok().and_then(|p| std::fs::read(p).ok()),
            level: 8,
            rep: 0,
            position: Some(1),
            role_name: Some("Founder".to_string()),
            role_colour: 0xe6b464,
            role_icon: None,
            xp: 18755,
            next_level_xp: 21600,
        };
        let png = render(&data).expect("card renders");
        assert!(png.starts_with(b"\x89PNG"));
        let decoded = Pixmap::decode_png(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));

        if let Ok(path) = std::env::var("RANK_CARD_PREVIEW") {
            std::fs::write(path, png).unwrap();
        }
    }
}
