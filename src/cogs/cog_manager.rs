use std::collections::HashMap;

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection};

use super::{normalize_cog_name, Context, Error, COGS, COG_MANAGER};

const DB_FILE: &str = "database/cog_config.db";

/// Per-guild cog enabled/disabled flags, cached in memory and persisted to SQLite.
/// Cogs default to enabled when a guild has no entry.
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
        self.config
            .get(&guild_id)
            .and_then(|cogs| cogs.get(cog_name))
            .copied()
            .unwrap_or(true)
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
    ctx.say(format!(
        "✅ Cog '{cog_name}' has been enabled for this server."
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
    ctx.say(format!(
        "✅ Cog '{cog_name}' has been disabled for this server."
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
