//! Changes people make through the web dashboard, shown on each server's Audit page.

use ::serenity::all as serenity;
use rusqlite::{params, Connection};

use crate::database::connection;

pub const DB_FILE: &str = "database/dashboard_log.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub user_id: u64,
    pub user_name: String,
    pub action: String,
    /// Unix seconds
    pub at: i64,
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
        conn.execute_batch(include_str!("../../../migrations/dashboard_log.sql"))?;
        Ok(Self { conn })
    }

    pub fn add(&self, guild_id: u64, action: &Action) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO actions (guild_id, user_id, user_name, action, at) VALUES (?, ?, ?, ?, ?)",
            params![guild_id, action.user_id, action.user_name, action.action, action.at],
        )?;
        Ok(())
    }

    /// The server's actions, newest first.
    pub fn list(&self, guild_id: u64, limit: u32) -> rusqlite::Result<Vec<Action>> {
        let mut stmt = self.conn.prepare(
            "SELECT user_id, user_name, action, at FROM actions WHERE guild_id = ? ORDER BY at DESC, id DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![guild_id, limit], |row| {
            Ok(Action { user_id: row.get(0)?, user_name: row.get(1)?, action: row.get(2)?, at: row.get(3)? })
        })?;
        rows.collect()
    }

    /// Removes actions for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute_batch(&format!("DELETE FROM actions WHERE guild_id NOT IN ({})", ids.join(",")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_listed_newest_first_per_server() {
        let s = Store::open_path(":memory:").unwrap();
        let action = |text: &str, at| Action { user_id: 7, user_name: "Wayne".into(), action: text.into(), at };
        s.add(1, &action("Turned Levels on", 10)).unwrap();
        s.add(1, &action("Turned Levels off", 20)).unwrap();
        s.add(2, &action("Set the message log", 15)).unwrap();
        let list = s.list(1, 10).unwrap();
        assert_eq!(list.iter().map(|a| a.action.as_str()).collect::<Vec<_>>(), ["Turned Levels off", "Turned Levels on"]);
        assert_eq!(s.list(1, 1).unwrap().len(), 1);
        assert_eq!(s.list(2, 10).unwrap().len(), 1);
    }
}
