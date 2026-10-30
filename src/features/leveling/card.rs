//! Draws the /rank card as a PNG: a dark card with a glowing cyan-to-orange border,
//! a glowing avatar ring, and a segmented gradient XP bar.

use tiny_skia::{PathBuilder, Pixmap};
use unicode_normalization::UnicodeNormalization;

use crate::render::{colour, draw_png, fill, glow, gradient, mask_for, rounded_rect, solid, stroke, Align, Fonts, Weight};

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
    /// Names to show, in order of preference; the first one the font can draw is used
    pub names: Vec<String>,
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

/// Renders the rank card. Returns PNG bytes.
pub fn render(data: &CardData) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
    let fonts = Fonts::load()?;
    pixmap.fill(colour(PAGE, 255));

    // Card body
    let card = rounded_rect(CARD_X, CARD_Y, CARD_W, CARD_H, CARD_RADIUS)?;
    fill(&mut pixmap, &card, &gradient(CARD_X, CARD_X + CARD_W, CARD_LEFT, CARD_RIGHT, 255), None);

    // Left panel, clipped to the card's rounded corners
    let card_mask = mask_for(&card, WIDTH, HEIGHT)?;
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
        draw_png(&mut pixmap, avatar, avatar_cx - avatar_r, avatar_cy - avatar_r, avatar_r * 2.0, true);
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
    // Stylised letters (𝓒𝓻𝓮𝓪𝓽𝓲𝓿𝓮, 𝐁𝐨𝐥𝐝, ...) become plain ones; skip names with nothing drawable
    let name = data
        .names
        .iter()
        .map(|name| name.nfkc().collect::<String>())
        .find(|name| !fonts.glyphs(Weight::Regular, name).is_empty())
        .unwrap_or_default();
    let (name, name_size) = fonts.fit(Weight::Regular, &name, 30.0, 20.0, name_right - CONTENT_X);
    fonts.draw_spans(&mut pixmap, &[(&name, Weight::Regular, WHITE)], name_size, CONTENT_X, 88.0, Align::Left);

    // Highest role in its colour, with its icon if it has one
    if let Some(role) = &data.role_name {
        let mut x = CONTENT_X;
        if let Some(icon) = &data.role_icon {
            draw_png(&mut pixmap, icon, CONTENT_X, 118.0, 44.0, false);
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
            names: vec![std::env::var("RANK_CARD_NAME").unwrap_or_else(|_| "a_really_long_username_that_needs_to_shrink_to_fit".to_string())],
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
