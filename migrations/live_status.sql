-- Tables for live_status; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

-- The one streamer each server follows, and the live message posted for them
CREATE TABLE IF NOT EXISTS streamers (
    guild_id INTEGER PRIMARY KEY,
    twitch_login TEXT NOT NULL,
    channel_id INTEGER NOT NULL,
    ping_role_id INTEGER,
    tiktok_url TEXT,
    youtube_url TEXT,
    spotlight TEXT,
    colour INTEGER NOT NULL,
    -- The Twitch stream the live message is for, and the message
    stream_id TEXT,
    message_channel_id INTEGER,
    message_id INTEGER
);
