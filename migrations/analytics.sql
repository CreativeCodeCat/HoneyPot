-- Tables for analytics; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

-- A count per server, day (UTC, YYYY-MM-DD) and metric, like "joins" or "cmd:rank"
CREATE TABLE IF NOT EXISTS daily (
    guild_id INTEGER NOT NULL,
    day TEXT NOT NULL,
    metric TEXT NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (guild_id, day, metric)
);
-- Who sent a message each day, for counting active members
CREATE TABLE IF NOT EXISTS active (
    guild_id INTEGER NOT NULL,
    day TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    PRIMARY KEY (guild_id, day, user_id)
);
