-- Tables for guild_config; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS cog_config (
    guild_id INTEGER,
    cog_name TEXT,
    enabled BOOLEAN DEFAULT 1,
    PRIMARY KEY (guild_id, cog_name)
);
