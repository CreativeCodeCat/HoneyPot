-- Tables for dashboard_log; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

-- Changes made through the web dashboard
CREATE TABLE IF NOT EXISTS actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    user_name TEXT NOT NULL,
    -- What they did, e.g. "Turned Levels off"
    action TEXT NOT NULL,
    -- Unix seconds
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS actions_guild ON actions (guild_id, at);
