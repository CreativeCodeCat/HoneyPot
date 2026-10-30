-- Tables for applications; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    -- Where applications are posted for review
    channel_id INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE,
    role_id INTEGER,
    -- JSON array of question strings
    questions TEXT NOT NULL,
    UNIQUE (guild_id, name)
);
CREATE TABLE IF NOT EXISTS applications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    position_id INTEGER NOT NULL,
    -- 'pending', 'accepted' or 'denied'
    status TEXT NOT NULL,
    created TEXT NOT NULL
);
-- Posted panels, updated when positions change
CREATE TABLE IF NOT EXISTS panels (
    message_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL
);
