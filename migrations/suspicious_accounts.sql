-- Tables for suspicious_accounts; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    -- Staff are alerted when a member's score reaches this (0-100)
    threshold INTEGER NOT NULL,
    alert_channel INTEGER,
    -- Also quarantine members who reach the threshold
    auto_quarantine BOOLEAN NOT NULL
);

-- What each member has done since the bot started watching
CREATE TABLE IF NOT EXISTS activity (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    messages INTEGER NOT NULL DEFAULT 0,
    -- Unix time of their first message
    first_message_at INTEGER,
    joined_during_raid BOOLEAN NOT NULL DEFAULT 0,
    -- Messages with a Discord invite, mass mentions, links, and shouting or repeated characters
    invites INTEGER NOT NULL DEFAULT 0,
    mass_mentions INTEGER NOT NULL DEFAULT 0,
    links INTEGER NOT NULL DEFAULT 0,
    noisy INTEGER NOT NULL DEFAULT 0,
    -- Times they posted in a burst, or repeated the same message
    bursts INTEGER NOT NULL DEFAULT 0,
    duplicates INTEGER NOT NULL DEFAULT 0,
    -- Staff have been alerted about them
    alerted BOOLEAN NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, user_id)
);
