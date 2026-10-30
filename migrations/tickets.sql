-- Tables for tickets; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS ticket_setup (
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
CREATE INDEX IF NOT EXISTS idx_tickets_open ON tickets(closed, expires_at);
-- The last ticket number used in each guild. Kept apart from the tickets, as closed tickets
-- are deleted and numbers mustn't be reused
CREATE TABLE IF NOT EXISTS ticket_counters (
    guild_id INTEGER PRIMARY KEY,
    last_number INTEGER NOT NULL
);
