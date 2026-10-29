//! Sticky cog: `/sticky stick` keeps a message near the bottom of a channel. Once
//! enough messages have been posted below it, the old copy is deleted and it's
//! posted again at the bottom.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use super::{Context, Data, Error};
use crate::log;

pub const COG_NAME: &str = "sticky";
const DB_FILE: &str = "database/sticky.db";
/// Messages posted below the sticky before it's moved down, if not given
const DEFAULT_COUNT: u32 = 3;
const FOOTER: &str = "This is a sticky message";
const COLOUR: u32 = 0x2b2d31;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sticky {
    pub channel_id: u64,
    pub text: String,
    /// Messages posted below it before it's moved down
    pub max_count: u32,
    pub message_id: u64,
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
            "CREATE TABLE IF NOT EXISTS stickies (
                channel_id INTEGER PRIMARY KEY,
                guild_id INTEGER NOT NULL,
                text TEXT NOT NULL,
                max_count INTEGER NOT NULL,
                -- Messages posted since it was last moved down
                count INTEGER NOT NULL DEFAULT 0,
                message_id INTEGER NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Sticky> {
        Ok(Sticky { channel_id: row.get(0)?, text: row.get(1)?, max_count: row.get(2)?, message_id: row.get(3)? })
    }

    pub fn get(&self, channel_id: u64) -> rusqlite::Result<Option<Sticky>> {
        self.conn
            .query_row("SELECT channel_id, text, max_count, message_id FROM stickies WHERE channel_id = ?", [channel_id], Self::from_row)
            .optional()
    }

    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<Sticky>> {
        let mut stmt = self.conn.prepare("SELECT channel_id, text, max_count, message_id FROM stickies WHERE guild_id = ?")?;
        let rows = stmt.query_map([guild_id], Self::from_row)?;
        rows.collect()
    }

    pub fn add(&self, guild_id: u64, sticky: &Sticky) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stickies (channel_id, guild_id, text, max_count, message_id) VALUES (?, ?, ?, ?, ?)",
            params![sticky.channel_id, guild_id, sticky.text, sticky.max_count, sticky.message_id],
        )?;
        Ok(())
    }

    /// Returns false if the channel had no sticky.
    pub fn remove(&self, channel_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM stickies WHERE channel_id = ?", [channel_id])? > 0)
    }

    /// Counts a message posted in the channel. Returns the sticky if it's due to be
    /// moved down, starting the count again.
    pub fn bump(&self, channel_id: u64) -> rusqlite::Result<Option<Sticky>> {
        let due = self
            .conn
            .query_row(
                "UPDATE stickies SET count = CASE WHEN count + 1 >= max_count THEN 0 ELSE count + 1 END
                 WHERE channel_id = ? RETURNING count = 0",
                [channel_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        match due {
            Some(true) => self.get(channel_id),
            _ => Ok(None),
        }
    }

    pub fn set_message(&self, channel_id: u64, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE stickies SET message_id = ? WHERE channel_id = ?", [message_id, channel_id])?;
        Ok(())
    }

    /// Removes stickies for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute_batch(&format!("DELETE FROM stickies WHERE guild_id NOT IN ({})", ids.join(",")))
    }
}

fn embed(text: &str) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .colour(COLOUR)
        .description(text)
        .footer(serenity::CreateEmbedFooter::new(FOOTER))
        .timestamp(serenity::Timestamp::now())
}

/// Channels whose sticky is being moved down right now, so it only moves once.
static MOVING: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Default::default);

/// Moves the channel's sticky to the bottom once enough messages are posted below it.
pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild_id) = message.guild_id else { return };
    // The sticky itself doesn't count
    let is_sticky = message.author.id == ctx.cache.current_user().id
        && message.embeds.iter().any(|e| e.footer.as_ref().is_some_and(|f| f.text == FOOTER));
    if is_sticky || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let channel_id = message.channel_id.get();
    let sticky = match data.sticky.lock().unwrap().bump(channel_id) {
        Ok(Some(sticky)) => sticky,
        Ok(None) => return,
        Err(e) => return log::error(format!("Error reading sticky for channel {channel_id}: {e}")),
    };
    if !MOVING.lock().unwrap().insert(channel_id) {
        return;
    }

    let channel = message.channel_id;
    // Already gone if someone deleted it
    let _ = channel.delete_message(ctx, sticky.message_id).await;
    match channel.send_message(ctx, serenity::CreateMessage::new().embed(embed(&sticky.text))).await {
        Ok(new) => {
            if let Err(e) = data.sticky.lock().unwrap().set_message(channel_id, new.id.get()) {
                log::error(format!("Error saving sticky for channel {channel_id}: {e}"));
            }
        }
        Err(e) => log::error(format!("Error reposting sticky in channel {channel_id}: {e}")),
    }
    MOVING.lock().unwrap().remove(&channel_id);
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Keep a message at the bottom of a channel
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    category = "Sticky",
    required_permissions = "MANAGE_MESSAGES",
    default_member_permissions = "MANAGE_MESSAGES",
    subcommands("stick", "unstick", "list"),
    subcommand_required
)]
pub async fn sticky(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Stick a message to the bottom of this channel
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn stick(
    ctx: Context<'_>,
    #[description = "The message to keep at the bottom (use \\n for new lines)"]
    #[max_length = 4000]
    message: String,
    #[description = "How many messages can be posted below it before it moves down (default 3)"]
    #[min = 1]
    #[max = 100]
    count: Option<u32>,
) -> Result<(), Error> {
    let channel = ctx.channel_id();
    if ctx.data().sticky.lock().unwrap().get(channel.get())?.is_some() {
        return say_private(ctx, "❌ This channel already has a sticky message. Remove it with `/sticky unstick` first.").await;
    }
    let text = message.replace("\\n", "\n");
    let posted = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed(&text))).await?;
    let sticky = Sticky { channel_id: channel.get(), text, max_count: count.unwrap_or(DEFAULT_COUNT).max(1), message_id: posted.id.get() };
    ctx.data().sticky.lock().unwrap().add(ctx.guild_id().unwrap().get(), &sticky)?;
    say_private(ctx, "✅ The sticky message has been set up!").await
}

/// Remove this channel's sticky message
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn unstick(ctx: Context<'_>) -> Result<(), Error> {
    let channel = ctx.channel_id();
    let Some(sticky) = ctx.data().sticky.lock().unwrap().get(channel.get())? else {
        return say_private(ctx, "❌ This channel doesn't have a sticky message. Make one with `/sticky stick`.").await;
    };
    ctx.data().sticky.lock().unwrap().remove(channel.get())?;
    // Already gone if someone deleted it
    let _ = channel.delete_message(ctx, sticky.message_id).await;
    say_private(ctx, "✅ The sticky message in this channel has been removed.").await
}

/// List this server's sticky messages
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let stickies = ctx.data().sticky.lock().unwrap().list(ctx.guild_id().unwrap().get())?;
    if stickies.is_empty() {
        return say_private(ctx, "This server has no sticky messages.").await;
    }
    let lines: Vec<String> = stickies
        .iter()
        .map(|s| {
            let preview = s.text.lines().next().unwrap_or_default();
            let preview = if preview.chars().count() > 80 { format!("{}…", super::truncate(preview, 80)) } else { preview.to_string() };
            format!("<#{}> (moves after {} messages): {preview}", s.channel_id, s.max_count)
        })
        .collect();
    let pages = crate::pagination::chunk_lines(&lines, 10)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().colour(COLOUR).title("Sticky Messages").description(text))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_moves_after_enough_messages() {
        let s = Store::open_path(":memory:").unwrap();
        let sticky = Sticky { channel_id: 5, text: "Read the rules".into(), max_count: 3, message_id: 100 };
        s.add(1, &sticky).unwrap();
        assert_eq!(s.bump(6).unwrap(), None);
        assert_eq!(s.bump(5).unwrap(), None);
        assert_eq!(s.bump(5).unwrap(), None);
        assert_eq!(s.bump(5).unwrap(), Some(sticky.clone()));
        // The count starts again
        assert_eq!(s.bump(5).unwrap(), None);

        s.set_message(5, 101).unwrap();
        assert_eq!(s.list(1).unwrap()[0].message_id, 101);
        assert!(s.remove(5).unwrap());
        assert!(!s.remove(5).unwrap());
    }
}
