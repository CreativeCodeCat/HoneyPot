//! Each server's rules, in sections.

use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

pub const DB_FILE: &str = "database/rules.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub body: String,
    pub colour: u32,
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
        conn.execute_batch(include_str!("../../../migrations/rules.sql"))?;
        Ok(Self { conn })
    }

    /// Sections in display order.
    pub fn sections(&self, guild_id: u64) -> rusqlite::Result<Vec<Section>> {
        let mut stmt =
            self.conn.prepare("SELECT title, body, colour FROM sections WHERE guild_id = ? ORDER BY position, id")?;
        let rows = stmt.query_map([guild_id], |row| Ok(Section { title: row.get(0)?, body: row.get(1)?, colour: row.get(2)? }))?;
        rows.collect()
    }

    /// Row ID of the section at a 1-based position.
    pub fn section_id(&self, guild_id: u64, number: usize) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM sections WHERE guild_id = ? ORDER BY position, id LIMIT 1 OFFSET ?",
                params![guild_id, number.saturating_sub(1) as i64],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn add(&self, guild_id: u64, section: &Section) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO sections (guild_id, position, title, body, colour)
             VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM sections WHERE guild_id = ?1), ?2, ?3, ?4)",
            params![guild_id, section.title, section.body, section.colour],
        )?;
        Ok(())
    }

    /// Replaces section `number` (1-based). Returns false if there's no such section.
    pub fn update(&self, guild_id: u64, number: usize, section: &Section) -> rusqlite::Result<bool> {
        let Some(id) = self.section_id(guild_id, number)? else { return Ok(false) };
        self.conn.execute(
            "UPDATE sections SET title = ?, body = ?, colour = ? WHERE id = ?",
            params![section.title, section.body, section.colour, id],
        )?;
        Ok(true)
    }

    /// Deletes section `number` (1-based). Returns false if there's no such section.
    pub fn remove(&self, guild_id: u64, number: usize) -> rusqlite::Result<bool> {
        let Some(id) = self.section_id(guild_id, number)? else { return Ok(false) };
        self.conn.execute("DELETE FROM sections WHERE id = ?", [id])?;
        Ok(true)
    }

    /// The channel and messages of the current rules post.
    pub fn post(&self, guild_id: u64) -> rusqlite::Result<Option<(u64, Vec<u64>)>> {
        self.conn
            .query_row("SELECT channel_id, message_ids FROM posts WHERE guild_id = ?", [guild_id], |row| {
                let ids: String = row.get(1)?;
                Ok((row.get(0)?, serde_json::from_str(&ids).unwrap_or_default()))
            })
            .optional()
    }

    pub fn save_post(&self, guild_id: u64, channel_id: u64, message_ids: &[u64]) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO posts (guild_id, channel_id, message_ids) VALUES (?, ?, ?)",
            params![guild_id, channel_id, serde_json::to_string(message_ids).unwrap()],
        )?;
        Ok(())
    }
}
