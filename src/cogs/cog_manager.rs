use std::collections::HashMap;

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection};

use super::{games, normalize_cog_name, Context, Data, Error, COGS, COG_MANAGER};

const DB_FILE: &str = "database/cog_config.db";

/// Per-guild cog enabled/disabled flags, cached in memory and persisted to SQLite.
/// Cogs default to disabled when a guild has no entry, except the cog
/// manager, which is always enabled. Single games are stored as "game:<name>"
/// and are disabled by default too, so each one is turned on with `/enablegame`.
pub struct CogConfig {
    conn: Connection,
    config: HashMap<u64, HashMap<String, bool>>,
}

impl CogConfig {
    pub fn open() -> rusqlite::Result<Self> {
        let conn = Connection::open(DB_FILE)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cog_config (
                guild_id INTEGER,
                cog_name TEXT,
                enabled BOOLEAN DEFAULT 1,
                PRIMARY KEY (guild_id, cog_name)
            )",
        )?;

        let mut config: HashMap<u64, HashMap<String, bool>> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT guild_id, cog_name, enabled FROM cog_config")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })?;
            for row in rows {
                let (guild_id, cog_name, enabled) = row?;
                config
                    .entry(guild_id)
                    .or_default()
                    .insert(cog_name, enabled);
            }
        }

        Ok(Self { conn, config })
    }

    pub fn is_enabled(&self, guild_id: u64, cog_name: &str) -> bool {
        cog_name == COG_MANAGER
            || self
                .config
                .get(&guild_id)
                .and_then(|cogs| cogs.get(cog_name))
                .copied()
                .unwrap_or(false)
    }

    /// Whether a game command is on; the Games cog must be enabled as well.
    /// Commands in the Games cog that aren't games (like `/gamestatus`) count as enabled.
    pub fn is_game_enabled(&self, guild_id: u64, game: &str) -> bool {
        !games::GAMES.contains(&game)
            || self.config.get(&guild_id).and_then(|cogs| cogs.get(&game_key(game))).copied().unwrap_or(false)
    }

    pub fn set_enabled(
        &mut self,
        guild_id: u64,
        cog_name: &str,
        enabled: bool,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO cog_config (guild_id, cog_name, enabled) VALUES (?, ?, ?)",
            params![guild_id, cog_name, enabled],
        )?;
        self.config
            .entry(guild_id)
            .or_default()
            .insert(cog_name.to_string(), enabled);
        Ok(())
    }

    /// Removes config for guilds the bot is no longer in.
    pub fn cleanup_stale(&mut self, current_guilds: &[serenity::GuildId]) {
        let stale: Vec<u64> = self
            .config
            .keys()
            .copied()
            .filter(|id| !current_guilds.iter().any(|g| g.get() == *id))
            .collect();

        for guild_id in stale {
            crate::log::warning(format!(
                "Guild {guild_id} no longer exists, removing from cog config"
            ));
            if let Err(e) = self
                .conn
                .execute("DELETE FROM cog_config WHERE guild_id = ?", [guild_id])
            {
                crate::log::error(format!("Error cleaning up stale cog config: {e}"));
                continue;
            }
            self.config.remove(&guild_id);
        }
    }
}

pub(crate) fn game_key(game: &str) -> String {
    format!("game:{game}")
}

/// Slash and context menu registrations for the commands that pass `keep`, given
/// their cog and name. Commands without a cog count as CogManager, so they're always available.
pub fn application_commands(commands: &[poise::Command<Data, Error>], keep: impl Fn(&str, &str) -> bool) -> Vec<serenity::CreateCommand> {
    commands
        .iter()
        .filter(|command| keep(&command.category.as_deref().map_or(COG_MANAGER.to_string(), normalize_cog_name), &command.name))
        .flat_map(|command| [command.create_as_slash_command(), command.create_as_context_menu_command()])
        .flatten()
        .collect()
}

/// Registers the CogManager commands globally; every other cog's commands are per guild.
pub async fn sync_global_commands(http: &serenity::Http, commands: &[poise::Command<Data, Error>]) -> serenity::Result<usize> {
    let global = application_commands(commands, |cog, _| cog == COG_MANAGER);
    let count = global.len();
    serenity::Command::set_global_commands(http, global).await?;
    Ok(count)
}

/// Gives a guild the commands of the cogs (and games) it has enabled, removing the rest.
pub async fn sync_guild_commands(
    http: &serenity::Http,
    commands: &[poise::Command<Data, Error>],
    data: &Data,
    guild_id: serenity::GuildId,
) -> serenity::Result<usize> {
    let enabled = {
        let config = data.cog_config.lock().unwrap();
        application_commands(commands, |cog, name| {
            cog != COG_MANAGER
                && config.is_enabled(guild_id.get(), cog)
                && (cog != "games" || config.is_game_enabled(guild_id.get(), name))
        })
    };
    let count = enabled.len();
    guild_id.set_commands(http, enabled).await?;
    Ok(count)
}

/// Re-syncs the current guild's commands after a cog (or game) is enabled or disabled.
pub(crate) async fn resync(ctx: Context<'_>) -> String {
    let guild_id = ctx.guild_id().unwrap();
    match sync_guild_commands(ctx.http(), &ctx.framework().options().commands, ctx.data(), guild_id).await {
        Ok(_) => String::new(),
        Err(e) => {
            crate::log::error(format!("Error syncing commands for guild {guild_id}: {e}"));
            "\n⚠️ Couldn't update the server's slash commands; they'll update on the next restart.".to_string()
        }
    }
}

async fn autocomplete_cog_name(
    _ctx: Context<'_>,
    partial: &str,
) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    COGS.iter()
        .filter(|cog| {
            normalize_cog_name(cog) != COG_MANAGER && cog.to_lowercase().contains(&partial)
        })
        .map(|cog| serenity::AutocompleteChoice::new(*cog, normalize_cog_name(cog)))
        .collect()
}

/// Resolves user input to a known cog's config name.
fn resolve_cog(input: &str) -> Option<String> {
    let input = normalize_cog_name(input);
    COGS.iter()
        .map(|cog| normalize_cog_name(cog))
        .find(|cog| *cog == input)
}

/// Enable a cog for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn enablecog(
    ctx: Context<'_>,
    #[description = "Cog to enable"]
    #[autocomplete = "autocomplete_cog_name"]
    cog_name: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let Some(cog_name) = resolve_cog(&cog_name) else {
        ctx.say(format!("❌ Unknown cog '{cog_name}'.")).await?;
        return Ok(());
    };

    ctx.data()
        .cog_config
        .lock()
        .unwrap()
        .set_enabled(guild_id, &cog_name, true)?;
    let warning = resync(ctx).await;
    let note = if cog_name == "games" { "\nℹ️ Every game starts off; turn on the ones you want with `/enablegame`." } else { "" };
    ctx.say(format!(
        "✅ Cog '{cog_name}' has been enabled for this server.{note}{warning}"
    ))
    .await?;
    Ok(())
}

/// Disable a cog for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn disablecog(
    ctx: Context<'_>,
    #[description = "Cog to disable"]
    #[autocomplete = "autocomplete_cog_name"]
    cog_name: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let Some(cog_name) = resolve_cog(&cog_name) else {
        ctx.say(format!("❌ Unknown cog '{cog_name}'.")).await?;
        return Ok(());
    };

    if cog_name == COG_MANAGER {
        ctx.say("❌ You cannot disable the CogManager cog.").await?;
        return Ok(());
    }

    ctx.data()
        .cog_config
        .lock()
        .unwrap()
        .set_enabled(guild_id, &cog_name, false)?;
    let warning = resync(ctx).await;
    ctx.say(format!(
        "✅ Cog '{cog_name}' has been disabled for this server.{warning}"
    ))
    .await?;
    Ok(())
}

/// Check the status of all cogs for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn cogstatus(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let status_msg = {
        let config = ctx.data().cog_config.lock().unwrap();
        COGS.iter()
            .map(|cog| {
                let status = if config.is_enabled(guild_id, &normalize_cog_name(cog)) {
                    "✅ Enabled"
                } else {
                    "❌ Disabled"
                };
                format!("{status}: {cog}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = serenity::CreateEmbed::new()
        .title("Cog Status")
        .description(status_msg)
        .colour(0x3498db);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cog_manager_commands_are_global() {
        let commands = crate::commands();
        let uncategorised: Vec<_> = commands.iter().filter(|c| c.category.is_none()).map(|c| c.name.clone()).collect();
        assert!(uncategorised.is_empty(), "no cog: {uncategorised:?}");
        let global = application_commands(&commands, |cog, _| cog == COG_MANAGER);
        let cog_manager = commands.iter().filter(|c| c.category.as_deref() == Some("CogManager")).count();
        assert_eq!(global.len(), cog_manager);
    }

    #[test]
    fn every_game_can_be_toggled() {
        let mut games: Vec<_> = crate::commands().into_iter().filter(|c| c.category.as_deref() == Some("Games") && !["enablegame", "disablegame", "gamestatus"].contains(&c.name.as_str())).map(|c| c.name).collect();
        games.sort();
        assert_eq!(games, games::GAMES);
    }
}
