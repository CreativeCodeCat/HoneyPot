-- Tables for bot_messages; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

-- Messages sent by bots, so the message log can skip them when they're deleted after
-- they've left the cache (sent before a restart, or too long ago)
CREATE TABLE IF NOT EXISTS bot_messages (
    message_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL
);
