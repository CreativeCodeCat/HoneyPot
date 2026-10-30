//! Incident reports: a ban, kick or timeout with the member's account details and messages
//! as they were at the time, kept as evidence.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::database::connection;

pub const DB_FILE: &str = "database/incidents.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incident {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    /// e.g. "banned", "kicked", "timed out"
    pub action: String,
    pub reason: Option<String>,
    pub moderator_id: u64,
    pub created_at: String,
    pub username: String,
    pub display_name: Option<String>,
    pub joined_at: Option<String>,
    pub roles: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceMessage {
    pub message_id: u64,
    pub channel_id: u64,
    pub content: String,
    pub attachments: Vec<String>,
    pub sent_at: String,
}

const COLUMNS: &str = "id, guild_id, user_id, action, reason, moderator_id, created_at, username, display_name, joined_at, roles";

fn incident(row: &Row) -> rusqlite::Result<Incident> {
    let roles: String = row.get(10)?;
    Ok(Incident {
        id: row.get(0)?,
        guild_id: row.get(1)?,
        user_id: row.get(2)?,
        action: row.get(3)?,
        reason: row.get(4)?,
        moderator_id: row.get(5)?,
        created_at: row.get(6)?,
        username: row.get(7)?,
        display_name: row.get(8)?,
        joined_at: row.get(9)?,
        roles: roles.split(',').filter_map(|id| id.parse().ok()).collect(),
    })
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
        conn.execute_batch(include_str!("../../../migrations/incidents.sql"))?;
        Ok(Self { conn })
    }

    /// Saves an incident and its messages, returning its ID. `incident.id` is ignored.
    pub fn create(&mut self, incident: &Incident, messages: &[EvidenceMessage]) -> rusqlite::Result<i64> {
        let tx = self.conn.transaction()?;
        let roles: Vec<String> = incident.roles.iter().map(u64::to_string).collect();
        tx.execute(
            "INSERT INTO incidents (guild_id, user_id, action, reason, moderator_id, created_at, username, display_name, joined_at, roles)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                incident.guild_id,
                incident.user_id,
                incident.action,
                incident.reason,
                incident.moderator_id,
                incident.created_at,
                incident.username,
                incident.display_name,
                incident.joined_at,
                roles.join(","),
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO incident_messages (incident_id, message_id, channel_id, content, attachments, sent_at) VALUES (?, ?, ?, ?, ?, ?)",
            )?;
            for m in messages {
                stmt.execute(params![id, m.message_id, m.channel_id, m.content, m.attachments.join("\n"), m.sent_at])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    /// The incident, if it's from this server.
    pub fn get(&self, guild_id: u64, id: i64) -> rusqlite::Result<Option<Incident>> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM incidents WHERE guild_id = ? AND id = ?"), params![guild_id, id], incident)
            .optional()
    }

    /// The server's incidents, newest first; `user_id` narrows to one member.
    pub fn list(&self, guild_id: u64, user_id: Option<u64>) -> rusqlite::Result<Vec<Incident>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM incidents WHERE guild_id = ?1 AND (?2 IS NULL OR user_id = ?2) ORDER BY id DESC"
        ))?;
        let rows = stmt.query_map(params![guild_id, user_id], incident)?;
        rows.collect()
    }

    /// An incident's messages, oldest first.
    pub fn messages(&self, incident_id: i64) -> rusqlite::Result<Vec<EvidenceMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id, channel_id, content, attachments, sent_at FROM incident_messages WHERE incident_id = ? ORDER BY message_id",
        )?;
        let rows = stmt.query_map([incident_id], |row| {
            let attachments: String = row.get(3)?;
            Ok(EvidenceMessage {
                message_id: row.get(0)?,
                channel_id: row.get(1)?,
                content: row.get(2)?,
                attachments: attachments.lines().map(String::from).filter(|a| !a.is_empty()).collect(),
                sent_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Removes incidents from servers the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<usize> {
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute(
            &format!("DELETE FROM incident_messages WHERE incident_id IN (SELECT id FROM incidents WHERE guild_id NOT IN ({ids}))"),
            [],
        )?;
        self.conn.execute(&format!("DELETE FROM incidents WHERE guild_id NOT IN ({ids})"), [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(guild_id: u64, user_id: u64) -> Incident {
        Incident {
            id: 0,
            guild_id,
            user_id,
            action: "banned".into(),
            reason: Some("spam".into()),
            moderator_id: 5,
            created_at: "2026-10-30T12:00:00+00:00".into(),
            username: "someone".into(),
            display_name: None,
            joined_at: Some("2026-01-01T00:00:00+00:00".into()),
            roles: vec![7, 8],
        }
    }

    fn message(id: u64) -> EvidenceMessage {
        EvidenceMessage { message_id: id, channel_id: 3, content: format!("message {id}"), attachments: vec!["https://cdn/a.png".into()], sent_at: "2026-10-30T11:00:00+00:00".into() }
    }

    #[test]
    fn saves_and_reads_back() {
        let mut s = Store::open_path(":memory:").unwrap();
        let id = s.create(&sample(1, 10), &[message(2), message(1)]).unwrap();
        let saved = s.get(1, id).unwrap().unwrap();
        assert_eq!(saved, Incident { id, ..sample(1, 10) });
        assert_eq!(s.messages(id).unwrap(), vec![message(1), message(2)]);
        // Other servers can't see it
        assert_eq!(s.get(2, id).unwrap(), None);
    }

    #[test]
    fn lists_newest_first_by_member() {
        let mut s = Store::open_path(":memory:").unwrap();
        let first = s.create(&sample(1, 10), &[]).unwrap();
        let second = s.create(&sample(1, 11), &[]).unwrap();
        let third = s.create(&sample(1, 10), &[]).unwrap();
        s.create(&sample(2, 10), &[]).unwrap();
        let ids = |list: Vec<Incident>| list.into_iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(ids(s.list(1, None).unwrap()), vec![third, second, first]);
        assert_eq!(ids(s.list(1, Some(10)).unwrap()), vec![third, first]);
    }

    #[test]
    fn forgets_servers_left() {
        let mut s = Store::open_path(":memory:").unwrap();
        let kept = s.create(&sample(1, 10), &[message(1)]).unwrap();
        let gone = s.create(&sample(2, 10), &[message(2)]).unwrap();
        assert_eq!(s.cleanup_stale(&[serenity::GuildId::new(1)]).unwrap(), 1);
        assert!(s.get(1, kept).unwrap().is_some());
        assert!(s.messages(gone).unwrap().is_empty());
    }
}
