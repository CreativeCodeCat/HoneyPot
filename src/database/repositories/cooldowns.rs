//! Per-server command cooldowns.

use std::collections::HashMap;
use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::{connection, migrations};
use crate::features::games;

/// Cooldowns for commands that had one before they could be configured
pub const DEFAULTS: &[(&str, u64)] = &[
    ("ping", 300),
    ("suggest", 300),
    ("hack", games::HACK_COOLDOWN_SECS as u64),
    ("heist", games::heist::HEIST_COOLDOWN_SECS as u64),
    ("defuse", games::defuse::DEFUSE_COOLDOWN_SECS as u64),
    ("fight", games::fight::FIGHT_COOLDOWN_SECS as u64),
    // Guesses are messages, so this is started by the guessing game itself
    ("guessing", games::guessing::GUESS_COOLDOWN_SECS as u64),
];

/// The `last_used` row a server-wide cooldown is kept under
pub const EVERYONE: u64 = 0;

/// Who a cooldown applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, poise::ChoiceParameter)]
pub enum Scope {
    /// Each member waits on their own
    #[default]
    User,
    /// One use makes the whole server wait
    Global,
}

pub fn default_for(command: &str) -> u64 {
    DEFAULTS.iter().find(|(name, _)| *name == command).map_or(0, |(_, secs)| *secs)
}

pub const DB_FILE: &str = "database/cooldowns.db";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    seconds: u64,
    scope: Scope,
}

/// Configured cooldowns, cached in memory, and when each member last used each command,
/// both persisted to SQLite so running cooldowns survive restarts.
pub struct Cooldowns {
    conn: Connection,
    config: HashMap<(u64, String), Setting>,
}

impl Cooldowns {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/cooldowns.sql"))?;
        // Databases created before cooldowns could be server-wide
        migrations::add_column(&conn, "command_cooldowns", "global", "BOOLEAN NOT NULL DEFAULT 0")?;
        let config = {
            let mut stmt = conn.prepare("SELECT guild_id, command, seconds, global FROM command_cooldowns")?;
            let rows = stmt.query_map([], |row| {
                let scope = if row.get(3)? { Scope::Global } else { Scope::User };
                Ok(((row.get(0)?, row.get(1)?), Setting { seconds: row.get(2)?, scope }))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Self { conn, config })
    }

    /// Seconds between uses of `command`, by its qualified name (e.g. "eco work").
    pub fn seconds(&self, guild_id: u64, command: &str) -> u64 {
        self.setting(guild_id, command).seconds
    }

    pub fn scope(&self, guild_id: u64, command: &str) -> Scope {
        self.setting(guild_id, command).scope
    }

    pub fn setting(&self, guild_id: u64, command: &str) -> Setting {
        self.config
            .get(&(guild_id, command.to_string()))
            .copied()
            .unwrap_or_else(|| Setting { seconds: default_for(command), scope: Scope::User })
    }

    pub fn set(&mut self, guild_id: u64, command: &str, seconds: u64, scope: Scope) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO command_cooldowns (guild_id, command, seconds, global) VALUES (?, ?, ?, ?)",
            params![guild_id, command, seconds, scope == Scope::Global],
        )?;
        self.config.insert((guild_id, command.to_string()), Setting { seconds, scope });
        Ok(())
    }

    /// Whose last use counts: the member's, or anyone's for a server-wide cooldown.
    pub fn user_key(&self, guild_id: u64, user_id: u64, command: &str) -> u64 {
        if self.scope(guild_id, command) == Scope::Global { EVERYONE } else { user_id }
    }

    /// The Unix time the member can use the command again, if its cooldown is still running at `now`.
    pub fn ready_at(&self, guild_id: u64, user_id: u64, command: &str, now: i64) -> rusqlite::Result<Option<i64>> {
        let user_id = self.user_key(guild_id, user_id, command);
        let cooldown = self.seconds(guild_id, command) as i64;
        if cooldown == 0 {
            return Ok(None);
        }
        let last: Option<i64> = self
            .conn
            .query_row(
                "SELECT used_at FROM last_used WHERE guild_id = ? AND user_id = ? AND command = ?",
                params![guild_id, user_id, command],
                |row| row.get(0),
            )
            .optional()?;
        Ok(last.map(|last| last + cooldown).filter(|ready| now < *ready))
    }

    /// Records a use at Unix time `now`, or returns the Unix time the member can use the command again.
    pub fn start(&self, guild_id: u64, user_id: u64, command: &str, now: i64) -> rusqlite::Result<Result<(), i64>> {
        if self.seconds(guild_id, command) == 0 {
            return Ok(Ok(()));
        }
        if let Some(ready) = self.ready_at(guild_id, user_id, command, now)? {
            return Ok(Err(ready));
        }
        let user_id = self.user_key(guild_id, user_id, command);
        self.conn.execute(
            "INSERT OR REPLACE INTO last_used (guild_id, user_id, command, used_at) VALUES (?, ?, ?, ?)",
            params![guild_id, user_id, command, now],
        )?;
        Ok(Ok(()))
    }

    /// Removes settings for guilds the bot is no longer in.
    pub fn cleanup_stale(&mut self, current_guilds: &[serenity::GuildId]) {
        let in_guild = |id: u64| current_guilds.iter().any(|g| g.get() == id);
        let stale = self.conn.prepare("SELECT guild_id FROM command_cooldowns UNION SELECT guild_id FROM last_used").and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, u64>(0))?.collect::<rusqlite::Result<Vec<u64>>>()
        });
        let stale = match stale {
            Ok(ids) => ids.into_iter().filter(|g| !in_guild(*g)),
            Err(e) => return crate::log::error(format!("Error cleaning up stale cooldowns: {e}")),
        };
        for guild_id in stale {
            let deleted = self.conn.execute("DELETE FROM command_cooldowns WHERE guild_id = ?", [guild_id]).and_then(|_| {
                self.conn.execute("DELETE FROM last_used WHERE guild_id = ?", [guild_id])
            });
            if let Err(e) = deleted {
                crate::log::error(format!("Error cleaning up stale cooldowns: {e}"));
                continue;
            }
            self.config.retain(|(g, _), _| *g != guild_id);
        }
    }
}
