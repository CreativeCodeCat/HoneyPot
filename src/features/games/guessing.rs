//! Guessing: the bot thinks of a number from 1 to `MAX_NUMBER`, and members guess it
//! by posting numbers in the server's guessing channel. Wrong guesses get a 🔼 or
//! 🔽 reaction as a hint; whoever gets it wins money for their wallet, and the bot
//! thinks of a new number. Anything else posted in the channel is deleted.

use poise::serenity_prelude as serenity;

use super::super::economy::money;
use super::super::economy::store::{EcoError, Pocket};
use super::super::{Context, Data, Error};
use crate::log;

pub use crate::database::repositories::guessing::*;

/// Seconds each member waits between guesses
pub const GUESS_COOLDOWN_SECS: i64 = 15;
/// Money paid into the winner's wallet
const REWARD: i64 = 500;

const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;
const BLUE: u32 = 0x3498db;

/// The guess a message is: one whole number from 1 to `MAX_NUMBER` and nothing else.
fn parse(content: &str) -> Option<u32> {
    content.trim().parse().ok().filter(|n| (1..=MAX_NUMBER).contains(n))
}

/// Checks guesses posted in the guessing channel.
pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild_id) = message.guild_id else { return };
    if message.author.bot {
        return;
    }
    let enabled = {
        let config = data.cog_config.lock().unwrap();
        config.is_enabled(guild_id.get(), "games") && config.is_game_enabled(guild_id.get(), "guessing")
    };
    if !enabled {
        return;
    }
    let in_channel = data.guessing.lock().unwrap().get(guild_id.get()).map(|g| g.is_some_and(|g| g.channel_id == message.channel_id.get()));
    if !matches!(in_channel, Ok(true)) {
        return;
    }
    let Some(number) = parse(&message.content) else {
        // Only guesses belong in the guessing channel
        let _ = message.delete(ctx).await;
        return;
    };
    let now = chrono::Utc::now().timestamp();
    let cooldown = data.cooldowns.lock().unwrap().start(guild_id.get(), message.author.id.get(), "guessing", now);
    match cooldown {
        Ok(Ok(())) => {}
        // Too soon: the guess doesn't count
        Ok(Err(_)) => {
            let _ = message.react(ctx, '⏳').await;
            return;
        }
        Err(e) => return log::error(format!("Error checking guessing cooldown in guild {guild_id}: {e}")),
    }
    let outcome = data.guessing.lock().unwrap().guess(guild_id.get(), message.channel_id.get(), number);
    let guesses = match outcome {
        Ok(Some(Outcome::Found(guesses))) => guesses,
        Ok(Some(Outcome::Higher(higher))) => {
            let _ = message.react(ctx, if higher { '🔼' } else { '🔽' }).await;
            return;
        }
        Ok(None) => return,
        Err(e) => return log::error(format!("Error guessing in guild {guild_id}: {e}")),
    };

    let _ = message.react(ctx, '🎉').await;
    let user = message.author.id;
    let prize = match data.economy.lock().unwrap().adjust(guild_id.get(), user.get(), Pocket::Wallet, REWARD) {
        Ok(Ok(_)) => format!("{} has been added to your wallet.", money(REWARD)),
        Ok(Err(EcoError::NoAccount)) => format!("Join with `/eco join` to win {} next time.", money(REWARD)),
        Ok(Err(_)) => "Your wallet is full, so the prize stays put.".to_string(),
        Err(e) => {
            log::error(format!("Error paying guessing prize in guild {guild_id}: {e}"));
            String::new()
        }
    };
    let tries = if guesses == 1 { "first guess".to_string() } else { format!("{guesses} guesses") };
    let text = format!(
        "🎉 <@{user}> got it! The number was **{number}**, found in {tries}. {prize}\n\n\
         I'm thinking of a new number from 1 to {MAX_NUMBER}..."
    );
    let embed = serenity::CreateEmbed::new().colour(GREEN).description(text);
    if let Err(e) = message.channel_id.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await {
        log::error(format!("Error posting in guessing channel in guild {guild_id}: {e}"));
    }
}

async fn reply(ctx: Context<'_>, colour: u32, text: impl Into<String>) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// The number guessing game
#[poise::command(
    slash_command,
    guild_only,
    category = "Games",
    subcommands("setup", "stop", "stats"),
    subcommand_required
)]
pub async fn guessing(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Choose the channel members guess in (picks a new number)
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn setup(
    ctx: Context<'_>,
    #[description = "Channel to guess in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    ctx.data().guessing.lock().unwrap().set_channel(ctx.guild_id().unwrap().get(), channel.id.get())?;
    let embed = serenity::CreateEmbed::new().colour(BLUE).title("🎲 Guess the Number").description(format!(
        "I'm thinking of a number from **1** to **{MAX_NUMBER}**. Post a number to guess it!\n\n\
         🔼 means it's higher, 🔽 means it's lower. Whoever gets it wins {}.\n\
         You can guess once every {GUESS_COOLDOWN_SECS} seconds; ⏳ means you guessed too soon.",
        money(REWARD)
    ));
    channel.id.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await?;
    reply(ctx, GREEN, format!("✅ Guessing is set up in <#{}>.", channel.id)).await
}

/// Stop the guessing game in this server
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn stop(ctx: Context<'_>) -> Result<(), Error> {
    if ctx.data().guessing.lock().unwrap().stop(ctx.guild_id().unwrap().get())? {
        reply(ctx, GREEN, "✅ Guessing has been stopped. The stats are kept for when `/guessing setup` starts it again.").await
    } else {
        reply(ctx, RED, "❌ Guessing isn't running in this server.").await
    }
}

/// See how the guessing game is going
#[poise::command(slash_command, guild_only, ephemeral)]
async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let Some(game) = ctx.data().guessing.lock().unwrap().get(ctx.guild_id().unwrap().get())? else {
        return reply(ctx, RED, "❌ Guessing isn't set up in this server.").await;
    };
    let channel = if game.channel_id == 0 { "Stopped".to_string() } else { format!("Guessing in <#{}>", game.channel_id) };
    let best = if game.best == 0 { "none yet".to_string() } else { format!("**{}** guesses", game.best) };
    let text = format!(
        "{channel}\nGuesses for this number: **{}**\nNumbers found: **{}**\nFewest guesses: {best}",
        game.guesses, game.rounds
    );
    reply(ctx, BLUE, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_lead_to_the_number() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.guess(1, 5, 1).unwrap(), None);
        s.set_channel(1, 5).unwrap();
        assert_eq!(s.guess(1, 6, 1).unwrap(), None);

        // Halving the range always finds it within 7 guesses
        let (mut low, mut high) = (1, MAX_NUMBER);
        let found = loop {
            let guess = (low + high) / 2;
            match s.guess(1, 5, guess).unwrap().unwrap() {
                Outcome::Found(guesses) => break guesses,
                Outcome::Higher(true) => low = guess + 1,
                Outcome::Higher(false) => high = guess - 1,
            }
        };
        assert!(found <= 7);
        let game = s.get(1).unwrap().unwrap();
        assert_eq!((game.guesses, game.rounds, game.best), (0, 1, found));

        // Stopping keeps the stats, and nothing counts until it's set up again
        assert!(s.stop(1).unwrap());
        assert_eq!(s.guess(1, 5, 1).unwrap(), None);
        s.set_channel(1, 5).unwrap();
        assert_eq!(s.get(1).unwrap().unwrap().rounds, 1);
    }

    #[test]
    fn only_numbers_in_range_are_guesses() {
        assert_eq!(parse(" 7 "), Some(7));
        assert_eq!(parse("100"), Some(100));
        assert_eq!(parse("0"), None);
        assert_eq!(parse("101"), None);
        assert_eq!(parse("7 maybe"), None);
    }
}
