//! `/defuse`: a bomb with five coloured wires and a timer. One wire defuses it, one sets it
//! off, and the other three are dummies that make the timer jump forward. A clue says which
//! wire the defuse wire sits next to, so a careful player can narrow it down to two.
//!
//! An optional bet is held in the economy database until the game ends, and refunded if
//! the bot stops before then. Defusing pays the bet back doubled.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use ::serenity::all as serenity;
use rand::seq::SliceRandom;
use rand::Rng;

use crate::core::framework::{command, CreateReply};
use super::super::economy::money;
use super::super::economy::store::EcoError;
use super::super::{title_case, Context, Error};

/// Default seconds between games for each member; `/cooldown set defuse` changes it
pub const DEFUSE_COOLDOWN_SECS: i64 = 60;
/// How long the bomb's timer starts at
const FUSE: Duration = Duration::from_secs(30);
/// How far cutting a dummy wire jumps the timer forward
const DUMMY_PENALTY: Duration = Duration::from_secs(8);
const MIN_BET: i64 = 10;
const MAX_BET: i64 = 1000;

const ORANGE: u32 = 0xe67e22;
const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wire {
    Red,
    Blue,
    Green,
    Yellow,
    White,
}

impl Wire {
    const ALL: [Self; 5] = [Self::Red, Self::Blue, Self::Green, Self::Yellow, Self::White];

    fn emoji(self) -> char {
        match self {
            Self::Red => '🟥',
            Self::Blue => '🟦',
            Self::Green => '🟩',
            Self::Yellow => '🟨',
            Self::White => '⬜',
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::White => "white",
        }
    }
}

/// What a wire does when it's cut
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Defuse,
    Detonate,
    Dummy,
}

/// The wires in the order they're shown, what each does, and the clue.
struct Bomb {
    wires: Vec<(Wire, Role)>,
    /// A wire next to the defuse wire
    clue: Wire,
}

impl Bomb {
    fn new(rng: &mut impl Rng) -> Self {
        let mut colours = Wire::ALL.to_vec();
        colours.shuffle(rng);
        let mut roles = vec![Role::Defuse, Role::Detonate, Role::Dummy, Role::Dummy, Role::Dummy];
        roles.shuffle(rng);
        let wires: Vec<(Wire, Role)> = colours.into_iter().zip(roles).collect();
        let defuse = wires.iter().position(|(_, role)| *role == Role::Defuse).unwrap();
        let neighbours: Vec<usize> = [defuse.checked_sub(1), Some(defuse + 1)].into_iter().flatten().filter(|i| *i < wires.len()).collect();
        let clue = wires[neighbours[rng.random_range(0..neighbours.len())]].0;
        Self { wires, clue }
    }

    fn role(&self, wire: Wire) -> Role {
        self.wires.iter().find(|(w, _)| *w == wire).map_or(Role::Dummy, |(_, role)| *role)
    }
}

/// Members playing right now, so each has one bomb at a time.
static PLAYING: LazyLock<Mutex<HashSet<(u64, u64)>>> = LazyLock::new(Default::default);

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Defuse a bomb by cutting the right wire, optionally betting on it
#[command(slash_command, guild_only, category = "Games")]
pub async fn defuse(
    ctx: Context<'_>,
    #[description = "How much of your wallet to bet; defusing pays it back doubled"]
    #[min = 10]
    #[max = 1000]
    bet: Option<i64>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let user_id = ctx.author().id.get();
    if let Some(bet) = bet {
        if !(MIN_BET..=MAX_BET).contains(&bet) {
            return say_private(ctx, format!("❌ Bets must be between {} and {}.", money(MIN_BET), money(MAX_BET))).await;
        }
    }
    if !PLAYING.lock().unwrap().insert((guild_id, user_id)) {
        return say_private(ctx, "❌ You're already defusing a bomb! Finish that one first.").await;
    }
    let result = play(ctx, guild_id, user_id, bet).await;
    PLAYING.lock().unwrap().remove(&(guild_id, user_id));
    if result.is_err() && bet.is_some() {
        // Give back the bet if the game broke before it was settled
        let _ = ctx.data().economy.lock().unwrap().refund_one(guild_id, user_id, "defuse");
    }
    result
}

async fn play(ctx: Context<'_>, guild_id: u64, user_id: u64, bet: Option<i64>) -> Result<(), Error> {
    if let Some(bet) = bet {
        let held = ctx.data().economy.lock().unwrap().hold(guild_id, user_id, "defuse", bet)?;
        if let Err(e) = held {
            let text = match e {
                EcoError::NoAccount => "❌ You need an economy account to bet. Use `/eco join` first, or play without a bet.".to_string(),
                EcoError::NotEnough(available) => format!("❌ You can't bet {}, your wallet only has {}.", money(bet), money(available)),
                _ => "❌ Your bet didn't go through.".to_string(),
            };
            return say_private(ctx, text).await;
        }
    }

    let bomb = Bomb::new(&mut rand::rng());
    let prefix = ctx.id();
    let wire_id = |wire: Wire| format!("{prefix}defuse_{}", wire.name());
    let mut cut: Vec<Wire> = vec![];
    let mut deadline = tokio::time::Instant::now() + FUSE;
    let ends_at = |deadline: tokio::time::Instant| {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now()).as_secs() as i64;
        serenity::Timestamp::now().unix_timestamp() + left
    };

    let wires_row = |cut: &[Wire], enabled: bool| {
        let buttons = bomb
            .wires
            .iter()
            .map(|(wire, _)| {
                serenity::CreateButton::new(wire_id(*wire))
                    .emoji(wire.emoji())
                    .label(if cut.contains(wire) { "Cut".to_string() } else { title_case(wire.name()) })
                    .style(if cut.contains(wire) { serenity::ButtonStyle::Secondary } else { serenity::ButtonStyle::Primary })
                    .disabled(!enabled || cut.contains(wire))
            })
            .collect();
        vec![serenity::CreateActionRow::Buttons(buttons)]
    };
    let wires_text = |cut: &[Wire]| {
        bomb.wires
            .iter()
            .map(|(wire, _)| if cut.contains(wire) { "✂️".to_string() } else { wire.emoji().to_string() })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let stake = bet.map_or(String::new(), |bet| format!("\n**Bet:** {}", money(bet)));
    let board = |cut: &[Wire], note: &str, ends: i64| {
        serenity::CreateEmbed::new()
            .colour(ORANGE)
            .title("💣 Defuse the Bomb")
            .description(format!(
                "<@{user_id}>, cut the right wire before it goes off! One wire defuses it, one sets it off, and the rest make the timer jump.\n\n\
                 {}\n\n🔍 **Clue:** the defuse wire is right next to the **{}** wire.\n⏱️ **Explodes** <t:{ends}:R>{stake}{note}",
                wires_text(cut),
                bomb.clue.name()
            ))
    };
    let reply = ctx.send(CreateReply::default().embed(board(&cut, "", ends_at(deadline))).components(wires_row(&cut, true))).await?;

    let ids: Vec<String> = Wire::ALL.iter().map(|w| wire_id(*w)).collect();
    let outcome = loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        let ids = ids.clone();
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .filter(move |i| ids.contains(&i.data.custom_id))
            .timeout(left)
            .await;
        let Some(press) = press else { break None };
        if press.user.id.get() != user_id {
            let message = serenity::CreateInteractionResponseMessage::new().content("❌ This isn't your bomb! Start your own with `/defuse`.").ephemeral(true);
            press.create_response(ctx, serenity::CreateInteractionResponse::Message(message)).await?;
            continue;
        }
        let Some(wire) = Wire::ALL.into_iter().find(|w| wire_id(*w) == press.data.custom_id) else { continue };
        cut.push(wire);
        match bomb.role(wire) {
            Role::Dummy => {
                deadline -= DUMMY_PENALTY;
                let note = format!("\n\n✂️ You cut the **{}** wire... nothing happened, but the timer jumped forward!", wire.name());
                let update = serenity::CreateInteractionResponseMessage::new()
                    .embed(board(&cut, &note, ends_at(deadline)))
                    .components(wires_row(&cut, true));
                press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
            }
            role => {
                press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
                break Some((wire, role));
            }
        }
    };

    let won = matches!(outcome, Some((_, Role::Defuse)));
    let (title, mut text, colour) = match outcome {
        Some((wire, Role::Defuse)) => ("✅ Bomb Defused!", format!("<@{user_id}> cut the **{}** wire and the timer stopped. Nice work!", wire.name()), GREEN),
        Some((wire, _)) => ("💥 BOOM!", format!("<@{user_id}> cut the **{}** wire... it was the wrong one.", wire.name()), RED),
        None => ("💥 BOOM!", format!("<@{user_id}> ran out of time."), RED),
    };
    let answer = bomb.wires.iter().find(|(_, role)| *role == Role::Defuse).map_or("", |(wire, _)| wire.name());
    let detonator = bomb.wires.iter().find(|(_, role)| *role == Role::Detonate).map_or("", |(wire, _)| wire.name());
    if !won {
        text += &format!("\nThe defuse wire was **{answer}**, and the **{detonator}** wire set it off.");
    }
    if let Some(bet) = bet {
        let payout = if won { bet * 2 } else { 0 };
        let settled = ctx.data().economy.lock().unwrap().settle(guild_id, user_id, "defuse", payout)?;
        text += &match (won, settled) {
            (true, Ok(())) => format!("\n💰 You won {}!", money(payout)),
            (true, Err(_)) => "\n💰 Your wallet is full, so your winnings stay put.".to_string(),
            (false, _) => format!("\n💸 You lost your bet of {}.", money(bet)),
        };
    }
    let embed = serenity::CreateEmbed::new().colour(colour).title(title).description(format!("{}\n\n{text}", wires_text(&cut)));
    reply.edit(ctx, CreateReply::default().embed(embed).components(wires_row(&cut, false))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bombs_have_one_of_each_and_an_honest_clue() {
        let mut rng = rand::rng();
        for _ in 0..200 {
            let bomb = Bomb::new(&mut rng);
            let count = |role| bomb.wires.iter().filter(|(_, r)| *r == role).count();
            assert_eq!((count(Role::Defuse), count(Role::Detonate), count(Role::Dummy)), (1, 1, 3));
            let defuse = bomb.wires.iter().position(|(_, r)| *r == Role::Defuse).unwrap();
            let clue = bomb.wires.iter().position(|(w, _)| *w == bomb.clue).unwrap();
            assert_eq!(defuse.abs_diff(clue), 1);
        }
    }
}
