-- Tables for guessing; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS guessing (
    guild_id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL,
    number INTEGER NOT NULL,
    guesses INTEGER NOT NULL DEFAULT 0,
    rounds INTEGER NOT NULL DEFAULT 0,
    best INTEGER NOT NULL DEFAULT 0
);
