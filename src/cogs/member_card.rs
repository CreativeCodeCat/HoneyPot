//! Draws the member images as PNGs: a night-sky gradient with soft glowing orbs, the
//! member's avatar in a glowing ring, a headline with their name, a line about the server,
//! and a pill. `welcome` greets new members, and `boost` thanks members who boost.

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
const NITRO_PINK: u32 = 0xff73fa;
const NITRO_PURPLE: u32 = 0xb06dff;

/// The member the image is about.
pub struct Member {
    /// Names to show, in order of preference; the first one the font can draw is used
    pub names: Vec<String>,
    /// PNG bytes of the avatar
    pub avatar: Option<Vec<u8>>,
}

/// The colours of one kind of image.
struct Theme {
    sky_top: u32,
    /// Ring, border and name colours, from left to right
    accent: (u32, u32),
    name: u32,
    pill: (u32, u32),
    orbs: [u32; 4],
}

const WELCOME_THEME: Theme =
    Theme { sky_top: SKY_TOP, accent: (CYAN, PINK), name: CYAN, pill: (VIOLET, PINK), orbs: [VIOLET, CYAN, PINK, PINK] };
const BOOST_THEME: Theme = Theme {
    sky_top: 0x2a0f3d,
    accent: (NITRO_PURPLE, NITRO_PINK),
    name: NITRO_PINK,
    pill: (NITRO_PURPLE, NITRO_PINK),
    orbs: [NITRO_PINK, NITRO_PURPLE, NITRO_PINK, NITRO_PURPLE],
};

/// The text on an image, top to bottom.
struct Text<'a> {
    headline: &'a str,
    /// A plain lead-in then a bold part, like ("to ", "Server Name")
    line: (&'a str, &'a str),
    pill: &'a str,
}

/// A new member's welcome: "WELCOME", their name, "to <server>" and which member they are.
pub fn welcome(member: &Member, server: &str, member_count: u64) -> Option<Vec<u8>> {
    let pill = format!("{} member", ordinal(member_count));
    render(member, &Text { headline: "W E L C O M E", line: ("to ", server), pill: &pill }, &WELCOME_THEME)
}

/// Thanks for boosting: their name, "for boosting <server>" and the server's boosts and level.
pub fn boost(member: &Member, server: &str, boosts: u64, level: u8) -> Option<Vec<u8>> {
    let level = if level == 0 { "No level yet".to_string() } else { format!("Level {level}") };
    let pill = format!("{boosts} boost{} • {level}", if boosts == 1 { "" } else { "s" });
    render(member, &Text { headline: "T H A N K   Y O U", line: ("for boosting ", server), pill: &pill }, &BOOST_THEME)
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

/// Renders an image. Returns PNG bytes.
fn render(member: &Member, text: &Text, theme: &Theme) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
    let fonts = Fonts::load()?;
    let (w, h) = (WIDTH as f32, HEIGHT as f32);

    // Night sky with soft orbs of light
    let sky = rounded_rect(0.0, 0.0, w, h, 0.0)?;
    fill(&mut pixmap, &sky, &vgradient(0.0, h, theme.sky_top, SKY_BOTTOM, 255), None);
    let [a, b, c, d] = theme.orbs;
    for (x, y, r, rgb) in [(120.0, 90.0, 140.0, a), (980.0, 420.0, 170.0, b), (900.0, 70.0, 90.0, c), (210.0, 450.0, 110.0, d)] {
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
    let (from, to) = theme.accent;
    glow(&mut pixmap, &card, |alpha| gradient(card_x, card_x + card_w, from, to, alpha / 2), 1.5);
    let card_mask = mask_for(&card, WIDTH, HEIGHT)?;
    // A sheen across the top of the card
    let sheen = rounded_rect(card_x, card_y, card_w, 90.0, 0.0)?;
    fill(&mut pixmap, &sheen, &vgradient(card_y, card_y + 90.0, WHITE, CARD, 10), Some(&card_mask));

    // Avatar in a glowing ring
    let (cx, cy, r) = (w / 2.0, 150.0, 78.0);
    let ring = PathBuilder::from_circle(cx, cy, r + 7.0)?;
    fill(&mut pixmap, &ring, &solid(SKY_BOTTOM, 255), None);
    match &member.avatar {
        Some(avatar) => draw_png(&mut pixmap, avatar, cx - r, cy - r, r * 2.0, true),
        None => {
            let blank = PathBuilder::from_circle(cx, cy, r)?;
            fill(&mut pixmap, &blank, &gradient(cx - r, cx + r, theme.pill.0, theme.pill.1, 255), None);
        }
    }
    glow(&mut pixmap, &ring, |alpha| gradient(cx - r, cx + r, from, to, alpha), 4.0);

    // The headline, spaced out
    let (headline, size) = fonts.fit(Weight::Bold, text.headline, 54.0, 30.0, card_w - 80.0);
    fonts.draw_spans(&mut pixmap, &[(&headline, Weight::Bold, WHITE)], size, cx, 300.0, Align::Center);

    // Their name; stylised letters (𝓒𝓻𝓮𝓪𝓽𝓲𝓿𝓮...) become plain ones, and names the font can't draw are skipped
    let name = member
        .names
        .iter()
        .map(|name| name.nfkc().collect::<String>())
        .find(|name| !fonts.glyphs(Weight::Bold, name).is_empty())
        .unwrap_or_default();
    let (name, size) = fonts.fit(Weight::Bold, &name, 40.0, 24.0, card_w - 120.0);
    fonts.draw_spans(&mut pixmap, &[(&name, Weight::Bold, theme.name)], size, cx, 352.0, Align::Center);

    // Like "to <server>", leaving room for the lead-in
    let (lead, server) = text.line;
    let server = server.nfkc().collect::<String>();
    let lead_w = fonts.width(Weight::Regular, lead, 26.0);
    let (server, size) = fonts.fit(Weight::Bold, &server, 26.0, 18.0, card_w - 200.0 - lead_w);
    fonts.draw_spans(&mut pixmap, &[(lead, Weight::Regular, GREY), (&server, Weight::Bold, WHITE)], size, cx, 392.0, Align::Center);

    // The pill
    let pill_w = fonts.width(Weight::Bold, text.pill, 20.0) + 48.0;
    let pill = rounded_rect(cx - pill_w / 2.0, 410.0, pill_w, 34.0, 17.0)?;
    fill(&mut pixmap, &pill, &gradient(cx - pill_w / 2.0, cx + pill_w / 2.0, theme.pill.0, theme.pill.1, 200), None);
    stroke(&mut pixmap, &pill, &solid(WHITE, 60), 1.0);
    fonts.draw_spans(&mut pixmap, &[(text.pill, Weight::Bold, WHITE)], 20.0, cx, 434.0, Align::Center);

    pixmap.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set MEMBER_CARD_PREVIEW to a folder to save the images, and MEMBER_CARD_AVATAR to a PNG avatar.
    #[test]
    fn renders_pngs() {
        let member = Member {
            names: vec!["CreativeCodeCat".to_string()],
            avatar: std::env::var("MEMBER_CARD_AVATAR").ok().and_then(|p| std::fs::read(p).ok()),
        };
        for (file, png) in [
            ("welcome.png", welcome(&member, "The HoneyPot Community", 1234)),
            ("boost.png", boost(&member, "The HoneyPot Community", 7, 2)),
        ] {
            let png = png.expect("image renders");
            let decoded = Pixmap::decode_png(&png).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
            if let Ok(folder) = std::env::var("MEMBER_CARD_PREVIEW") {
                std::fs::write(std::path::Path::new(&folder).join(file), png).unwrap();
            }
        }
        assert_eq!((ordinal(1), ordinal(12), ordinal(23), ordinal(111)), ("1st".into(), "12th".into(), "23rd".into(), "111th".into()));
        assert_eq!(ordinal(1234), "1,234th");
    }
}
