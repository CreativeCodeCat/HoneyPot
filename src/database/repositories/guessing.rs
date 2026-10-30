//! Number guessing game channels and their secret numbers.

use ::serenity::all as serenity;
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

pub const MAX_NUMBER: u32 = 100;

pub const DB_FILE: &str = "database/guessing.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    /// 0 while guessing is stopped, which keeps the stats for next time
    pub channel_id: u64,
    pub number: u32,
    /// Guesses so far for the current number
    pub guesses: u32,
    /// Numbers found so far
    pub rounds: u32,
    /// Fewest guesses a number has been found in, 0 before any have
    pub best: u32,
}

/// What a number posted in the guessing channel did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Right, after this many guesses
    Found(u32),
    /// Wrong; true if the number is higher
    Higher(bool),
}

pub struct Store {
    conn: Connection,
}

pub fn new_number() -> u32 {
    rand::rng().random_range(1..=MAX_NUMBER)
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/guessing.sql"))?;
        Ok(Self { conn })
    }

    pub fn get(&self, guild_id: u64) -> rusqlite::Result<Option<Game>> {
        self.conn
            .query_row(
                "SELECT channel_id, number, guesses, rounds, best FROM guessing WHERE guild_id = ?",
                [guild_id],
                |row| Ok(Game { channel_id: row.get(0)?, number: row.get(1)?, guesses: row.get(2)?, rounds: row.get(3)?, best: row.get(4)? }),
            )
            .optional()
    }

    /// Sets the guessing channel with a new number, keeping the stats.
    pub fn set_channel(&self, guild_id: u64, channel_id: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO guessing (guild_id, channel_id, number) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id) DO UPDATE SET channel_id = ?2, number = ?3, guesses = 0",
            params![guild_id, channel_id, new_number()],
        )?;
        Ok(())
    }

    /// Stops guessing, keeping the stats. Returns false if it wasn't running.
    pub fn stop(&self, guild_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE guessing SET channel_id = 0 WHERE guild_id = ? AND channel_id != 0", [guild_id])? > 0)
    }

    /// Plays a guess, or `None` if `channel_id` isn't the guessing channel.
    pub fn guess(&self, guild_id: u64, channel_id: u64, guess: u32) -> rusqlite::Result<Option<Outcome>> {
        let Some(game) = self.get(guild_id)?.filter(|g| g.channel_id == channel_id) else { return Ok(None) };
        let guesses = game.guesses + 1;
        if guess != game.number {
            self.conn.execute("UPDATE guessing SET guesses = ? WHERE guild_id = ?", [guesses as u64, guild_id])?;
            return Ok(Some(Outcome::Higher(game.number > guess)));
        }
        let best = if game.best == 0 { guesses } else { game.best.min(guesses) };
        self.conn.execute(
            "UPDATE guessing SET number = ?, guesses = 0, rounds = rounds + 1, best = ? WHERE guild_id = ?",
            params![new_number(), best, guild_id],
        )?;
        Ok(Some(Outcome::Found(guesses)))
    }

    /// Removes games in guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute_batch(&format!("DELETE FROM guessing WHERE guild_id NOT IN ({})", ids.join(",")))
    }
}
