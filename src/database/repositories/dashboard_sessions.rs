//! Sign-ins to the web dashboard, kept so restarting the bot doesn't sign everyone out.

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::database::connection;

pub const DB_FILE: &str = "database/dashboard_sessions.db";

/// What's stored for a cookie's token: never the token itself.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
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
        conn.execute_batch(include_str!("../../../migrations/dashboard_sessions.sql"))?;
        Ok(Self { conn })
    }

    pub fn save(&self, token_hash: &str, user_id: u64, data: &str, expires_at: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (token_hash, user_id, data, expires_at) VALUES (?, ?, ?, ?)",
            params![token_hash, user_id, data, expires_at],
        )?;
        Ok(())
    }

    pub fn remove(&self, token_hash: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE token_hash = ?", [token_hash])?;
        Ok(())
    }

    /// Deletes expired sign-ins, then returns the rest: (token hash, data, expires_at).
    pub fn load(&self, now: i64) -> rusqlite::Result<Vec<(String, String, i64)>> {
        self.conn.execute("DELETE FROM sessions WHERE expires_at <= ?", [now])?;
        let mut stmt = self.conn.prepare("SELECT token_hash, data, expires_at FROM sessions")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_survive_until_they_expire() {
        let s = Store::open_path(":memory:").unwrap();
        s.save(&token_hash("a"), 1, "{}", 100).unwrap();
        s.save(&token_hash("b"), 2, "{}", 200).unwrap();
        let loaded = s.load(150).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, token_hash("b"));
        s.remove(&token_hash("b")).unwrap();
        assert!(s.load(0).unwrap().is_empty());
        // The token isn't stored, only a hash of it
        assert_eq!(token_hash("b").len(), 64);
        assert_ne!(token_hash("b"), "b");
    }
}
