-- Tables for rules; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS sections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    colour INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS posts (
    guild_id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL,
    -- JSON list of the message IDs currently posted
    message_ids TEXT NOT NULL
);
