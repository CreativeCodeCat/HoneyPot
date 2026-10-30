//! Locked channels, with what to restore and when the lock ends.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};
use serenity::Permissions as P;

use crate::database::connection;

pub const DB_FILE: &str = "database/lockdown.db";

pub struct Lockdown {
    pub channel_id: u64,
    /// @everyone's overwrite on the channel before the lockdown
    pub previous_allow: P,
    pub previous_deny: P,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        let conn = connection::open(DB_FILE)?;
        conn.execute_batch(include_str!("../../../migrations/lockdown.sql"))?;
        Ok(Self { conn })
    }

    pub fn lockdown_from_row(row: &rusqlite::Row) -> rusqlite::Result<Lockdown> {
        Ok(Lockdown {
            channel_id: row.get(0)?,
            previous_allow: P::from_bits_truncate(row.get(1)?),
            previous_deny: P::from_bits_truncate(row.get(2)?),
        })
    }

    pub fn get(&self, channel_id: u64) -> rusqlite::Result<Option<Lockdown>> {
        self.conn
            .query_row(
                "SELECT channel_id, previous_allow, previous_deny FROM lockdowns WHERE channel_id = ?",
                [channel_id],
                Self::lockdown_from_row,
            )
            .optional()
    }

    pub fn add(&self, guild_id: u64, lockdown: &Lockdown, expires_at: i64, reason: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO lockdowns (channel_id, guild_id, expires_at, previous_allow, previous_deny, reason)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                lockdown.channel_id,
                guild_id,
                expires_at,
                lockdown.previous_allow.bits(),
                lockdown.previous_deny.bits(),
                reason
            ],
        )?;
        Ok(())
    }

    pub fn remove(&self, channel_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM lockdowns WHERE channel_id = ?", [channel_id])?;
        Ok(())
    }

    pub fn expired(&self, now: i64) -> rusqlite::Result<Vec<Lockdown>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, previous_allow, previous_deny FROM lockdowns
             WHERE expires_at IS NOT NULL AND expires_at <= ?",
        )?;
        let rows = stmt.query_map([now], Self::lockdown_from_row)?;
        rows.collect()
    }
}
