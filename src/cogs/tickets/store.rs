//! Ticket system configuration and tickets, persisted to SQLite.

use rusqlite::{params, Connection, OptionalExtension, Row};

const DB_FILE: &str = "database/tickets.db";

#[derive(Clone, Debug)]
pub struct Setup {
    pub category_id: u64,
    pub panel_channel_id: u64,
    pub transcript_channel_id: u64,
    pub handler_role_id: u64,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct Ticket {
    pub guild_id: u64,
    pub channel_id: u64,
    /// The ticket's control message (the one with the buttons)
    pub message_id: u64,
    /// "<number>_<username>", also used in the channel name
    pub ticket_id: String,
    pub ticket_type: String,
    pub creator_id: u64,
    /// Everyone with access to the ticket, including the creator
    pub members: Vec<u64>,
    pub locked: bool,
    pub claimed_by: Option<u64>,
}

impl Ticket {
    /// The ticket number, e.g. "3" for ticket ID "3_wayne"
    pub fn number(&self) -> &str {
        self.ticket_id.split('_').next().unwrap_or_default()
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let members: Option<String> = row.get("members")?;
        Ok(Self {
            guild_id: row.get("guild_id")?,
            channel_id: row.get("channel_id")?,
            message_id: row.get("message_id")?,
            ticket_id: row.get("ticket_id")?,
            ticket_type: row.get("ticket_type")?,
            creator_id: row.get("creator_id")?,
            members: members.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or_default(),
            locked: row.get("locked")?,
            claimed_by: row.get("claimed_by")?,
        })
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        let conn = Connection::open(DB_FILE)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ticket_setup (
                guild_id INTEGER PRIMARY KEY,
                category_id INTEGER NOT NULL,
                panel_channel_id INTEGER NOT NULL,
                transcript_channel_id INTEGER NOT NULL,
                handler_role_id INTEGER NOT NULL,
                description TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tickets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER NOT NULL,
                channel_id INTEGER NOT NULL UNIQUE,
                message_id INTEGER NOT NULL,
                ticket_id TEXT NOT NULL,
                ticket_type TEXT NOT NULL,
                creator_id INTEGER NOT NULL,
                members TEXT NOT NULL,
                closed BOOLEAN NOT NULL DEFAULT 0,
                locked BOOLEAN NOT NULL DEFAULT 0,
                claimed_by INTEGER,
                -- Unix time after which an open ticket is closed for inactivity
                expires_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tickets_open ON tickets(closed, expires_at);",
        )?;
        Ok(Self { conn })
    }

    pub fn setup(&self, guild_id: u64) -> rusqlite::Result<Option<Setup>> {
        self.conn
            .query_row(
                "SELECT category_id, panel_channel_id, transcript_channel_id, handler_role_id, description
                 FROM ticket_setup WHERE guild_id = ?",
                [guild_id],
                |row| {
                    Ok(Setup {
                        category_id: row.get(0)?,
                        panel_channel_id: row.get(1)?,
                        transcript_channel_id: row.get(2)?,
                        handler_role_id: row.get(3)?,
                        description: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub fn save_setup(&self, guild_id: u64, setup: &Setup) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ticket_setup
             (guild_id, category_id, panel_channel_id, transcript_channel_id, handler_role_id, description)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                guild_id,
                setup.category_id,
                setup.panel_channel_id,
                setup.transcript_channel_id,
                setup.handler_role_id,
                setup.description,
            ],
        )?;
        Ok(())
    }

    /// The number for the next ticket in a guild.
    pub fn next_number(&self, guild_id: u64) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) + 1 FROM tickets WHERE guild_id = ?", [guild_id], |row| row.get(0))
    }

    pub fn create(&self, ticket: &Ticket, expires_at: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO tickets
             (guild_id, channel_id, message_id, ticket_id, ticket_type, creator_id, members, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                ticket.guild_id,
                ticket.channel_id,
                ticket.message_id,
                ticket.ticket_id,
                ticket.ticket_type,
                ticket.creator_id,
                serde_json::to_string(&ticket.members).unwrap(),
                expires_at,
            ],
        )?;
        Ok(())
    }

    /// The open ticket using this channel, if any.
    pub fn open_ticket(&self, channel_id: u64) -> rusqlite::Result<Option<Ticket>> {
        self.conn
            .query_row("SELECT * FROM tickets WHERE channel_id = ? AND closed = 0", [channel_id], Ticket::from_row)
            .optional()
    }

    pub fn expired_tickets(&self, now: i64) -> rusqlite::Result<Vec<Ticket>> {
        let mut stmt = self.conn.prepare("SELECT * FROM tickets WHERE closed = 0 AND expires_at <= ?")?;
        let rows = stmt.query_map([now], Ticket::from_row)?;
        rows.collect()
    }

    pub fn open_tickets_by_creator(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Vec<Ticket>> {
        let mut stmt = self.conn.prepare("SELECT * FROM tickets WHERE closed = 0 AND guild_id = ? AND creator_id = ?")?;
        let rows = stmt.query_map([guild_id, user_id], Ticket::from_row)?;
        rows.collect()
    }

    pub fn set_members(&self, channel_id: u64, members: &[u64]) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE tickets SET members = ? WHERE channel_id = ?",
            params![serde_json::to_string(members).unwrap(), channel_id],
        )?;
        Ok(())
    }

    pub fn set_locked(&self, channel_id: u64, locked: bool) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE tickets SET locked = ? WHERE channel_id = ?", params![locked, channel_id])?;
        Ok(())
    }

    pub fn set_claimed_by(&self, channel_id: u64, user_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE tickets SET claimed_by = ? WHERE channel_id = ?", params![user_id, channel_id])?;
        Ok(())
    }

    pub fn set_expiry(&self, channel_id: u64, expires_at: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE tickets SET expires_at = ? WHERE channel_id = ? AND closed = 0",
            params![expires_at, channel_id],
        )?;
        Ok(())
    }

    /// Marks a ticket closed. Returns false if it was already closed, so a ticket
    /// is only ever closed once even if several close paths race.
    pub fn mark_closed(&self, channel_id: u64) -> rusqlite::Result<bool> {
        let changed = self.conn.execute("UPDATE tickets SET closed = 1 WHERE channel_id = ? AND closed = 0", [channel_id])?;
        Ok(changed > 0)
    }
}
