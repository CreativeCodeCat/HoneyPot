-- Tables for quarantine; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    -- The Quarantine role, and the one channel it can see
    role_id INTEGER,
    channel_id INTEGER,
    -- Where review cards are posted for staff
    review_channel_id INTEGER,
    -- Accounts younger than this many days are quarantined when they join; NULL for off
    new_account_days INTEGER
);

-- Members in quarantine. Kept if they leave, so rejoining doesn't get them out.
CREATE TABLE IF NOT EXISTS quarantined (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    reason TEXT NOT NULL,
    -- What put them there, e.g. "Honeypot", "Raid protection", "New account"
    source TEXT NOT NULL,
    -- The roles taken away, to give back on release; comma separated
    saved_roles TEXT NOT NULL DEFAULT '',
    -- Unix time
    quarantined_at INTEGER NOT NULL,
    review_message_id INTEGER,
    PRIMARY KEY (guild_id, user_id)
);
