-- Tables for incidents; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

-- A ban, kick or timeout, with the member's account as it was at the time
CREATE TABLE IF NOT EXISTS incidents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    -- e.g. "banned", "kicked", "timed out"
    action TEXT NOT NULL,
    reason TEXT,
    moderator_id INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    username TEXT NOT NULL,
    -- Their server nickname or display name, if different from the username
    display_name TEXT,
    -- When they joined the server, if the bot still knew
    joined_at TEXT,
    -- Role IDs, comma separated
    roles TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS incidents_by_user ON incidents (guild_id, user_id);

-- The member's messages the bot had seen when the incident was opened
CREATE TABLE IF NOT EXISTS incident_messages (
    incident_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    content TEXT NOT NULL,
    -- Attachment URLs, one per line
    attachments TEXT NOT NULL DEFAULT '',
    sent_at TEXT NOT NULL,
    PRIMARY KEY (incident_id, message_id)
);
