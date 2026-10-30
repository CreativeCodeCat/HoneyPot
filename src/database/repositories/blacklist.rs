//! Servers and users the bot owner has blacklisted.

use rusqlite::{params, Connection};

use crate::database::connection;
use crate::shared::utils::now_iso;

pub const BLACKLIST_DB: &str = "database/blacklist.db";

/// What a blacklist entry is: a server the bot won't stay in, or a user whose servers it won't
/// stay in and who can't use its commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlacklistKind {
    Server,
    User,
}

impl BlacklistKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::User => "user",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "server" => Some(Self::Server),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// A blacklisted server or user: (kind, ID, name when it was added).
pub type BlacklistEntry = (BlacklistKind, u64, String);

pub struct Blacklist {
    conn: Connection,
}

impl Blacklist {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(BLACKLIST_DB)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/blacklist.sql"))?;
        Ok(Self { conn })
    }

    pub fn add(&self, kind: BlacklistKind, id: u64, name: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO blacklist (kind, id, name, added) VALUES (?, ?, ?, ?)",
            params![kind.key(), id, name, now_iso()],
        )?;
        Ok(())
    }

    /// Returns false if it wasn't blacklisted.
    pub fn remove(&self, kind: BlacklistKind, id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM blacklist WHERE kind = ? AND id = ?", params![kind.key(), id])? > 0)
    }

    pub fn contains(&self, kind: BlacklistKind, id: u64) -> rusqlite::Result<bool> {
        self.conn.query_row("SELECT COUNT(*) > 0 FROM blacklist WHERE kind = ? AND id = ?", params![kind.key(), id], |row| row.get(0))
    }

    pub fn list(&self) -> rusqlite::Result<Vec<BlacklistEntry>> {
        let mut stmt = self.conn.prepare("SELECT kind, id, name FROM blacklist ORDER BY added DESC")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.filter_map(Result::ok).filter_map(|(kind, id, name)| Some((BlacklistKind::from_key(&kind)?, id, name))).collect())
    }
}
