-- Tables for offlimit; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS offlimit_channels (
    channel_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL
);
