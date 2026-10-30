//! `/fight`: challenge another member, and once they accept, the two trade blows
//! until one runs out of health. Either can bet money on it; the winner takes both
//! wagers. Both fighters then need `/cooldown set fight` seconds (10 minutes by
//! default) to recover.
//!
//! Challenging the bot skips the accept. Wagers are held in the economy database,
//! so they come back if the bot stops mid-fight.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use poise::serenity_prelude as serenity;
use rand::Rng;

use super::super::economy::money;
use super::super::economy::store::EcoError;
use super::super::{Context, Error};

/// Default seconds each fighter recovers after a fight; `/cooldown set fight` changes it
pub const FIGHT_COOLDOWN_SECS: i64 = 10 * 60;
const ACCEPT_TIME: Duration = Duration::from_secs(60);
/// Pause between the moves that are shown
const MOVE_DELAY: Duration = Duration::from_secs(5);
/// Percent chance an attack lands
const ATTACK_SUCCESS: u32 = 65;
const MIN_DAMAGE: i64 = 1;
const MAX_DAMAGE: i64 = 5;
const HEALTH: i64 = 10;
/// Moves shown before the dust cloud hides the rest of the fight
const CAPTIONS: usize = 4;

const RED: u32 = 0xed4245;
const GREEN: u32 = 0x57f287;
const GREY: u32 = 0x95a5a6;

/// `{a}` attacks, `{d}` defends, `{dmg}` is the damage
const ATTACKS: &[&str] = &[
    "{a} slashes {d} with their claws for {dmg} damage.",
    "{a} mauls {d} and does {dmg} damage.",
    "{a} bodyslams {d} for {dmg} damage. That's gotta hurt...",
    "{a} headbutts {d} for {dmg} damage.",
    "An artful juke from {a}, followed by a punch. {d} takes {dmg} damage.",
    "{a} did something and it caused {dmg} damage to {d}. We're not sure what happened there...",
    "{a} slams {d} into a tree for {dmg} damage.",
    "{d} tries to stab {a}, but {a} parries with a quick jab to the ribs for {dmg} damage.",
    "{a} throws a tree trunk onto {d}. Ouch. {dmg} damage.",
    "{d} is confused and trips. {dmg} damage to self.",
];

/// `{a}` attacks, `{d}` defends
const DODGES: &[&str] = &[
    "{a} tries punching {d}, but {d} squats to dodge it.",
    "{d} dodge-rolls out of the slash of {a}.",
    "{d} sidesteps the charge from {a}.",
    "{a} growls ferociously... {d} seems unaffected.",
    "{d} manages to block {a}'s attack somehow.",
    "{a} slips up and misses the attack on {d}.",
    "{a} used SPLASH!... But nothing happened.",
    "{d} glares dangerously at {a}, making them incapable of action.",
    "{a} is thinking about their next move... Then forgets to do anything.",
];

/// Guilds with a fight on, as there's one arena per server.
static ARENAS: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Default::default);

/// Holds a guild's arena, freeing it when dropped, even if the fight errors.
struct Arena(u64);

impl Arena {
    fn take(guild_id: u64) -> Option<Self> {
        ARENAS.lock().unwrap().insert(guild_id).then_some(Self(guild_id))
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        ARENAS.lock().unwrap().remove(&self.0);
    }
}

/// A whole fight: the moves in order, and whether the challenger won.
fn simulate(challenger: &str, opponent: &str) -> (Vec<String>, bool) {
    let mut rng = rand::rng();
    let mut health = [HEALTH, HEALTH];
    let names = [challenger, opponent];
    let mut moves = vec![];
    let mut last = usize::MAX;
    // The challenger goes first
    let mut turn = 0;
    loop {
        let (a, d) = (names[turn], names[1 - turn]);
        let (list, damage) = if rng.random_range(0..100) < ATTACK_SUCCESS {
            (ATTACKS, rng.random_range(MIN_DAMAGE..=MAX_DAMAGE))
        } else {
            (DODGES, 0)
        };
        // Never the same line twice in a row
        let mut index = rng.random_range(0..list.len());
        if index == last {
            index = (index + 1) % list.len();
        }
        last = index;
        moves.push(list[index].replace("{a}", a).replace("{d}", d).replace("{dmg}", &damage.to_string()));

        health[1 - turn] -= damage;
        if health[1 - turn] <= 0 {
            return (moves, turn == 0);
        }
        turn = 1 - turn;
    }
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

fn wager_error(e: EcoError, who: &str, wager: i64) -> String {
    match e {
        EcoError::NoAccount => format!("❌ {who} need an economy account to bet. Use `/eco join` first."),
        EcoError::NotEnough(available) => {
            format!("❌ {who} can't bet {}, there's only {} in the wallet.", money(wager), money(available))
        }
        _ => "❌ The wager didn't go through.".to_string(),
    }
}

/// Challenge someone to a fight
#[poise::command(slash_command, prefix_command, guild_only, category = "Games")]
pub async fn fight(
    ctx: Context<'_>,
    #[description = "Who to fight"] user: serenity::User,
    #[description = "Money each of you bets; the winner takes it all"]
    #[min = 0]
    wager: Option<i64>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let author = ctx.author().id;
    let wager = wager.unwrap_or(0).max(0);
    let data = ctx.data();
    let vs_bot = user.id == ctx.framework().bot_id;

    if user.id == author {
        return say_private(ctx, "❌ You can't fight yourself.").await;
    }
    if user.bot && !vs_bot {
        return say_private(ctx, "❌ That bot doesn't want to fight.").await;
    }
    let now = Utc::now().timestamp();
    let recovering = {
        let cooldowns = data.cooldowns.lock().unwrap();
        (cooldowns.ready_at(guild_id, author.get(), "fight", now)?, cooldowns.ready_at(guild_id, user.id.get(), "fight", now)?)
    };
    match recovering {
        (Some(ready), _) => {
            return say_private(ctx, format!("❌ You're still recovering from your last fight. Try again <t:{ready}:R>.")).await;
        }
        (_, Some(ready)) => return say_private(ctx, format!("❌ <@{}> is still recovering. Try again <t:{ready}:R>.", user.id)).await,
        _ => {}
    }
    let Some(_arena) = Arena::take(guild_id) else {
        return say_private(ctx, "❌ There's already a fight in the arena. Wait for it to finish!").await;
    };
    if wager > 0 {
        let held = data.economy.lock().unwrap().hold(guild_id, author.get(), "fight", wager)?;
        if let Err(e) = held {
            return say_private(ctx, wager_error(e, "You", wager)).await;
        }
    }
    // Gives back whichever wagers were taken
    let refund = |users: &[serenity::UserId]| -> Result<(), Error> {
        for id in users {
            if wager > 0 {
                let _ = data.economy.lock().unwrap().settle(guild_id, id.get(), "fight", wager)?;
            }
        }
        Ok(())
    };

    let bet = if wager > 0 { format!(" for {}", money(wager)) } else { String::new() };
    let embed = |text: String, colour| serenity::CreateEmbed::new().colour(colour).description(text);
    let (accept_id, decline_id) = (format!("{}fight_accept", ctx.id()), format!("{}fight_decline", ctx.id()));
    let buttons = |disabled: bool| {
        vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&accept_id).label("Accept").style(serenity::ButtonStyle::Success).emoji('⚔').disabled(disabled),
            serenity::CreateButton::new(&decline_id).label("Decline").style(serenity::ButtonStyle::Secondary).disabled(disabled),
        ])]
    };

    let reply = if vs_bot {
        let text = format!("⚔️ <@{author}> is asking <@{}> for a fight{bet}! Challenge accepted... Things are about to heat up in the arena!", user.id);
        ctx.send(poise::CreateReply::default().embed(embed(text, RED))).await?
    } else {
        let text = format!(
            "⚔️ <@{author}> is asking <@{}> for a fight{bet}!\n<@{}>, do you accept? (expires <t:{}:R>)",
            user.id,
            user.id,
            now + ACCEPT_TIME.as_secs() as i64
        );
        let reply = ctx
            .send(poise::CreateReply::default().content(format!("<@{}>", user.id)).embed(embed(text, RED)).components(buttons(false)))
            .await?;

        let deadline = Instant::now() + ACCEPT_TIME;
        let accepted = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (accept, decline) = (accept_id.clone(), decline_id.clone());
            let Some(press) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
                .filter(move |i| i.data.custom_id == accept || i.data.custom_id == decline)
                .timeout(remaining)
                .await
            else {
                break None;
            };
            if press.user.id != user.id {
                let not_yours = serenity::CreateInteractionResponseMessage::new().content("This fight isn't for you.").ephemeral(true);
                press.create_response(ctx, serenity::CreateInteractionResponse::Message(not_yours)).await?;
                continue;
            }
            // The opponent bets when they accept
            if press.data.custom_id == accept_id && wager > 0 {
                let held = data.economy.lock().unwrap().hold(guild_id, user.id.get(), "fight", wager)?;
                if let Err(e) = held {
                    let why = serenity::CreateInteractionResponseMessage::new().content(wager_error(e, "You", wager)).ephemeral(true);
                    press.create_response(ctx, serenity::CreateInteractionResponse::Message(why)).await?;
                    continue;
                }
            }
            press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
            break Some(press.data.custom_id == accept_id);
        };

        let finish = |text: String, colour| poise::CreateReply::default().content("").embed(embed(text, colour)).components(buttons(true));
        if accepted != Some(true) {
            refund(&[author])?;
            let why = if accepted.is_none() { "hasn't responded" } else { "declined" };
            let text = format!("<@{}> {why}, so <@{author}>'s fight is off. The arena is free again!", user.id);
            reply.edit(ctx, finish(text, GREY)).await?;
            return Ok(());
        }
        let text = format!("⚔️ <@{}> accepted <@{author}>'s fight{bet}... Things are about to heat up in the arena!", user.id);
        reply.edit(ctx, finish(text, RED)).await?;
        reply
    };

    let result = async {
        let (moves, challenger_won) = simulate(&format!("<@{author}>"), &format!("<@{}>", user.id));
        let mut text = String::new();
        for line in moves.iter().take(CAPTIONS) {
            tokio::time::sleep(MOVE_DELAY).await;
            text = format!("{text}{line}\n");
            reply.edit(ctx, poise::CreateReply::default().content("").embed(embed(text.clone(), RED))).await?;
        }
        if moves.len() > CAPTIONS {
            tokio::time::sleep(MOVE_DELAY).await;
            text = format!("{text}*The dust cloud from the fight gets so thick it hides the rest of it...*\n");
            reply.edit(ctx, poise::CreateReply::default().content("").embed(embed(text.clone(), RED))).await?;
        }
        tokio::time::sleep(MOVE_DELAY).await;

        let winner = if challenger_won { author } else { user.id };
        let mut ending = format!("🏆 The fight is over! The winner is <@{winner}>! Congratulations!");
        if wager > 0 {
            // Against the bot only the challenger bets, but a win pays the same
            let fighters: &[serenity::UserId] = if vs_bot { &[author] } else { &[author, user.id] };
            let mut full = false;
            for &id in fighters {
                let payout = if id == winner { wager * 2 } else { 0 };
                full |= data.economy.lock().unwrap().settle(guild_id, id.get(), "fight", payout)?.is_err();
            }
            if full {
                ending = format!("{ending} Their wallet is full, so the winnings stay put.");
            } else if fighters.contains(&winner) {
                ending = format!("{ending} You've won {}.", money(wager * 2));
            }
        }
        {
            let cooldowns = data.cooldowns.lock().unwrap();
            let now = Utc::now().timestamp();
            // Can't still be recovering: that was checked before the fight
            let _ = cooldowns.start(guild_id, author.get(), "fight", now)?;
            if !vs_bot {
                let _ = cooldowns.start(guild_id, user.id.get(), "fight", now)?;
            }
        }
        text = format!("{text}\n{ending}");
        reply.edit(ctx, poise::CreateReply::default().content("").embed(embed(text, GREEN))).await?;
        Ok::<_, Error>(())
    }
    .await;
    if result.is_err() {
        // Settled wagers are gone from the hold, so this only returns what wasn't paid out
        if let Err(e) = data.economy.lock().unwrap().refund_held(Some((guild_id, "fight"))) {
            crate::log::error(format!("Error refunding fight wagers in guild {guild_id}: {e}"));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fights_end_with_a_winner() {
        for _ in 0..200 {
            let (moves, _) = simulate("A", "B");
            assert!(!moves.is_empty());
            assert!(moves.iter().all(|m| !m.contains('{')));
            assert!(moves.windows(2).all(|w| w[0] != w[1]));
        }
    }
}
