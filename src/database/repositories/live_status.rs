//! The Twitch streamer each server follows, and the live message posted while they stream.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::database::{connection, migrations};

pub const DB_FILE: &str = "database/live_status.db";

/// The green from the live card's accent bar and lines
pub const DEFAULT_COLOUR: u32 = 0x00d26a;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Streamer {
    pub guild_id: u64,
    pub twitch_login: String,
    pub channel_id: u64,
    pub ping_role_id: Option<u64>,
    pub tiktok_url: Option<String>,
    pub youtube_url: Option<String>,
    pub kick_url: Option<String>,
    pub spotlight: Option<String>,
    pub colour: u32,
    /// Set while a live message is posted
    pub stream_id: Option<String>,
    pub message_channel_id: Option<u64>,
    pub message_id: Option<u64>,
}

impl Streamer {
    pub fn new(guild_id: u64, twitch_login: String, channel_id: u64) -> Self {
        Self {
            guild_id,
            twitch_login,
            channel_id,
            ping_role_id: None,
            tiktok_url: None,
            youtube_url: None,
            kick_url: None,
            spotlight: None,
            colour: DEFAULT_COLOUR,
            stream_id: None,
            message_channel_id: None,
            message_id: None,
        }
    }
}

const COLUMNS: &str =
    "guild_id, twitch_login, channel_id, ping_role_id, tiktok_url, youtube_url, spotlight, colour, stream_id, message_channel_id, message_id, kick_url";

fn streamer(row: &Row) -> rusqlite::Result<Streamer> {
    Ok(Streamer {
        guild_id: row.get(0)?,
        twitch_login: row.get(1)?,
        channel_id: row.get(2)?,
        ping_role_id: row.get(3)?,
        tiktok_url: row.get(4)?,
        youtube_url: row.get(5)?,
        spotlight: row.get(6)?,
        colour: row.get(7)?,
        stream_id: row.get(8)?,
        message_channel_id: row.get(9)?,
        message_id: row.get(10)?,
        kick_url: row.get(11)?,
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
        conn.execute_batch(include_str!("../../../migrations/live_status.sql"))?;
        migrations::add_column(&conn, "streamers", "kick_url", "TEXT")?;
        Ok(Self { conn })
    }

    pub fn get(&self, guild_id: u64) -> rusqlite::Result<Option<Streamer>> {
        self.conn.query_row(&format!("SELECT {COLUMNS} FROM streamers WHERE guild_id = ?"), [guild_id], streamer).optional()
    }

    pub fn all(&self) -> rusqlite::Result<Vec<Streamer>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLUMNS} FROM streamers"))?;
        let rows = stmt.query_map([], streamer)?;
        rows.collect()
    }

    /// Saves everything about the server's streamer.
    pub fn set(&self, s: &Streamer) -> rusqlite::Result<()> {
        self.conn.execute(
            &format!("INSERT OR REPLACE INTO streamers ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
            params![
                s.guild_id,
                s.twitch_login,
                s.channel_id,
                s.ping_role_id,
                s.tiktok_url,
                s.youtube_url,
                s.spotlight,
                s.colour,
                s.stream_id,
                s.message_channel_id,
                s.message_id,
                s.kick_url
            ],
        )?;
        Ok(())
    }

    /// Records the live message posted for a stream, or clears it with `None`.
    pub fn set_live_message(&self, guild_id: u64, live: Option<(&str, u64, u64)>) -> rusqlite::Result<()> {
        let (stream_id, channel_id, message_id) = match live {
            Some((stream, channel, message)) => (Some(stream), Some(channel), Some(message)),
            None => (None, None, None),
        };
        self.conn.execute(
            "UPDATE streamers SET stream_id = ?, message_channel_id = ?, message_id = ? WHERE guild_id = ?",
            params![stream_id, channel_id, message_id, guild_id],
        )?;
        Ok(())
    }

    pub fn remove(&self, guild_id: u64) -> rusqlite::Result<Option<Streamer>> {
        let found = self.get(guild_id)?;
        self.conn.execute("DELETE FROM streamers WHERE guild_id = ?", [guild_id])?;
        Ok(found)
    }

    /// Removes streamers for servers the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<usize> {
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute(&format!("DELETE FROM streamers WHERE guild_id NOT IN ({})", ids.join(",")), [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_tracks_the_live_message() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.get(1).unwrap(), None);
        let mut streamer = Streamer::new(1, "somestreamer".into(), 5);
        streamer.ping_role_id = Some(7);
        streamer.spotlight = Some("Merch is out!".into());
        streamer.kick_url = Some("https://kick.com/somestreamer".into());
        s.set(&streamer).unwrap();
        assert_eq!(s.get(1).unwrap(), Some(streamer.clone()));

        s.set_live_message(1, Some(("stream1", 5, 99))).unwrap();
        let live = s.get(1).unwrap().unwrap();
        assert_eq!((live.stream_id.as_deref(), live.message_channel_id, live.message_id), (Some("stream1"), Some(5), Some(99)));
        s.set_live_message(1, None).unwrap();
        assert_eq!(s.get(1).unwrap(), Some(streamer));
        assert_eq!(s.all().unwrap().len(), 1);
        assert!(s.remove(1).unwrap().is_some());
        assert_eq!(s.get(1).unwrap(), None);
    }
}
