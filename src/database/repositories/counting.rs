//! Counting game channels and their current count.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

pub const DB_FILE: &str = "database/counting.db";

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

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/counting.sql"))?;
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
