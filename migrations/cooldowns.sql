-- Tables for cooldowns; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS command_cooldowns (
    guild_id INTEGER NOT NULL,
    command TEXT NOT NULL,
    seconds INTEGER NOT NULL,
    -- One cooldown for the whole server instead of one per member
    global BOOLEAN NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, command)
);
CREATE TABLE IF NOT EXISTS last_used (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    command TEXT NOT NULL,
    -- Unix time the member last used the command
    used_at INTEGER NOT NULL,
    PRIMARY KEY (guild_id, user_id, command)
);
