-- Tables for leveling; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    rate INTEGER NOT NULL,
    min_xp INTEGER NOT NULL,
    max_xp INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS members (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    xp INTEGER NOT NULL DEFAULT 0,
    rep INTEGER NOT NULL DEFAULT 0,
    -- Unix times of the last XP award and the last reputation this member gave
    last_xp INTEGER,
    last_rep_given INTEGER,
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_members_xp ON members(guild_id, xp DESC);
