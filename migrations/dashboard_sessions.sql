-- Tables for dashboard_sessions; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

-- People signed in to the web dashboard, so restarting the bot doesn't sign them out
CREATE TABLE IF NOT EXISTS sessions (
    -- SHA-256 of the cookie's token, so this file can't be used to sign in
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    -- Who they are and the servers they manage, as JSON
    data TEXT NOT NULL,
    -- Unix seconds
    expires_at INTEGER NOT NULL
);
