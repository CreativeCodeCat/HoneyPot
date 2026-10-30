//! Per-guild configuration that the core reads on every command.

use std::collections::HashMap;

use ::serenity::all as serenity;
use rusqlite::{params, Connection};

use crate::database::connection;
use crate::features::{games, COG_MANAGER};

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
        let conn = connection::open(DB_FILE)?;
        conn.execute_batch(include_str!("../../migrations/guild_config.sql"))?;

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
