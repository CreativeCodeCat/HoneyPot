//! `/server-setup`: stores the channels a server uses for logs, commands, games and
//! announcements. Other cogs read them back with `Store::get`.

use std::time::Duration;

use poise::serenity_prelude as serenity;
use poise::ChoiceParameter;
use rusqlite::{params, Connection, OptionalExtension};

use super::{Context, Data, Error};

const DB_FILE: &str = "database/server_setup.db";
const PURPLE: u32 = 0x9b59b6;
const RED: u32 = 0xed4245;
const BLANK: u32 = 0x2b2d31;
/// How long the confirmation posted in a newly set channel stays before it's deleted
const CONFIRM_DELETE_AFTER: Duration = Duration::from_secs(5);

/// Per-guild settings, keyed like "logs.message" or "channel.games", holding a channel ID.
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
            "CREATE TABLE IF NOT EXISTS settings (
                guild_id INTEGER NOT NULL,
                key TEXT NOT NULL,
                -- Channel or role ID
                value INTEGER NOT NULL,
                PRIMARY KEY (guild_id, key)
            );
            -- Settings from /server-setup options that were removed, as nothing used them
            DELETE FROM settings WHERE key LIKE 'status.%' OR key LIKE 'command.%'
                OR key IN ('channel.counting', 'channel.invite') OR key LIKE 'roles.%';",
        )?;
        Ok(Self { conn })
    }

    pub fn get(&self, guild_id: u64, key: &str) -> rusqlite::Result<Option<u64>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE guild_id = ? AND key = ?", params![guild_id, key], |row| row.get(0))
            .optional()
    }

    pub fn set(&self, guild_id: u64, key: &str, value: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, key, value) VALUES (?, ?, ?)",
            params![guild_id, key, value],
        )?;
        Ok(())
    }

    pub fn all(&self, guild_id: u64) -> rusqlite::Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings WHERE guild_id = ? ORDER BY key")?;
        let rows = stmt.query_map([guild_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Every guild that has `key` set: (guild_id, value).
    pub fn everywhere(&self, key: &str) -> rusqlite::Result<Vec<(u64, u64)>> {
        let mut stmt = self.conn.prepare("SELECT guild_id, value FROM settings WHERE key = ?")?;
        let rows = stmt.query_map([key], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Removes every setting for a guild, returning how many there were.
    pub fn reset(&self, guild_id: u64) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM settings WHERE guild_id = ?", [guild_id])
    }
}

#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum LogType {
    Channel,
    Events,
    Emoji,
    Guild,
    #[name = "Join & Leave"]
    JoinLeave,
    Member,
    Message,
    Role,
    Sticker,
    Thread,
    User,
    Voice,
    /// The bot starting up
    Ready,
    /// A friendly welcome for each new member
    Welcome,
    /// Members starting and stopping server boosts
    Boosts,
}

/// Cogs whose commands work in any channel: setup commands and moderation (used where the
/// trouble is)
const ANY_CHANNEL_COGS: &[&str] = &["CogManager", "Moderation", "StaffPolicy"];

/// Members who can manage the server, so moderation commands work where they're needed.
async fn is_staff(ctx: Context<'_>) -> bool {
    if let poise::Context::Application(app) = ctx {
        if let Some(perms) = app.interaction.member.as_ref().and_then(|m| m.permissions) {
            return perms.administrator() || perms.manage_guild();
        }
    }
    let Some(member) = ctx.author_member().await else { return false };
    ctx.guild().is_some_and(|guild| {
        let perms = guild.member_permissions(&member);
        perms.administrator() || perms.manage_guild()
    })
}

/// Whether a message calls one of the bot's prefix commands, like `!ping`.
fn is_prefix_command(content: &str, commands: &[poise::Command<Data, Error>]) -> bool {
    let Some(name) = content.strip_prefix('!').and_then(|rest| rest.split_whitespace().next()) else { return false };
    let name = name.to_lowercase();
    commands.iter().any(|c| c.prefix_action.is_some() && (c.name == name || c.aliases.contains(&name)))
}

/// Keeps the commands and games channels for commands only: deletes every message that
/// isn't a prefix command, from admins too. Bots' messages (like command replies) stay.
/// Returns true if the message was deleted.
pub async fn on_message(ctx: &serenity::Context, data: &Data, msg: &serenity::Message, commands: &[poise::Command<Data, Error>]) -> bool {
    let Some(guild_id) = msg.guild_id else { return false };
    if msg.author.bot || is_prefix_command(&msg.content, commands) {
        return false;
    }
    // The commands and games channels are only for using the bot's commands
    let channel = msg.channel_id.get();
    let kept_clear = {
        let setup = data.server_setup.lock().unwrap();
        ["channel.commands", GAMES_CHANNEL].into_iter().find(|key| setup.get(guild_id.get(), key).is_ok_and(|id| id == Some(channel)))
    };
    let Some(key) = kept_clear else { return false };

    if let Err(e) = msg.delete(ctx).await {
        let which = if key == GAMES_CHANNEL { "games" } else { "commands" };
        crate::log::error(format!("Couldn't delete a message in the {which} channel (needs Manage Messages): {e}"));
        return false;
    }
    true
}

/// Where game commands go once set with `/server-setup games`
const GAMES_CHANNEL: &str = "channel.games";
/// Games with their own channels, set with `/counting` and `/guessing`
const OWN_CHANNEL_GAMES: &[&str] = &["counting", "guessing"];

/// Framework-wide check: once a games channel is set, games can only be played there and
/// other commands can't be used there; and once a commands channel is set, members can only
/// use other commands there. Staff, and some cogs' commands, are only held to the games rule.
pub async fn commands_channel_check(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else { return Ok(()) };
    let root = super::root_command(ctx).name.as_str();
    let is_game = super::command_category(ctx) == Some("Games")
        && super::games::GAMES.contains(&root)
        && !OWN_CHANNEL_GAMES.contains(&root);
    if is_game {
        let games_channel = ctx.data().server_setup.lock().unwrap().get(guild_id.get(), GAMES_CHANNEL)?;
        match games_channel {
            Some(channel) if channel != ctx.channel_id().get() => {
                return Err(format!("🎮 Games are played in <#{channel}>. Head over there to play `/{root}`!").into());
            }
            // In the games channel, even if it isn't the commands channel
            Some(_) => return Ok(()),
            None => {}
        }
    }
    if super::command_category(ctx).is_some_and(|cog| ANY_CHANNEL_COGS.contains(&cog)) {
        return Ok(());
    }
    // `/ticket` manages the ticket it's used in, so it only works inside ticket channels;
    // `/ticket-setup` follows the commands channel rule
    if root == "ticket" {
        let in_ticket = ctx.data().tickets.lock().unwrap().open_ticket(ctx.channel_id().get())?.is_some();
        return if in_ticket { Ok(()) } else { Err("🎫 `/ticket` only works inside a ticket channel.".into()) };
    }
    let (games_channel, commands_channel) = {
        let setup = ctx.data().server_setup.lock().unwrap();
        (setup.get(guild_id.get(), GAMES_CHANNEL)?, setup.get(guild_id.get(), "channel.commands")?)
    };
    // The games channel is only for games, so other commands are sent elsewhere
    if games_channel == Some(ctx.channel_id().get()) && !is_staff(ctx).await {
        return Err(match commands_channel {
            Some(channel) => format!("🎮 This channel is only for games. Use `/{root}` in <#{channel}> instead."),
            None => format!("🎮 This channel is only for games. Use `/{root}` in another channel instead."),
        }
        .into());
    }
    match commands_channel {
        Some(channel) if channel != ctx.channel_id().get() && !is_staff(ctx).await => {
            Err(format!("❌ Commands can only be used in <#{channel}>.").into())
        }
        _ => Ok(()),
    }
}

/// The settings key for a choice, e.g. `key("logs", "Join & Leave")` -> "logs.joinleave".
fn key(group: &str, choice: &str) -> String {
    format!("{group}.{}", choice.to_lowercase().replace(" & ", "").replace(' ', ""))
}

fn embed(colour: u32, text: impl Into<String>) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new().colour(colour).description(text)
}

async fn reply(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Saves a channel setting, posts a short-lived confirmation in that channel and replies privately.
async fn set_channel(ctx: Context<'_>, key: &str, channel: &serenity::GuildChannel, what: &str) -> Result<(), Error> {
    let posted = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed(PURPLE, format!("✅ | Successfully setup the {what}.")))).await;
    let message = match posted {
        Ok(message) => message,
        Err(_) => return reply(ctx, embed(RED, format!("❌ The bot can't send messages in <#{}>.", channel.id))).await,
    };
    ctx.data().server_setup.lock().unwrap().set(ctx.guild_id().unwrap().get(), key, channel.id.get())?;

    let http = ctx.serenity_context().http.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CONFIRM_DELETE_AFTER).await;
        message.delete(&http).await.ok();
    });
    reply(ctx, embed(PURPLE, format!("✅ | Successfully setup the {what}: <#{}>.", channel.id))).await
}

/// Setup or reset the channels the bot uses on this server
#[poise::command(
    slash_command,
    guild_only,
    rename = "server-setup",
    category = "CogManager",
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR",
    subcommands("logs", "commands", "games_channel", "announcements", "reset", "help"),
    subcommand_required
)]
pub async fn server_setup(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Setup the server logs channels
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn logs(
    ctx: Context<'_>,
    #[description = "The type of log to setup"] r#type: LogType,
    #[description = "The channel to send log messages to"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    let name = r#type.name();
    set_channel(ctx, &key("logs", name), &channel, &format!("{name} logs")).await
}

/// Setup the server commands channel
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn commands(
    ctx: Context<'_>,
    #[description = "The channel to run server commands in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    set_channel(ctx, "channel.commands", &channel, "server commands channel").await
}

/// Setup the channel games are played in
#[poise::command(slash_command, guild_only, rename = "games", required_permissions = "ADMINISTRATOR")]
async fn games_channel(
    ctx: Context<'_>,
    #[description = "The channel to play games like /hack, /heist, /fight and /defuse in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    set_channel(ctx, GAMES_CHANNEL, &channel, "games channel").await
}

/// Setup the server announcements channel
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn announcements(
    ctx: Context<'_>,
    #[description = "The channel to post server announcements in"]
    #[channel_types("Text", "News")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    set_channel(ctx, "channel.announcements", &channel, "server announcements channel").await
}

/// Reset all of the channels
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn reset(ctx: Context<'_>) -> Result<(), Error> {
    let removed = ctx.data().server_setup.lock().unwrap().reset(ctx.guild_id().unwrap().get())?;
    if removed == 0 {
        return reply(ctx, embed(RED, "❌ There is no setup data to reset.")).await;
    }
    reply(ctx, embed(PURPLE, "✅ | Successfully reset the server setup.")).await
}

/// Get help with the server-setup commands, and see what's set
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn help(ctx: Context<'_>) -> Result<(), Error> {
    let settings = ctx.data().server_setup.lock().unwrap().all(ctx.guild_id().unwrap().get())?;
    let current = if settings.is_empty() {
        "Nothing has been setup yet.".to_string()
    } else {
        settings
            .iter()
            .map(|(key, id)| format!("`{key}` → <#{id}>"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let bot = ctx.cache().current_user().clone();
    let help = serenity::CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(&bot.name).icon_url(bot.face()))
        .description(
            "**Help Panel**\n\n\
             > Get help with the commands: `/server-setup`!\n\n\
             > `help` • Get information about the server-setup commands.\n\
             > `logs` • Setup the server logs channels.\n\
             > `commands` • Setup the server commands channel.\n\
             > `games` • Setup the channel games are played in.\n\
             > `announcements` • Setup the server announcements channel.\n\
             > `reset` • Reset all of the channels.",
        )
        .field("Current Setup", current, false)
        .colour(BLANK)
        .timestamp(serenity::Timestamp::now());
    reply(ctx, help).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_flat() {
        assert_eq!(key("logs", LogType::JoinLeave.name()), "logs.joinleave");
    }

    #[test]
    fn set_get_and_reset() {
        let s = Store::open_path(":memory:").unwrap();
        s.set(1, "logs.message", 10).unwrap();
        s.set(1, "logs.message", 11).unwrap();
        s.set(2, "channel.games", 20).unwrap();
        assert_eq!(s.get(1, "logs.message").unwrap(), Some(11));
        assert_eq!(s.all(1).unwrap().len(), 1);
        assert_eq!(s.reset(1).unwrap(), 1);
        assert_eq!(s.get(1, "logs.message").unwrap(), None);
        assert_eq!(s.get(2, "channel.games").unwrap(), Some(20));
    }

    #[test]
    fn only_real_prefix_commands_are_kept() {
        let commands = crate::commands();
        assert!(is_prefix_command("!ping", &commands));
        assert!(is_prefix_command("!PING now", &commands));
        assert!(!is_prefix_command("!notacommand", &commands));
        assert!(!is_prefix_command("hello", &commands));

    }
}
