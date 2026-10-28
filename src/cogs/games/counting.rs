//! Counting: members count up one number at a time in the server's counting
//! channel, taking turns. A wrong number, or counting twice in a row, ruins the
//! count, the bot pokes fun at whoever did it, and it starts again from 1.

use poise::serenity_prelude as serenity;
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};

use super::super::{Context, Data, Error};
use crate::log;

const DB_FILE: &str = "database/counting.db";

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    /// 0 while counting is stopped, which keeps the high score for next time
    pub channel_id: u64,
    /// The last number counted, 0 before anyone has
    pub count: u64,
    pub last_user: Option<u64>,
    pub high_score: u64,
}

/// What a number posted in the counting channel did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Right number; true if it's a new high score
    Counted(bool),
    /// Wrong number after the count reached `count`
    Wrong { count: u64 },
    /// Same member twice in a row after the count reached `count`
    Twice { count: u64 },
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS counting (
                guild_id INTEGER PRIMARY KEY,
                channel_id INTEGER NOT NULL,
                count INTEGER NOT NULL DEFAULT 0,
                last_user INTEGER,
                high_score INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        Ok(Self { conn })
    }

    pub fn get(&self, guild_id: u64) -> rusqlite::Result<Option<Game>> {
        self.conn
            .query_row(
                "SELECT channel_id, count, last_user, high_score FROM counting WHERE guild_id = ?",
                [guild_id],
                |row| Ok(Game { channel_id: row.get(0)?, count: row.get(1)?, last_user: row.get(2)?, high_score: row.get(3)? }),
            )
            .optional()
    }

    /// Sets the counting channel, starting the count again but keeping the high score.
    pub fn set_channel(&self, guild_id: u64, channel_id: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO counting (guild_id, channel_id) VALUES (?1, ?2)
             ON CONFLICT (guild_id) DO UPDATE SET channel_id = ?2, count = 0, last_user = NULL",
            params![guild_id, channel_id],
        )?;
        Ok(())
    }

    /// Stops counting, keeping the high score. Returns false if it wasn't running.
    pub fn stop(&self, guild_id: u64) -> rusqlite::Result<bool> {
        let stopped = self.conn.execute(
            "UPDATE counting SET channel_id = 0, count = 0, last_user = NULL WHERE guild_id = ? AND channel_id != 0",
            [guild_id],
        )?;
        Ok(stopped > 0)
    }

    /// Plays `number` from `user`, or `None` if `channel_id` isn't the counting channel.
    pub fn count(&self, guild_id: u64, channel_id: u64, user: u64, number: u64) -> rusqlite::Result<Option<Outcome>> {
        let Some(game) = self.get(guild_id)?.filter(|g| g.channel_id == channel_id) else { return Ok(None) };
        let outcome = if game.last_user == Some(user) {
            Outcome::Twice { count: game.count }
        } else if number != game.count + 1 {
            Outcome::Wrong { count: game.count }
        } else {
            let record = number > game.high_score;
            self.conn.execute(
                "UPDATE counting SET count = ?, last_user = ?, high_score = MAX(high_score, ?) WHERE guild_id = ?",
                params![number, user, number, guild_id],
            )?;
            return Ok(Some(Outcome::Counted(record)));
        };
        self.conn.execute("UPDATE counting SET count = 0, last_user = NULL WHERE guild_id = ?", [guild_id])?;
        Ok(Some(outcome))
    }

    /// Removes games in guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute_batch(&format!("DELETE FROM counting WHERE guild_id NOT IN ({})", ids.join(",")))
    }
}

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
    let Some(number) = parse(&message.content) else { return };
    let enabled = {
        let config = data.cog_config.lock().unwrap();
        config.is_enabled(guild_id.get(), "games") && config.is_game_enabled(guild_id.get(), "counting")
    };
    if !enabled {
        return;
    }
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
