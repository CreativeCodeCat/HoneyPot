//! `/server-setup`: stores the channels and roles a server uses for logs, status, counting,
//! announcements and so on. Other cogs read them back with `Store::get`.

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

/// Per-guild settings, keyed like "logs.message" or "roles.bots", holding a channel or role ID.
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
            )",
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
}

#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum RoleType {
    Bots,
    Partners,
    Pro,
    Supporters,
    Helpers,
    Welcome,
}

#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum StatusType {
    Members,
    Bots,
    Roles,
    Boosts,
    Channels,
    Guilds,
}

#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum CommandType {
    Apply,
}

/// Cogs whose commands work in any channel: setup commands, and tickets (used inside ticket channels)
const ANY_CHANNEL_COGS: &[&str] = &["CogManager", "Tickets"];

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

/// Keeps the commands channel for commands only: deletes every message that isn't a
/// prefix command, from admins too. Bots' messages (like command replies) stay.
/// Returns true if the message was deleted.
pub async fn on_message(ctx: &serenity::Context, data: &Data, msg: &serenity::Message, commands: &[poise::Command<Data, Error>]) -> bool {
    let Some(guild_id) = msg.guild_id else { return false };
    if msg.author.bot || is_prefix_command(&msg.content, commands) {
        return false;
    }
    let commands_channel = match data.server_setup.lock().unwrap().get(guild_id.get(), "channel.commands") {
        Ok(channel) => channel,
        Err(e) => {
            crate::log::error(format!("Error reading the commands channel: {e}"));
            return false;
        }
    };
    if commands_channel != Some(msg.channel_id.get()) {
        return false;
    }

    if let Err(e) = msg.delete(ctx).await {
        crate::log::error(format!("Couldn't delete a message in the commands channel (needs Manage Messages): {e}"));
        return false;
    }
    true
}

/// Framework-wide check: once a commands channel is set, members can only use commands there.
pub async fn commands_channel_check(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else { return Ok(()) };
    if ctx.command().category.as_deref().is_some_and(|cog| ANY_CHANNEL_COGS.contains(&cog)) {
        return Ok(());
    }
    let commands_channel = ctx.data().server_setup.lock().unwrap().get(guild_id.get(), "channel.commands")?;
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

/// Setup or reset the channels and roles the bot uses on this server
#[poise::command(
    slash_command,
    guild_only,
    rename = "server-setup",
    category = "CogManager",
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR",
    subcommands("logs", "roles", "status", "command", "commands", "counting", "announcements", "invite", "reset", "help"),
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

/// Setup the server roles
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn roles(
    ctx: Context<'_>,
    #[description = "The type of role to setup"] r#type: RoleType,
    #[description = "The role to give to that type"] role: serenity::Role,
) -> Result<(), Error> {
    let name = r#type.name();
    ctx.data().server_setup.lock().unwrap().set(ctx.guild_id().unwrap().get(), &key("roles", name), role.id.get())?;
    reply(ctx, embed(PURPLE, format!("✅ | Successfully setup the {name} role: <@&{}>.", role.id))).await
}

/// Setup the server status channels
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn status(
    ctx: Context<'_>,
    #[description = "The type of status to setup"] r#type: StatusType,
    #[description = "The channel to send status messages to"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    let name = r#type.name();
    set_channel(ctx, &key("status", name), &channel, &format!("{name} status channel")).await
}

/// Setup the channel for a select command's output
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn command(
    ctx: Context<'_>,
    #[description = "The command to setup"] r#type: CommandType,
    #[description = "The channel to send command output to"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    let name = r#type.name();
    set_channel(ctx, &key("command", name), &channel, &format!("{name} command channel")).await
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

/// Setup the server counting channel
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn counting(
    ctx: Context<'_>,
    #[description = "The channel to run server counting in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    set_channel(ctx, "channel.counting", &channel, "server counting channel").await
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

/// Setup the server invite channel
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn invite(
    ctx: Context<'_>,
    #[description = "The channel to get invites from"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    set_channel(ctx, "channel.invite", &channel, "server invite channel").await
}

/// Reset all of the channels and roles
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
            .map(|(key, id)| if key.starts_with("roles.") { format!("`{key}` → <@&{id}>") } else { format!("`{key}` → <#{id}>") })
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
             > `roles` • Setup the server roles.\n\
             > `status` • Setup the server status channels.\n\
             > `command` • Setup the channel for a select command output.\n\
             > `commands` • Setup the server commands channel.\n\
             > `counting` • Setup the server counting channel.\n\
             > `announcements` • Setup the server announcements channel.\n\
             > `invite` • Setup the server invite channel.\n\
             > `reset` • Reset all of the channels and roles.",
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
        assert_eq!(key("roles", RoleType::Bots.name()), "roles.bots");
    }

    #[test]
    fn set_get_and_reset() {
        let s = Store::open_path(":memory:").unwrap();
        s.set(1, "logs.message", 10).unwrap();
        s.set(1, "logs.message", 11).unwrap();
        s.set(2, "roles.bots", 20).unwrap();
        assert_eq!(s.get(1, "logs.message").unwrap(), Some(11));
        assert_eq!(s.all(1).unwrap().len(), 1);
        assert_eq!(s.reset(1).unwrap(), 1);
        assert_eq!(s.get(1, "logs.message").unwrap(), None);
        assert_eq!(s.get(2, "roles.bots").unwrap(), Some(20));
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
