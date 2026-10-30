-- Tables for lockdown; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS lockdowns (
    channel_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    -- Unix time to lift the lockdown
    expires_at INTEGER,
    previous_allow INTEGER NOT NULL,
    previous_deny INTEGER NOT NULL,
    reason TEXT NOT NULL
);
