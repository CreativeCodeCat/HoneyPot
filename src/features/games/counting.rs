//! Counting: members count up one number at a time in the server's counting
//! channel, taking turns. A wrong number, or counting twice in a row, ruins the
//! count, the bot pokes fun at whoever did it, and it starts again from 1. Messages
//! that don't start with a number are deleted to keep the channel tidy.

use poise::serenity_prelude as serenity;
use rand::Rng;

use super::super::{Context, Data, Error};
use crate::log;

pub use crate::database::repositories::counting::*;

const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;
const BLUE: u32 = 0x3498db;

/// Said when someone ruins the count. `{user}` is them, `{number}` what they
/// should have said, `{count}` how far the count got.
const RUINED: &[&str] = &[
    "💥 {user} ruined it at **{count}**! The next number was **{number}**... Back to **1** we go.",
    "🤦 Oh no, {user}! It was **{number}**. The count is back to **1**.",
    "📉 {user} took the count from **{count}** straight down to zero. Start again at **1**!",
    "🧮 Did {user} fall asleep in maths class? **{number}** was next. Back to **1**!",
    "🔥 {user} set **{count}** numbers on fire. Count from **1** again!",
    "🙈 We don't talk about what {user} just did. The next number was **{number}**; start over at **1**.",
    "🐌 So close, {user}... it was **{number}**. Everyone back to **1**!",
    "🎺 *Sad trombone* for {user}, who forgot **{number}** comes after **{count}**. Back to **1**.",
];

/// Said when someone counts twice in a row.
const TWICE: &[&str] = &[
    "✋ {user}, you can't count twice in a row! The count got to **{count}**; back to **1**.",
    "🙄 Let someone else have a go, {user}! Counting twice in a row ruined it at **{count}**. Start again at **1**.",
    "🚫 Two in a row, {user}? Greedy! The count is back to **1**.",
];

/// Said when someone ruins the count before it even starts.
const NOT_STARTED: &[&str] = &[
    "😅 {user}, the count starts at **1**!",
    "🤔 Nice try, {user}, but we start at **1** around here.",
];

/// The number a message starts with, so "12 nice" counts as 12.
fn parse(content: &str) -> Option<u64> {
    content.split_whitespace().next()?.replace(',', "").parse().ok()
}

fn pick(lines: &[&'static str]) -> &'static str {
    lines[rand::rng().random_range(0..lines.len())]
}

/// Checks numbers posted in the counting channel.
pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild_id) = message.guild_id else { return };
    if message.author.bot {
        return;
    }
    let enabled = {
        let config = data.cog_config.lock().unwrap();
        config.is_enabled(guild_id.get(), "games") && config.is_game_enabled(guild_id.get(), "counting")
    };
    if !enabled {
        return;
    }
    let in_channel = data.counting.lock().unwrap().get(guild_id.get()).map(|g| g.is_some_and(|g| g.channel_id == message.channel_id.get()));
    if !matches!(in_channel, Ok(true)) {
        return;
    }
    let Some(number) = parse(&message.content) else {
        // Only numbers belong in the counting channel
        let _ = message.delete(ctx).await;
        return;
    };
    let outcome = data.counting.lock().unwrap().count(guild_id.get(), message.channel_id.get(), message.author.id.get(), number);
    let outcome = match outcome {
        Ok(Some(outcome)) => outcome,
        Ok(None) => return,
        Err(e) => return log::error(format!("Error counting in guild {guild_id}: {e}")),
    };

    let (emoji, line, count) = match outcome {
        Outcome::Counted(record) => {
            let _ = message.react(ctx, if record { '🏆' } else { '✅' }).await;
            return;
        }
        Outcome::Twice { count } => ('❌', pick(TWICE), count),
        Outcome::Wrong { count: 0 } => ('❌', pick(NOT_STARTED), 0),
        Outcome::Wrong { count } => ('❌', pick(RUINED), count),
    };
    let _ = message.react(ctx, emoji).await;
    let text = line
        .replace("{count}", &count.to_string())
        .replace("{number}", &(count + 1).to_string())
        .replace("{user}", &format!("<@{}>", message.author.id));
    let embed = serenity::CreateEmbed::new().colour(RED).description(text);
    if let Err(e) = message.channel_id.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await {
        log::error(format!("Error posting in counting channel in guild {guild_id}: {e}"));
    }
}

async fn reply(ctx: Context<'_>, colour: u32, text: impl Into<String>) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// The counting game
#[poise::command(
    slash_command,
    guild_only,
    category = "Games",
    subcommands("setup", "stop", "stats"),
    subcommand_required
)]
pub async fn counting(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Choose the channel members count in (starts the count again)
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn setup(
    ctx: Context<'_>,
    #[description = "Channel to count in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    ctx.data().counting.lock().unwrap().set_channel(ctx.guild_id().unwrap().get(), channel.id.get())?;
    let embed = serenity::CreateEmbed::new()
        .colour(BLUE)
        .title("🔢 Counting")
        .description("Count up one number at a time, taking turns. Get it wrong, or count twice in a row, and it's back to the start!\n\nStart with **1**.");
    channel.id.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await?;
    reply(ctx, GREEN, format!("✅ Counting is set up in <#{}>.", channel.id)).await
}

/// Stop the counting game in this server
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn stop(ctx: Context<'_>) -> Result<(), Error> {
    if ctx.data().counting.lock().unwrap().stop(ctx.guild_id().unwrap().get())? {
        reply(ctx, GREEN, "✅ Counting has been stopped. The high score is kept for when `/counting setup` starts it again.").await
    } else {
        reply(ctx, RED, "❌ Counting isn't set up in this server.").await
    }
}

/// See the current count and the high score
#[poise::command(slash_command, guild_only, ephemeral)]
async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let game = ctx.data().counting.lock().unwrap().get(ctx.guild_id().unwrap().get())?;
    let Some(game) = game.clone().filter(|g| g.channel_id != 0) else {
        let high_score = game.map(|g| format!(" The high score is **{}**.", g.high_score)).unwrap_or_default();
        return reply(ctx, RED, format!("❌ Counting isn't running in this server.{high_score}")).await;
    };
    let last = game.last_user.map(|id| format!("\nLast counted by <@{id}>")).unwrap_or_default();
    let text = format!(
        "Counting in <#{}>\nCurrent count: **{}**\nNext number: **{}**\nHigh score: **{}**{last}",
        game.channel_id,
        game.count,
        game.count + 1,
        game.high_score
    );
    reply(ctx, BLUE, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_goes_up_and_resets() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.count(1, 5, 10, 1).unwrap(), None);
        s.set_channel(1, 5).unwrap();
        // Other channels don't count
        assert_eq!(s.count(1, 6, 10, 1).unwrap(), None);

        assert_eq!(s.count(1, 5, 10, 1).unwrap(), Some(Outcome::Counted(true)));
        assert_eq!(s.count(1, 5, 10, 2).unwrap(), Some(Outcome::Twice { count: 1 }));
        assert_eq!(s.count(1, 5, 10, 1).unwrap(), Some(Outcome::Counted(false)));
        assert_eq!(s.count(1, 5, 11, 2).unwrap(), Some(Outcome::Counted(true)));
        assert_eq!(s.count(1, 5, 10, 4).unwrap(), Some(Outcome::Wrong { count: 2 }));
        let game = s.get(1).unwrap().unwrap();
        assert_eq!((game.count, game.last_user, game.high_score), (0, None, 2));

        // A new channel starts again but keeps the high score
        s.count(1, 5, 10, 1).unwrap();
        s.set_channel(1, 7).unwrap();
        assert_eq!(s.get(1).unwrap().unwrap(), Game { channel_id: 7, count: 0, last_user: None, high_score: 2 });

        // Stopping keeps the high score, and nothing counts until it's set up again
        assert!(s.stop(1).unwrap());
        assert!(!s.stop(1).unwrap());
        assert_eq!(s.count(1, 7, 10, 1).unwrap(), None);
        assert_eq!(s.get(1).unwrap().unwrap().high_score, 2);
        s.set_channel(1, 7).unwrap();
        assert_eq!(s.count(1, 7, 10, 1).unwrap(), Some(Outcome::Counted(false)));
    }

    #[test]
    fn messages_start_with_the_number() {
        assert_eq!(parse("12"), Some(12));
        assert_eq!(parse("1,000 woo"), Some(1000));
        assert_eq!(parse("hello 3"), None);
        assert_eq!(parse("-1"), None);
    }
}
