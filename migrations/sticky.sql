-- Tables for sticky; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS stickies (
    channel_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    max_count INTEGER NOT NULL,
    -- Messages posted since it was last moved down
    count INTEGER NOT NULL DEFAULT 0,
    message_id INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    -- Seconds a sticky waits once it's due before moving down
    delay_secs INTEGER NOT NULL
);
