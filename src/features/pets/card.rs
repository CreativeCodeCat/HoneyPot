//! Draws the /pet status card: a parchment card with a wooden name sign, the pet's
//! picture in the middle, and its stats around it. Pictures and icons are Twemoji.

use tiny_skia::{PathBuilder, Pixmap};

use super::store::Mood;
use crate::render::{colour, draw_svg, fill, rounded_rect, solid, stroke, vgradient, Align, Fonts, Weight};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 880;

const PAGE: u32 = 0xe6d6bd;
const PARCHMENT_TOP: u32 = 0xf8eedf;
const PARCHMENT_BOTTOM: u32 = 0xefdfc5;
const EDGE: u32 = 0xc4a175;
const BOX: u32 = 0xf3e3c9;
const WOOD_TOP: u32 = 0xdcab74;
const WOOD_BOTTOM: u32 = 0xb47c46;
const WOOD_EDGE: u32 = 0x7a4c27;
const SCROLL: u32 = 0xf1e1c3;
const SCROLL_CURL: u32 = 0xdcc29a;
const SHADOW: u32 = 0xdcc6a4;
const INK: u32 = 0x3b2416;
const FADED_INK: u32 = 0x7a5c45;

const LEFT: f32 = 56.0;
const CENTER: f32 = 384.0;
const RIGHT: f32 = 734.0;

pub struct CardData {
    pub name: String,
    /// e.g. "@CreativeCodeCat's Male Saber Tooth Tiger"
    pub subtitle: String,
    /// Animal key, e.g. "saber_tooth_tiger"
    pub kind: String,
    pub age: String,
    pub level: i64,
    pub exp: f64,
    /// Total experience needed for the next level; `None` at the highest level
    pub next_level_exp: Option<f64>,
    pub fun: f64,
    pub max_fun: f64,
    pub happy_at: f64,
    pub mood: Mood,
    pub mood_text: String,
    pub treats: f64,
    pub max_treats: f64,
    pub food_lasts: String,
}

/// Twemoji SVGs, by name.
fn emoji(name: &str) -> Option<&'static [u8]> {
    macro_rules! emoji {
        ($($name:literal),* $(,)?) => {
            match name {
                $($name => Some(include_bytes!(concat!("../../../assets/emoji/", $name, ".svg")).as_slice()),)*
                _ => None,
            }
        };
    }
    emoji!(
        "cat", "dog", "pig", "horse", "panda", "saber_tooth_tiger", "wolf", "t_rex", "bat", "mouse", "phoenix", "fairy",
        "house_elf", "dragon", "duck", "ant", "penguin", "lizard", "snake", "spider", "hourglass", "shield", "star",
        "cookie", "balloon", "happy", "meh", "sad", "bone",
    )
}

fn icon(pixmap: &mut Pixmap, name: &str, x: f32, y: f32, size: f32) {
    if let Some(svg) = emoji(name) {
        draw_svg(pixmap, svg, x, y, size);
    }
}

fn mood_icon(mood: Mood) -> &'static str {
    match mood {
        Mood::Happy => "happy",
        Mood::Meh => "meh",
        Mood::Sad => "sad",
    }
}

/// A "Label:" heading with an icon after it, then the value underneath.
fn stat(pixmap: &mut Pixmap, fonts: &Fonts, x: f32, y: f32, label: &str, icon_name: Option<&str>, value: &str) {
    fonts.draw_spans(pixmap, &[(label, Weight::Bold, INK)], 38.0, x, y, Align::Left);
    if let Some(name) = icon_name {
        icon(pixmap, name, x + fonts.width(Weight::Bold, label, 38.0) + 10.0, y - 34.0, 40.0);
    }
    fonts.draw_spans(pixmap, &[(value, Weight::Regular, INK)], 36.0, x, y + 54.0, Align::Left);
}

/// Renders the pet status card. Returns PNG bytes.
pub fn render(data: &CardData) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
    let fonts = Fonts::load()?;
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    pixmap.fill(colour(PAGE, 255));

    // Parchment card
    let card = rounded_rect(14.0, 14.0, w - 28.0, h - 28.0, 30.0)?;
    fill(&mut pixmap, &card, &vgradient(14.0, h - 14.0, PARCHMENT_TOP, PARCHMENT_BOTTOM, 255), None);
    stroke(&mut pixmap, &card, &solid(EDGE, 160), 3.0);

    // Wooden name sign
    let (sign_w, sign_h) = (440.0, 104.0);
    let (sign_x, sign_y) = ((w - sign_w) / 2.0, 30.0);
    let sign = rounded_rect(sign_x, sign_y, sign_w, sign_h, 14.0)?;
    fill(&mut pixmap, &sign, &vgradient(sign_y, sign_y + sign_h, WOOD_TOP, WOOD_BOTTOM, 255), None);
    stroke(&mut pixmap, &sign, &solid(WOOD_EDGE, 255), 5.0);
    let inner = rounded_rect(sign_x + 10.0, sign_y + 10.0, sign_w - 20.0, sign_h - 20.0, 8.0)?;
    stroke(&mut pixmap, &inner, &solid(WOOD_EDGE, 120), 2.0);
    for (nx, ny) in [(18.0, 18.0), (sign_w - 18.0, 18.0), (18.0, sign_h - 18.0), (sign_w - 18.0, sign_h - 18.0)] {
        let nail = PathBuilder::from_circle(sign_x + nx, sign_y + ny, 4.0)?;
        fill(&mut pixmap, &nail, &solid(WOOD_EDGE, 200), None);
    }
    let (name, name_size) = fonts.fit(Weight::Bold, &data.name, 52.0, 30.0, sign_w - 130.0);
    let name_w = fonts.width(Weight::Bold, &name, name_size);
    let title_x = w / 2.0 - (name_w + 66.0) / 2.0;
    icon(&mut pixmap, &data.kind, title_x, sign_y + 24.0, 56.0);
    fonts.draw_spans(&mut pixmap, &[(&name, Weight::Bold, INK)], name_size, title_x + 66.0, sign_y + 70.0, Align::Left);

    // Scroll banner with the owner and kind
    let (scroll_x, scroll_y, scroll_w, scroll_h) = (110.0, 152.0, w - 220.0, 62.0);
    for curl_x in [scroll_x - 18.0, scroll_x + scroll_w - 10.0] {
        let curl = rounded_rect(curl_x, scroll_y - 6.0, 28.0, scroll_h + 12.0, 12.0)?;
        fill(&mut pixmap, &curl, &solid(SCROLL_CURL, 255), None);
        stroke(&mut pixmap, &curl, &solid(EDGE, 255), 2.0);
    }
    let scroll = rounded_rect(scroll_x, scroll_y, scroll_w, scroll_h, 6.0)?;
    fill(&mut pixmap, &scroll, &solid(SCROLL, 255), None);
    stroke(&mut pixmap, &scroll, &solid(EDGE, 255), 2.0);
    let (subtitle, subtitle_size) = fonts.fit(Weight::Regular, &data.subtitle, 32.0, 20.0, scroll_w - 40.0);
    fonts.draw_spans(&mut pixmap, &[(&subtitle, Weight::Regular, INK)], subtitle_size, w / 2.0, scroll_y + 43.0, Align::Center);

    // Top row
    stat(&mut pixmap, &fonts, LEFT, 290.0, "Age:", Some("hourglass"), &data.age);
    stat(&mut pixmap, &fonts, CENTER, 290.0, "Level:", Some("shield"), &data.level.to_string());
    stat(&mut pixmap, &fonts, RIGHT, 290.0, "Experience:", Some("star"), &match data.next_level_exp {
        Some(next) => format!("{:.2} / {next:.0}", data.exp),
        None => format!("{:.0} (max)", data.exp),
    });

    // Fun, with a face for the mood
    let fun = format!("{:.1} / {:.0}", data.fun, data.max_fun);
    stat(&mut pixmap, &fonts, LEFT, 420.0, "Fun:", None, &fun);
    icon(&mut pixmap, mood_icon(data.mood), LEFT + fonts.width(Weight::Regular, &fun, 36.0) + 12.0, 444.0, 40.0);
    let happy_at = format!("happy at {:.0}", data.happy_at);
    fonts.draw_spans(&mut pixmap, &[(&happy_at, Weight::Regular, FADED_INK)], 26.0, LEFT, 518.0, Align::Left);

    // Mood note
    let (mood_x, mood_y, mood_w, mood_h) = (LEFT - 8.0, 556.0, 300.0, 250.0);
    let mood_box = rounded_rect(mood_x, mood_y, mood_w, mood_h, 10.0)?;
    fill(&mut pixmap, &mood_box, &solid(BOX, 255), None);
    stroke(&mut pixmap, &mood_box, &solid(EDGE, 255), 2.0);
    fonts.draw_spans(&mut pixmap, &[("Mood:", Weight::Bold, INK)], 36.0, mood_x + 22.0, mood_y + 52.0, Align::Left);
    icon(&mut pixmap, mood_icon(data.mood), mood_x + 22.0 + fonts.width(Weight::Bold, "Mood:", 36.0) + 10.0, mood_y + 20.0, 38.0);
    for (i, line) in fonts.wrap(Weight::Regular, &data.mood_text, 30.0, mood_w - 44.0).iter().take(4).enumerate() {
        fonts.draw_spans(&mut pixmap, &[(line, Weight::Regular, INK)], 30.0, mood_x + 22.0, mood_y + 102.0 + i as f32 * 40.0, Align::Left);
    }

    // The pet itself, standing on a shadow with a toy nearby
    let mut shadow = PathBuilder::new();
    shadow.push_oval(tiny_skia::Rect::from_xywh(392.0, 650.0, 240.0, 44.0)?);
    fill(&mut pixmap, &shadow.finish()?, &solid(SHADOW, 255), None);
    icon(&mut pixmap, &data.kind, 392.0, 420.0, 240.0);
    icon(&mut pixmap, "bone", 628.0, 640.0, 64.0);

    // Treats box
    let (treat_x, treat_y, treat_w, treat_h) = (RIGHT - 8.0, 400.0, 250.0, 170.0);
    let treat_box = rounded_rect(treat_x, treat_y, treat_w, treat_h, 10.0)?;
    fill(&mut pixmap, &treat_box, &solid(BOX, 255), None);
    stroke(&mut pixmap, &treat_box, &solid(EDGE, 255), 2.0);
    fonts.draw_spans(&mut pixmap, &[("Treats:", Weight::Bold, INK)], 38.0, treat_x + 22.0, treat_y + 54.0, Align::Left);
    icon(&mut pixmap, "cookie", treat_x + 22.0, treat_y + 80.0, 60.0);
    let treats = format!("{:.0} / {:.0}", data.treats.ceil(), data.max_treats);
    fonts.draw_spans(&mut pixmap, &[(&treats, Weight::Regular, INK)], 38.0, treat_x + 96.0, treat_y + 126.0, Align::Left);

    // Food lasts
    stat(&mut pixmap, &fonts, CENTER, 772.0, "Food lasts:", None, &data.food_lasts);

    pixmap.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_animal_has_a_picture() {
        for (kind, _, _) in super::super::ANIMALS {
            assert!(emoji(kind).is_some(), "missing picture for {kind}");
        }
    }

    /// Set PET_CARD_PREVIEW to a path to save the rendered card.
    #[test]
    fn renders_a_png() {
        let data = CardData {
            name: "Scarlet".to_string(),
            subtitle: "@CreativeCodeCat's Male Saber Tooth Tiger".to_string(),
            kind: "saber_tooth_tiger".to_string(),
            age: "6 minutes".to_string(),
            level: 4,
            exp: 34.5,
            next_level_exp: Some(60.0),
            fun: 3.3,
            max_fun: 50.0,
            happy_at: 7.0,
            mood: Mood::Meh,
            mood_text: "A bit meh. Maybe you should play with him.".to_string(),
            treats: 4.0,
            max_treats: 34.0,
            food_lasts: "about 3d 23h".to_string(),
        };
        let png = render(&data).expect("card renders");
        let decoded = Pixmap::decode_png(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
        if let Ok(path) = std::env::var("PET_CARD_PREVIEW") {
            std::fs::write(path, png).unwrap();
        }
    }
}
