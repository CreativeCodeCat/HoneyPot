//! Draws the welcome image as a PNG: a night-sky gradient with soft glowing orbs, the new
//! member's avatar in a glowing cyan-to-pink ring, "WELCOME" with their name, the server's
//! name, and their member number in a pill.

use tiny_skia::{PathBuilder, Pixmap};
use unicode_normalization::UnicodeNormalization;

use crate::render::{draw_png, fill, glow, gradient, mask_for, rounded_rect, solid, stroke, vgradient, Align, Fonts, Weight};

const WIDTH: u32 = 1100;
const HEIGHT: u32 = 500;

const SKY_TOP: u32 = 0x1a1033;
const SKY_BOTTOM: u32 = 0x0b1026;
const CARD: u32 = 0x12162a;
const WHITE: u32 = 0xf5f7ff;
const GREY: u32 = 0xa9b1d6;
const CYAN: u32 = 0x22d3ee;
const PINK: u32 = 0xf472b6;
const VIOLET: u32 = 0x8b5cf6;

pub struct WelcomeData {
    /// Names to show, in order of preference; the first one the font can draw is used
    pub names: Vec<String>,
    /// PNG bytes of the avatar
    pub avatar: Option<Vec<u8>>,
    pub server: String,
    pub member_count: u64,
}

/// "1st", "2nd", "23rd", "1,234th"...
fn ordinal(n: u64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    // Thousands separated, like "1,234th"
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped}{suffix}")
}

/// Renders the welcome image. Returns PNG bytes.
pub fn render(data: &WelcomeData) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
    let fonts = Fonts::load()?;
    let (w, h) = (WIDTH as f32, HEIGHT as f32);

    // Night sky with soft orbs of light
    let sky = rounded_rect(0.0, 0.0, w, h, 0.0)?;
    fill(&mut pixmap, &sky, &vgradient(0.0, h, SKY_TOP, SKY_BOTTOM, 255), None);
    for (x, y, r, rgb) in [(120.0, 90.0, 140.0, VIOLET), (980.0, 420.0, 170.0, CYAN), (900.0, 70.0, 90.0, PINK), (210.0, 450.0, 110.0, PINK)] {
        // Rings of falling opacity fake a soft blur
        for step in 0..12 {
            let radius = r * (1.0 - step as f32 / 12.0);
            if let Some(orb) = PathBuilder::from_circle(x, y, radius) {
                fill(&mut pixmap, &orb, &solid(rgb, 9), None);
            }
        }
    }
    // Tiny stars
    for i in 0..60u32 {
        let x = (i * 397 % WIDTH) as f32;
        let y = (i * 211 % HEIGHT) as f32;
        let size = 1.0 + (i % 3) as f32 * 0.6;
        if let Some(star) = PathBuilder::from_circle(x, y, size) {
            fill(&mut pixmap, &star, &solid(WHITE, 40 + (i % 5) as u8 * 25), None);
        }
    }

    // Frosted card with a glowing gradient border
    let (card_x, card_y, card_w, card_h) = (40.0, 40.0, w - 80.0, h - 80.0);
    let card = rounded_rect(card_x, card_y, card_w, card_h, 28.0)?;
    fill(&mut pixmap, &card, &solid(CARD, 150), None);
    glow(&mut pixmap, &card, |alpha| gradient(card_x, card_x + card_w, CYAN, PINK, alpha / 2), 1.5);
    let card_mask = mask_for(&card, WIDTH, HEIGHT)?;
    // A sheen across the top of the card
    let sheen = rounded_rect(card_x, card_y, card_w, 90.0, 0.0)?;
    fill(&mut pixmap, &sheen, &vgradient(card_y, card_y + 90.0, WHITE, CARD, 10), Some(&card_mask));

    // Avatar in a glowing cyan-to-pink ring
    let (cx, cy, r) = (w / 2.0, 150.0, 78.0);
    let ring = PathBuilder::from_circle(cx, cy, r + 7.0)?;
    fill(&mut pixmap, &ring, &solid(SKY_BOTTOM, 255), None);
    match &data.avatar {
        Some(avatar) => draw_png(&mut pixmap, avatar, cx - r, cy - r, r * 2.0, true),
        None => {
            let blank = PathBuilder::from_circle(cx, cy, r)?;
            fill(&mut pixmap, &blank, &gradient(cx - r, cx + r, VIOLET, PINK, 255), None);
        }
    }
    glow(&mut pixmap, &ring, |alpha| gradient(cx - r, cx + r, CYAN, PINK, alpha), 4.0);

    // WELCOME, spaced out
    let welcome = "W E L C O M E";
    fonts.draw_spans(&mut pixmap, &[(welcome, Weight::Bold, WHITE)], 54.0, cx, 300.0, Align::Center);

    // Their name; stylised letters (𝓒𝓻𝓮𝓪𝓽𝓲𝓿𝓮...) become plain ones, and names the font can't draw are skipped
    let name = data
        .names
        .iter()
        .map(|name| name.nfkc().collect::<String>())
        .find(|name| !fonts.glyphs(Weight::Bold, name).is_empty())
        .unwrap_or_default();
    let (name, size) = fonts.fit(Weight::Bold, &name, 40.0, 24.0, card_w - 120.0);
    fonts.draw_spans(&mut pixmap, &[(&name, Weight::Bold, CYAN)], size, cx, 352.0, Align::Center);

    // "to <server>"
    let server = data.server.nfkc().collect::<String>();
    let (server, size) = fonts.fit(Weight::Regular, &server, 26.0, 18.0, card_w - 200.0);
    fonts.draw_spans(&mut pixmap, &[("to ", Weight::Regular, GREY), (&server, Weight::Bold, WHITE)], size, cx, 392.0, Align::Center);

    // Member number in a pill
    let count = format!("{} member", ordinal(data.member_count));
    let pill_w = fonts.width(Weight::Bold, &count, 20.0) + 48.0;
    let pill = rounded_rect(cx - pill_w / 2.0, 410.0, pill_w, 34.0, 17.0)?;
    fill(&mut pixmap, &pill, &gradient(cx - pill_w / 2.0, cx + pill_w / 2.0, VIOLET, PINK, 200), None);
    stroke(&mut pixmap, &pill, &solid(WHITE, 60), 1.0);
    fonts.draw_spans(&mut pixmap, &[(&count, Weight::Bold, WHITE)], 20.0, cx, 434.0, Align::Center);

    pixmap.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set WELCOME_CARD_PREVIEW to a path to save the image, and WELCOME_CARD_AVATAR to a PNG avatar.
    #[test]
    fn renders_a_png() {
        let data = WelcomeData {
            names: vec!["CreativeCodeCat".to_string()],
            avatar: std::env::var("WELCOME_CARD_AVATAR").ok().and_then(|p| std::fs::read(p).ok()),
            server: "The HoneyPot Community".to_string(),
            member_count: 1234,
        };
        let png = render(&data).expect("image renders");
        let decoded = Pixmap::decode_png(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
        if let Ok(path) = std::env::var("WELCOME_CARD_PREVIEW") {
            std::fs::write(path, png).unwrap();
        }
        assert_eq!((ordinal(1), ordinal(12), ordinal(23), ordinal(111)), ("1st".into(), "12th".into(), "23rd".into(), "111th".into()));
        assert_eq!(ordinal(1234), "1,234th");
    }
}
