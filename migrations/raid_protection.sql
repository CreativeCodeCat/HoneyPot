-- Tables for raid_protection; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    -- A raid is this many joins within `seconds`
    joins INTEGER NOT NULL,
    seconds INTEGER NOT NULL,
    -- Raid mode ends after this long without raid-speed joins
    quiet_minutes INTEGER NOT NULL,
    lock_channels BOOLEAN NOT NULL,
    raise_verification BOOLEAN NOT NULL,
    -- What happens to accounts that join during a raid: 'none', 'timeout', 'kick', 'ban' or 'quarantine'
    joiner_action TEXT NOT NULL,
    timeout_minutes INTEGER NOT NULL,
    alert_channel INTEGER
);

-- Servers in raid mode, with what to put back when it ends
CREATE TABLE IF NOT EXISTS raids (
    guild_id INTEGER PRIMARY KEY,
    started_at INTEGER NOT NULL,
    -- Unix time of the last raid-speed join
    last_activity INTEGER NOT NULL,
    -- The verification level before it was raised, if it was
    previous_verification INTEGER,
    -- Channels raid mode locked, comma separated
    locked_channels TEXT NOT NULL DEFAULT '',
    -- Accounts that joined during the raid
    joiners INTEGER NOT NULL DEFAULT 0
);
