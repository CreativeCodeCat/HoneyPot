-- Tables for counting; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS counting (
    guild_id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    last_user INTEGER,
    high_score INTEGER NOT NULL DEFAULT 0
);
